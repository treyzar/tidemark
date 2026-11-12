//! Portable data model, mirroring the Windows SMTC card (`src/media.rs`).

use std::time::{Duration, Instant};

/// A player identity pinned to the unique D-Bus owner (`:1.x`) that held the
/// well-known `org.mpris.MediaPlayer2.*` name when the player was discovered.
/// A restarted player gets a new unique name: every command re-checks and
/// reports [`Error::OwnerChanged`] instead of acting on the wrong instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MprisPlayerId {
    /// Well-known requested bus name, e.g. `org.mpris.MediaPlayer2.spotify`.
    pub bus_name: String,
    /// Unique owner name captured at discovery, e.g. `:1.42`.
    pub owner: String,
}

impl MprisPlayerId {
    pub fn new(bus_name: impl Into<String>, owner: impl Into<String>) -> Self {
        Self {
            bus_name: bus_name.into(),
            owner: owner.into(),
        }
    }

    /// Identity whose pinned owner no longer matches the live owner.
    pub fn owner_changed(&self, live_owner: &str) -> bool {
        self.owner != live_owner
    }
}

impl std::fmt::Display for MprisPlayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.bus_name, self.owner)
    }
}

/// A point-in-time view of a player, the MPRIS analogue of `MediaState`.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackSnapshot {
    pub player: MprisPlayerId,
    /// `mpris:trackid` of the current track, as published (kept unchanged).
    pub track_id: Option<String>,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    /// `mpris:artUrl`, returned unchanged; bounded downloading is a later task.
    pub art_url: Option<String>,
    pub playing: bool,
    /// Position as of `sampled_at`; MPRIS `Position` does not auto-advance.
    pub position: Duration,
    pub duration: Option<Duration>,
    pub can_prev: bool,
    pub can_next: bool,
    pub can_seek: bool,
    /// MediaPlayer2 root `Volume` (0.0..=1.0), if published.
    pub volume: Option<f64>,
    /// When `position` was sampled; used to extrapolate while playing.
    pub sampled_at: Instant,
}

impl TrackSnapshot {
    /// Live position: `Position` plus the time spent playing since sampling,
    /// capped at the track duration — same contract as the Windows card
    /// (`MediaState::position_now` in `src/media.rs`).
    pub fn position_now(&self) -> Duration {
        let pos = if self.playing {
            self.position + self.sampled_at.elapsed().min(Duration::from_secs(36_000))
        } else {
            self.position
        };
        match self.duration {
            Some(d) if d > Duration::ZERO => pos.min(d),
            _ => pos,
        }
    }

    /// Whether `track_id` still identifies the track this snapshot captured.
    pub fn track_matches(&self, track_id: &str) -> bool {
        self.track_id.as_deref() == Some(track_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(playing: bool, position: Duration, duration: Option<Duration>) -> TrackSnapshot {
        TrackSnapshot {
            player: MprisPlayerId::new("org.mpris.MediaPlayer2.test", ":1.1"),
            track_id: Some("/org/mpris/MediaPlayer2/Track/1".into()),
            title: "Song".into(),
            artists: vec!["Artist".into()],
            album: Some("Album".into()),
            art_url: Some("https://example.test/cover.png".into()),
            playing,
            position,
            duration,
            can_prev: true,
            can_next: true,
            can_seek: true,
            volume: Some(0.5),
            sampled_at: Instant::now(),
        }
    }

    #[test]
    fn paused_position_does_not_advance_and_playing_is_capped() {
        let mut s = snapshot(
            false,
            Duration::from_secs(50),
            Some(Duration::from_secs(200)),
        );
        s.sampled_at = Instant::now() - Duration::from_secs(5);
        assert_eq!(s.position_now(), Duration::from_secs(50));

        let mut s = snapshot(
            true,
            Duration::from_secs(199),
            Some(Duration::from_secs(200)),
        );
        s.sampled_at = Instant::now() - Duration::from_secs(5);
        assert_eq!(s.position_now(), Duration::from_secs(200));
    }

    #[test]
    fn extrapolation_is_bounded_when_duration_unknown() {
        let mut s = snapshot(true, Duration::from_secs(10), None);
        s.sampled_at = Instant::now() - Duration::from_secs(5);
        let pos = s.position_now();
        assert!(pos >= Duration::from_secs(15), "{pos:?}");
        assert!(pos < Duration::from_secs(16), "{pos:?}");
    }

    #[test]
    fn track_guard_is_exact() {
        let s = snapshot(true, Duration::from_secs(0), None);
        assert!(s.track_matches("/org/mpris/MediaPlayer2/Track/1"));
        assert!(!s.track_matches("/org/mpris/MediaPlayer2/Track/2"));
        assert!(!s.track_matches(""));
        let mut none = s.clone();
        none.track_id = None;
        assert!(!none.track_matches("/org/mpris/MediaPlayer2/Track/1"));
    }

    #[test]
    fn identity_pin_detects_owner_change() {
        let id = MprisPlayerId::new("org.mpris.MediaPlayer2.test", ":1.1");
        assert!(!id.owner_changed(":1.1"));
        assert!(id.owner_changed(":1.2"));
        assert_eq!(id.to_string(), "org.mpris.MediaPlayer2.test@:1.1");
    }
}
