//! Linux integration tests against a fake MPRIS player served on a private
//! D-Bus bus (run under `dbus-run-session`). Real desktop players are never
//! touched: the fake service lives in this file, and the bus is session-local.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tidemark_mpris::{Client, Error, MprisPlayerId};
use zvariant::{ObjectPath, Signature, Value};

const NAME_BASE: &str = "org.mpris.MediaPlayer2.";
const TRACK: &str = "/org/mpris/MediaPlayer2/Track/1";

/// Each test gets its own well-known name so parallel tests sharing the
/// private `dbus-run-session` bus cannot see or command each other's fakes.
const SUFFIX_DISCOVERY: &str = "fake.discovery";
const SUFFIX_COMMANDS: &str = "fake.commands";
const SUFFIX_VOLUME: &str = "fake.volume";
const SUFFIX_RESTART: &str = "fake.restart";
const SUFFIX_GHOST: &str = "fake.ghost";

#[derive(Debug, Clone)]
struct State {
    playing: bool,
    position_us: i64,
    track: String,
    volume: f64,
}

impl State {
    fn new() -> Self {
        Self {
            playing: true,
            position_us: 5_000_000,
            track: TRACK.to_string(),
            volume: 0.5,
        }
    }
}

fn metadata(state: &State) -> HashMap<String, Value<'static>> {
    let mut m = HashMap::new();
    m.insert(
        "mpris:trackid".into(),
        Value::ObjectPath(ObjectPath::try_from(state.track.clone()).unwrap().into()),
    );
    m.insert("xesam:title".into(), Value::from("Fake Song"));
    // Canonical MPRIS type for xesam:artist is 'as' (array of strings), not
    // an array of variants — serialize a typed string array.
    let mut artists = zvariant::Array::new(&Signature::Str);
    artists.append(Value::from("Fake Artist")).expect("'as' item");
    m.insert("xesam:artist".into(), Value::Array(artists));
    m.insert("xesam:album".into(), Value::from("Fake Album"));
    m.insert("mpris:artUrl".into(), Value::from("https://art.test/cover.png"));
    m.insert("mpris:length".into(), Value::from(200_000_000i64));
    m
}

struct FakePlayer {
    state: Arc<Mutex<State>>,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl FakePlayer {
    #[zbus(property)]
    fn playback_status(&self) -> String {
        if self.state.lock().unwrap().playing {
            "Playing".into()
        } else {
            "Paused".into()
        }
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        let state = self.state.lock().unwrap();
        metadata(&state)
    }

    // Canonical MPRIS Player-interface property, per the spec: Volume is NOT
    // on the root interface.
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.state.lock().unwrap().volume
    }

    #[zbus(property)]
    fn set_volume(&self, volume: f64) {
        self.state.lock().unwrap().volume = volume;
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        self.state.lock().unwrap().position_us
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }

    fn set_position(&self, track_id: ObjectPath, position: i64) -> zbus::fdo::Result<()> {
        let mut state = self.state.lock().unwrap();
        if track_id.as_str() != state.track {
            return Err(zbus::fdo::Error::Failed("track mismatch".into()));
        }
        state.position_us = position;
        Ok(())
    }

    fn play_pause(&self) -> zbus::fdo::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.playing = !state.playing;
        Ok(())
    }

    fn pause(&self) -> zbus::fdo::Result<()> {
        self.state.lock().unwrap().playing = false;
        Ok(())
    }

    fn next(&self) -> zbus::fdo::Result<()> {
        Ok(())
    }

    fn previous(&self) -> zbus::fdo::Result<()> {
        Ok(())
    }
}



struct FakeInstance {
    name: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FakeInstance {
    /// Serves one fake MPRIS instance on the session bus; the name is held
    /// until `stop()` is called, so a second instance can re-acquire it.
    fn serve(suffix: &'static str, state: Arc<Mutex<State>>) -> Self {
        let name = format!("{NAME_BASE}{suffix}");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_state = (state.clone(), stop.clone(), name.clone());
        let handle = std::thread::Builder::new()
            .name("fake-mpris".into())
            .spawn(move || {
                async_io::block_on(async move {
                    let (state, stop, name) = thread_state;
                    let conn = zbus::Connection::session().await.expect("session bus");
                    conn.object_server()
                        .at("/org/mpris/MediaPlayer2", FakePlayer { state })
                        .await
                        .expect("serve player");
                    conn.request_name(name.clone()).await.expect("request name");
                    while !stop.load(Ordering::SeqCst) {
                        async_io::Timer::after(Duration::from_millis(20)).await;
                    }
                    let _ = conn.release_name(name).await;
                })
            })
            .expect("spawn fake player thread");
        Self {
            name,
            state,
            stop,
            handle: Some(handle),
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.join().expect("fake player thread");
        }
    }
}

fn wait_for_player(client: &Client, name: &str) -> MprisPlayerId {
    for _ in 0..100 {
        if let Ok(players) = client.list_players() {
            if let Some(id) = players.into_iter().find(|p| p.bus_name == name) {
                return id;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("fake player {name} never appeared");
}

fn wait_player_gone(client: &Client, name: &str) {
    for _ in 0..100 {
        if let Ok(players) = client.list_players() {
            if !players.iter().any(|p| p.bus_name == name) {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("fake player {name} never went away");
}

#[test]
fn discovery_pins_owner_and_snapshot_roundtrips() {
    let fake = FakeInstance::serve(SUFFIX_DISCOVERY, Arc::new(Mutex::new(State::new())));
    let client = Client::connect().expect("connect");
    let id = wait_for_player(&client, &fake.name);

    assert!(id.owner.starts_with(':'), "owner must be unique: {id}");
    let snap = client.snapshot(&id).expect("snapshot");
    assert_eq!(snap.player, id);
    assert_eq!(snap.title, "Fake Song");
    assert_eq!(snap.artists, vec!["Fake Artist"]);
    assert_eq!(snap.album.as_deref(), Some("Fake Album"));
    // Art URL is returned unchanged; bounded download is a later task.
    assert_eq!(snap.art_url.as_deref(), Some("https://art.test/cover.png"));
    assert!(snap.playing);
    assert_eq!(snap.duration, Some(Duration::from_millis(200_000)));
    assert!(snap.can_seek && snap.can_next && snap.can_prev);
    assert_eq!(snap.volume, Some(0.5));
    assert_eq!(snap.track_id.as_deref(), Some(TRACK));
    drop(fake);
}

#[test]
fn commands_act_and_track_guard_blocks_stale_seek() {
    let fake = FakeInstance::serve(SUFFIX_COMMANDS, Arc::new(Mutex::new(State::new())));
    let client = Client::connect().expect("connect");
    let id = wait_for_player(&client, &fake.name);

    client.play_pause(&id).expect("play_pause");
    assert!(!fake.state.lock().unwrap().playing, "play_pause must toggle");
    client.play_pause(&id).expect("play_pause back");
    assert!(fake.state.lock().unwrap().playing);

    // Seeking with a stale track id must be refused, not applied.
    let wrong = client.seek(&id, "/org/mpris/MediaPlayer2/Track/999", Duration::from_secs(60));
    assert!(matches!(wrong, Err(Error::TrackMismatch { .. })), "{wrong:?}");
    assert_eq!(fake.state.lock().unwrap().position_us, 5_000_000);

    // Seeking the current track works.
    client.seek(&id, TRACK, Duration::from_secs(90)).expect("seek");
    assert_eq!(fake.state.lock().unwrap().position_us, 90_000_000);

    client.pause_exact(&id).expect("pause_exact");
    assert!(!fake.state.lock().unwrap().playing);
    drop(fake);
}

#[test]
fn volume_validation_and_roundtrip() {
    let fake = FakeInstance::serve(SUFFIX_VOLUME, Arc::new(Mutex::new(State::new())));
    let client = Client::connect().expect("connect");
    let id = wait_for_player(&client, &fake.name);

    assert!(matches!(
        client.player_volume(&id, 1.5),
        Err(Error::InvalidVolume)
    ));
    assert!(matches!(
        client.player_volume(&id, f64::NAN),
        Err(Error::InvalidVolume)
    ));
    client.player_volume(&id, 0.75).expect("set volume");
    assert_eq!(fake.state.lock().unwrap().volume, 0.75);
    let snap = client.snapshot(&id).expect("snapshot");
    assert_eq!(snap.volume, Some(0.75));
    drop(fake);
}

/// The real restart case: the well-known name is released and re-acquired by
/// a different connection (new unique owner). The stale identity must be
/// reported, never commanded; the fresh identity works.
#[test]
fn restarted_player_cannot_be_commanded_with_stale_identity() {
    let first = FakeInstance::serve(SUFFIX_RESTART, Arc::new(Mutex::new(State::new())));
    let client = Client::connect().expect("connect");
    let stale = wait_for_player(&client, &first.name);

    let name = first.name.clone();
    first.stop();
    wait_player_gone(&client, &name);

    let second = FakeInstance::serve(SUFFIX_RESTART, Arc::new(Mutex::new(State::new())));
    let fresh = wait_for_player(&client, &name);
    assert_eq!(fresh.bus_name, stale.bus_name);
    assert_ne!(fresh.owner, stale.owner, "restart must get a new unique owner");

    let err = client.play_pause(&stale).expect_err("stale owner must fail");
    assert!(
        matches!(err, Error::OwnerChanged { .. }),
        "expected OwnerChanged, got {err}"
    );
    client.play_pause(&fresh).expect("fresh identity commands the live player");
    assert!(!second.state.lock().unwrap().playing);
    second.stop();
}

#[test]
fn unknown_player_reports_player_gone() {
    let fake = FakeInstance::serve(SUFFIX_GHOST, Arc::new(Mutex::new(State::new())));
    let client = Client::connect().expect("connect");
    let _ = wait_for_player(&client, &fake.name);
    let ghost = MprisPlayerId::new(&format!("{NAME_BASE}absent"), ":1.1234");
    let err = client.play_pause(&ghost).expect_err("ghost must fail");
    assert!(matches!(err, Error::PlayerGone { .. }), "{err}");
}
