//! Countdown timer state. The island arms one OS timer for the moment it
//! ends and ticks once a second only while the countdown is on screen.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    Idle,
    Running { ends: Instant },
    Paused { left: Duration },
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timer {
    pub length: Duration,
    pub state: State,
}

impl Default for Timer {
    fn default() -> Self {
        Self { length: Duration::from_secs(5 * 60), state: State::Idle }
    }
}

impl Timer {
    pub fn start(&mut self, length: Duration, now: Instant) {
        self.length = length;
        self.state = State::Running { ends: now + length };
    }

    pub fn toggle(&mut self, now: Instant) {
        self.state = match self.state {
            State::Running { ends } => State::Paused { left: ends.saturating_duration_since(now) },
            State::Paused { left } => State::Running { ends: now + left },
            State::Idle | State::Done => State::Running { ends: now + self.length },
        };
    }

    pub fn reset(&mut self) {
        self.state = State::Idle;
    }

    pub fn left(&self, now: Instant) -> Duration {
        match self.state {
            State::Running { ends } => ends.saturating_duration_since(now),
            State::Paused { left } => left,
            State::Idle => self.length,
            State::Done => Duration::ZERO,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.state, State::Running { .. })
    }

    /// Marks the timer done once its end has passed. True on that change.
    pub fn check_done(&mut self, now: Instant) -> bool {
        if let State::Running { ends } = self.state {
            if now + Duration::from_millis(50) >= ends {
                self.state = State::Done;
                return true;
            }
        }
        false
    }
}

/// "4:05", "1:02:03"; rounds up so the display never shows 0:00 early.
pub fn clock(left: Duration) -> String {
    let s = left.as_millis().div_ceil(1000) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// "5 мин", "1 ч 30 мин", "45 с".
pub fn length_label(d: Duration) -> String {
    let s = d.as_secs();
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, sec) => format!("{sec} с"),
        (0, m, _) => format!("{m} мин"),
        (h, 0, _) => format!("{h} ч"),
        (h, m, _) => format!("{h} ч {m} мин"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_pause_resume_finish() {
        let t0 = Instant::now();
        let mut t = Timer::default();
        t.start(Duration::from_secs(60), t0);
        assert_eq!(t.left(t0 + Duration::from_secs(20)), Duration::from_secs(40));
        t.toggle(t0 + Duration::from_secs(20)); // pause
        assert_eq!(t.left(t0 + Duration::from_secs(500)), Duration::from_secs(40));
        t.toggle(t0 + Duration::from_secs(100)); // resume
        assert!(!t.check_done(t0 + Duration::from_secs(120)));
        assert!(t.check_done(t0 + Duration::from_secs(140)));
        assert_eq!(t.state, State::Done);
        assert!(!t.check_done(t0 + Duration::from_secs(150)));
    }

    #[test]
    fn labels() {
        assert_eq!(clock(Duration::from_millis(245_100)), "4:06");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(clock(Duration::ZERO), "0:00");
        assert_eq!(length_label(Duration::from_secs(300)), "5 мин");
        assert_eq!(length_label(Duration::from_secs(5400)), "1 ч 30 мин");
        assert_eq!(length_label(Duration::from_secs(45)), "45 с");
    }
}
