//! Real D-Bus transport (Linux only). Implements the MPRIS2 spec
//! (https://specifications.freedesktop.org/mpris-spec/latest/) over zbus,
//! with every command pinned to the owner captured in [`MprisPlayerId`].
//!
//! Command safety contract:
//! - `verify_owner` re-checks that the well-known name still belongs to the
//!   captured unique owner (`:1.x`), reporting [`Error::OwnerChanged`] or
//!   [`Error::PlayerGone`] otherwise;
//! - the actual command is then sent with the **unique owner name as
//!   destination**, so the message always reaches the exact connection that
//!   was verified, never a player that migrated onto the well-known name;
//! - commands await the method reply (bounded by the connection's method
//!   timeout), so a remote rejection is surfaced as an error, not dropped.

use std::collections::HashMap;
use std::time::Duration;

use zbus::blocking::{connection::Builder, Proxy};
use zvariant::{ObjectPath, OwnedValue, Value};

use crate::error::{Error, Result};
use crate::model::{MprisPlayerId, TrackSnapshot};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const PLAYER_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
/// Bounded wait for every method call; zbus applies it to reply-awaiting calls.
const METHOD_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Client {
    conn: zbus::blocking::Connection,
}

impl Client {
    /// Connects to the session bus (must exist; under CI this is the private
    /// bus provided by `dbus-run-session`).
    pub fn connect() -> Result<Self> {
        let conn = Builder::session()
            .map_err(|e| Error::BusConnect(e.to_string()))?
            .method_timeout(METHOD_TIMEOUT)
            .build()
            .map_err(|e| Error::BusConnect(e.to_string()))?;
        Ok(Self { conn })
    }

    /// All MPRIS players on the bus, each pinned to its unique owner.
    pub fn list_players(&self) -> Result<Vec<MprisPlayerId>> {
        let bus = self.bus_proxy();
        let names: Vec<String> = bus
            .call("ListNames", &())
            .map_err(|e| Error::Dbus(e.to_string()))?;
        let mut out = Vec::new();
        for name in names {
            if !name.starts_with(MPRIS_PREFIX) {
                continue;
            }
            if !self.name_has_owner(&bus, &name)? {
                continue;
            }
            let owner = self.owner_of(&bus, &name)?;
            out.push(MprisPlayerId::new(name, owner));
        }
        out.sort_by(|a, b| a.bus_name.cmp(&b.bus_name));
        Ok(out)
    }

    /// Reads a full snapshot from the pinned owner, re-verifying it first.
    /// All properties, including `Volume`, are on the Player interface.
    pub fn snapshot(&self, id: &MprisPlayerId) -> Result<TrackSnapshot> {
        self.verify_owner(id)?;
        let player = self.player_proxy(id)?;

        let metadata = metadata_map(&player)?;
        // PlaybackStatus is a mandatory Player property; a missing one is a
        // protocol error, not a silent "paused".
        let status: String = player
            .get_property("PlaybackStatus")
            .map_err(|e| Error::Dbus(e.to_string()))?;
        let playing = status == "Playing";
        let position = player
            .get_property::<i64>("Position")
            .ok()
            .filter(|p| *p >= 0)
            .map(|p| Duration::from_micros(p as u64));
        let volume = player
            .get_property::<f64>("Volume")
            .ok()
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 1.0));

        Ok(TrackSnapshot {
            player: id.clone(),
            track_id: string_of(&metadata, "mpris:trackid"),
            title: string_of(&metadata, "xesam:title").unwrap_or_default(),
            artists: strings_of(&metadata, "xesam:artist"),
            album: string_of(&metadata, "xesam:album"),
            art_url: string_of(&metadata, "mpris:artUrl"),
            playing,
            position: position.unwrap_or_default(),
            duration: metadata.get("mpris:length").and_then(|v| match v {
                Value::I64(micros) if *micros >= 0 => Some(Duration::from_micros(*micros as u64)),
                _ => None,
            }),
            can_prev: player
                .get_property::<bool>("CanGoPrevious")
                .unwrap_or(false),
            can_next: player.get_property::<bool>("CanGoNext").unwrap_or(false),
            can_seek: player.get_property::<bool>("CanSeek").unwrap_or(false),
            volume,
            sampled_at: std::time::Instant::now(),
        })
    }

    /// Toggles play/pause on the pinned owner only.
    pub fn play_pause(&self, id: &MprisPlayerId) -> Result<()> {
        self.verify_owner(id)?;
        let _: () = self
            .player_proxy(id)?
            .call("PlayPause", &())
            .map_err(|e| Error::Dbus(e.to_string()))?;
        Ok(())
    }

    /// Pause on the exact pinned owner: a restarted or replaced player that
    /// re-acquired the bus name is reported, never commanded.
    pub fn pause_exact(&self, id: &MprisPlayerId) -> Result<()> {
        self.verify_owner(id)?;
        let _: () = self
            .player_proxy(id)?
            .call("Pause", &())
            .map_err(|e| Error::Dbus(e.to_string()))?;
        Ok(())
    }

    pub fn next(&self, id: &MprisPlayerId) -> Result<()> {
        self.verify_owner(id)?;
        let player = self.player_proxy(id)?;
        if !player.get_property::<bool>("CanGoNext").unwrap_or(false) {
            return Err(Error::CapabilityMissing("Next"));
        }
        let _: () = player
            .call("Next", &())
            .map_err(|e| Error::Dbus(e.to_string()))?;
        Ok(())
    }

    pub fn previous(&self, id: &MprisPlayerId) -> Result<()> {
        self.verify_owner(id)?;
        let player = self.player_proxy(id)?;
        if !player
            .get_property::<bool>("CanGoPrevious")
            .unwrap_or(false)
        {
            return Err(Error::CapabilityMissing("Previous"));
        }
        let _: () = player
            .call("Previous", &())
            .map_err(|e| Error::Dbus(e.to_string()))?;
        Ok(())
    }

    /// Seeks to `position` only while the player still reports `track_id` as
    /// the current track; any change is [`Error::TrackMismatch`], never a
    /// seek on the wrong track.
    pub fn seek(&self, id: &MprisPlayerId, track_id: &str, position: Duration) -> Result<()> {
        self.verify_owner(id)?;
        let player = self.player_proxy(id)?;
        if !player.get_property::<bool>("CanSeek").unwrap_or(false) {
            return Err(Error::CapabilityMissing("Seek"));
        }
        let metadata = metadata_map(&player)?;
        let current = string_of(&metadata, "mpris:trackid").ok_or(Error::TrackMismatch {
            expected: Some(track_id.to_string()),
            actual: None,
        })?;
        if current != track_id {
            return Err(Error::TrackMismatch {
                expected: Some(track_id.to_string()),
                actual: Some(current.clone()),
            });
        }
        let path = ObjectPath::try_from(current.as_str())
            .map_err(|e| Error::Dbus(format!("track id is not a D-Bus object path: {e}")))?;
        let _: () = player
            .call("SetPosition", &(path, position.as_micros() as i64))
            .map_err(|e| Error::Dbus(e.to_string()))?;
        Ok(())
    }

    /// Sets the Player interface `Volume` (0.0..=1.0) on the pinned owner.
    pub fn player_volume(&self, id: &MprisPlayerId, volume: f64) -> Result<()> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(Error::InvalidVolume);
        }
        self.verify_owner(id)?;
        self.player_proxy(id)?
            .set_property("Volume", volume)
            .map_err(|e| Error::Dbus(e.to_string()))
    }

    /// Requests that the pinned player's UI come forward using the MPRIS root
    /// interface. A successful reply acknowledges the request; the desktop may
    /// still prevent focus or foreground activation.
    pub fn raise(&self, id: &MprisPlayerId) -> Result<()> {
        self.verify_owner(id)?;
        let root = Proxy::new(&self.conn, id.owner.as_str(), PLAYER_PATH, ROOT_IFACE)
            .map_err(|e| Error::Dbus(e.to_string()))?;
        if !root
            .get_property::<bool>("CanRaise")
            .map_err(|e| Error::Dbus(e.to_string()))?
        {
            return Err(Error::CapabilityMissing("Raise"));
        }
        let _: () = root
            .call("Raise", &())
            .map_err(|e| Error::Dbus(e.to_string()))?;
        Ok(())
    }

    // -- internals ----------------------------------------------------------

    fn bus_proxy(&self) -> Proxy<'_> {
        Proxy::new(
            &self.conn,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .expect("statically valid bus proxy names")
    }

    /// Proxy addressed to the **unique owner** — the exact connection that
    /// `verify_owner` just confirmed holds the well-known name.
    fn player_proxy<'a>(&'a self, id: &'a MprisPlayerId) -> Result<Proxy<'a>> {
        Proxy::new(
            &self.conn,
            id.owner.as_str(),
            ObjectPath::try_from(PLAYER_PATH).map_err(|e| Error::Dbus(e.to_string()))?,
            PLAYER_IFACE,
        )
        .map_err(|e| Error::Dbus(e.to_string()))
    }

    fn name_has_owner(&self, bus: &Proxy<'_>, name: &str) -> Result<bool> {
        let has: bool = bus
            .call("NameHasOwner", &(name,))
            .map_err(|e| Error::Dbus(e.to_string()))?;
        Ok(has)
    }

    fn owner_of(&self, bus: &Proxy<'_>, name: &str) -> Result<String> {
        let owner: String = bus
            .call("GetNameOwner", &(name,))
            .map_err(|e| Error::Dbus(e.to_string()))?;
        Ok(owner)
    }

    /// Live owner check against the unique name captured at discovery.
    fn verify_owner(&self, id: &MprisPlayerId) -> Result<()> {
        let bus = self.bus_proxy();
        if !self.name_has_owner(&bus, &id.bus_name)? {
            return Err(Error::PlayerGone {
                bus: id.bus_name.clone(),
                owner: id.owner.clone(),
            });
        }
        let live = self.owner_of(&bus, &id.bus_name)?;
        if live != id.owner {
            return Err(Error::OwnerChanged {
                bus: id.bus_name.clone(),
                expected: id.owner.clone(),
                actual: live,
            });
        }
        Ok(())
    }
}

fn metadata_map(player: &Proxy<'_>) -> Result<HashMap<String, Value<'static>>> {
    let owned: HashMap<String, OwnedValue> = player
        .get_property("Metadata")
        .map_err(|e| Error::Dbus(e.to_string()))?;
    Ok(owned
        .into_iter()
        .map(|(k, v)| (k, Value::from(v)))
        .collect())
}

fn string_of(metadata: &HashMap<String, Value<'static>>, key: &str) -> Option<String> {
    value_as_string(metadata.get(key)?)
}

fn strings_of(metadata: &HashMap<String, Value<'static>>, key: &str) -> Vec<String> {
    match metadata.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(value_as_string).collect(),
        Some(v) => value_as_string(v).into_iter().collect(),
        None => Vec::new(),
    }
}

fn value_as_string(value: &Value<'_>) -> Option<String> {
    match value {
        Value::Str(s) => Some(s.to_string()),
        Value::ObjectPath(p) => Some(p.to_string()),
        _ => None,
    }
}
