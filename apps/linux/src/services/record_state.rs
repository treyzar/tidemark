//! Pure request lifecycle for the recording window. No effects or GTK values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request(u64);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Starting,
    Recording,
    Stopping,
    Cancelling,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Finish,
    Cancel,
}
#[derive(Default)]
pub struct Session {
    next: u64,
    pending: Option<Request>,
    phase: Phase,
}
impl Session {
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn begin(&mut self) -> Option<Request> {
        if self.phase != Phase::Idle {
            return None;
        }
        self.next = self.next.wrapping_add(1);
        let request = Request(self.next);
        self.pending = Some(request);
        self.phase = Phase::Starting;
        Some(request)
    }
    pub fn ready(&mut self, request: Request) -> bool {
        if self.pending == Some(request) && self.phase == Phase::Starting {
            self.phase = Phase::Recording;
            true
        } else {
            false
        }
    }
    pub fn finish(&mut self, request: Request) -> bool {
        if self.pending != Some(request) || self.phase == Phase::Closed {
            return false;
        }
        self.pending = None;
        self.phase = Phase::Idle;
        true
    }
    pub fn request_stop(&mut self) -> Option<Stop> {
        match self.phase {
            Phase::Starting => {
                self.phase = Phase::Cancelling;
                Some(Stop::Cancel)
            }
            Phase::Recording => {
                self.phase = Phase::Stopping;
                Some(Stop::Finish)
            }
            _ => None,
        }
    }
    pub fn close(&mut self) -> Option<Stop> {
        let stop = match self.phase {
            Phase::Starting | Phase::Cancelling => Some(Stop::Cancel),
            Phase::Recording | Phase::Stopping => Some(Stop::Finish),
            _ => None,
        };
        self.pending = None;
        self.phase = Phase::Closed;
        stop
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_request_and_stale_completion_cannot_release_new_recording() {
        let mut s = Session::default();
        let old = s.begin().unwrap();
        assert!(s.begin().is_none());
        assert!(s.finish(old));
        let current = s.begin().unwrap();
        assert!(!s.finish(old));
        assert_eq!(s.phase(), Phase::Starting);
        assert!(s.ready(current));
        assert_eq!(s.request_stop(), Some(Stop::Finish));
        assert_eq!(s.phase(), Phase::Stopping);
        assert!(s.request_stop().is_none());
        assert!(s.finish(current));
    }
    #[test]
    fn canceled_start_is_not_revived_by_late_ready() {
        let mut s = Session::default();
        let request = s.begin().unwrap();
        assert_eq!(s.request_stop(), Some(Stop::Cancel));
        assert!(!s.ready(request));
        assert_eq!(s.phase(), Phase::Cancelling);
        assert!(s.finish(request));
        assert_eq!(s.phase(), Phase::Idle);
    }
    #[test]
    fn closing_stops_recording_and_invalidates_all_ui_results() {
        let mut s = Session::default();
        let request = s.begin().unwrap();
        assert!(s.ready(request));
        assert_eq!(s.close(), Some(Stop::Finish));
        assert!(!s.finish(request));
        assert!(!s.ready(request));
        assert!(s.begin().is_none());
        let mut starting = Session::default();
        starting.begin().unwrap();
        assert_eq!(starting.close(), Some(Stop::Cancel));
    }
}
