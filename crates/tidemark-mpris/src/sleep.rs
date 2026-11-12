//! Sleep-timer scheduling model: captures the exact player identity, is not
//! affected by which track is playing or which player the UI currently shows,
//! can only be cancelled explicitly, and fires at most once. The worker that
//! calls the player is the parent's task; this module is the testable policy.

use std::time::{Duration, Instant};

use crate::model::MprisPlayerId;

#[derive(Debug, Clone, PartialEq)]
pub struct SleepTimer {
    player: MprisPlayerId,
    fire_at: Instant,
    cancelled: bool,
    fired: bool,
}

impl SleepTimer {
    /// Arms a timer for the captured player. Songs may end and the global
    /// current player may change while it runs: neither cancels it. Only
    /// [`SleepTimer::cancel`] does.
    pub fn arm(player: MprisPlayerId, after: Duration) -> Self {
        Self {
            player,
            fire_at: Instant::now() + after,
            cancelled: false,
            fired: false,
        }
    }

    /// Explicit cancellation, the only way a live timer stops early.
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn is_fired(&self) -> bool {
        self.fired
    }

    /// The captured identity, for display while the timer runs.
    pub fn player(&self) -> &MprisPlayerId {
        &self.player
    }

    /// Fires at most once: when the deadline has passed, returns the captured
    /// player identity exactly once so the caller can `pause_exact` it. Track
    /// changes on the captured player and switching the global current player
    /// do not block or redirect this; `Client::pause_exact` performs the live
    /// owner validation at execution time.
    pub fn take_due(&mut self, now: Instant) -> Option<MprisPlayerId> {
        if self.cancelled || self.fired || now < self.fire_at {
            return None;
        }
        self.fired = true;
        Some(self.player.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(owner: &str) -> MprisPlayerId {
        MprisPlayerId::new("org.mpris.MediaPlayer2.test", owner)
    }

    #[test]
    fn fires_once_with_the_captured_identity() {
        let mut t = SleepTimer::arm(player(":1.1"), Duration::from_millis(10));
        let now = Instant::now() + Duration::from_secs(1);
        assert_eq!(t.take_due(now), Some(player(":1.1")));
        assert!(t.is_fired());
        assert_eq!(t.take_due(now), None);
    }

    #[test]
    fn deadline_gates_firing() {
        let mut t = SleepTimer::arm(player(":1.1"), Duration::from_secs(60));
        assert_eq!(t.take_due(Instant::now()), None);
        assert_eq!(
            t.take_due(Instant::now() + Duration::from_secs(61)),
            Some(player(":1.1"))
        );
    }

    #[test]
    fn cancellation_is_explicit_and_prevents_firing() {
        let mut t = SleepTimer::arm(player(":1.1"), Duration::from_millis(10));
        t.cancel();
        assert!(t.is_cancelled());
        assert_eq!(t.take_due(Instant::now() + Duration::from_secs(1)), None);
    }

    #[test]
    fn natural_track_change_does_not_cancel_the_timer() {
        // The model has no track input at all: nothing can cancel it except
        // cancel(). The worker then targets the captured player, and
        // Client::pause_exact validates the live owner at execution time.
        let mut t = SleepTimer::arm(player(":1.1"), Duration::from_millis(10));
        assert_eq!(
            t.take_due(Instant::now() + Duration::from_secs(1)),
            Some(player(":1.1"))
        );
    }

    #[test]
    fn another_player_becoming_current_does_not_redirect_firing() {
        let mut t = SleepTimer::arm(player(":1.1"), Duration::from_millis(10));
        // Even if the global current player is now :1.2, the captured
        // identity is returned; there is no "current player" input here.
        assert_eq!(
            t.take_due(Instant::now() + Duration::from_secs(1)),
            Some(player(":1.1"))
        );
    }
}
