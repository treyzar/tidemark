//! UI request lifetime for explicit system actions. No GTK or host access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    id: u64,
    epoch: u64,
}
#[derive(Default)]
pub struct State {
    id: u64,
    epoch: u64,
    pending: Option<Request>,
    closed: bool,
}
impl State {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn begin(&mut self) -> Option<Request> {
        if self.closed || self.busy() {
            return None;
        }
        self.id = self.id.wrapping_add(1);
        let request = Request {
            id: self.id,
            epoch: self.epoch,
        };
        self.pending = Some(request);
        Some(request)
    }
    pub fn finish(&mut self, request: Request) -> bool {
        if self.closed || self.pending != Some(request) {
            return false;
        }
        self.pending = None;
        true
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.epoch = self.epoch.wrapping_add(1);
        self.pending = None;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_pending_request_and_stale_results_cannot_unlock_new_action() {
        let mut s = State::default();
        let first = s.begin().unwrap();
        assert!(s.begin().is_none());
        assert!(s.finish(first));
        let second = s.begin().unwrap();
        assert!(!s.finish(first));
        assert!(s.busy());
        assert!(s.finish(second));
    }
    #[test]
    fn closing_invalidates_results_and_never_starts_new_work() {
        let mut s = State::default();
        let first = s.begin().unwrap();
        s.close();
        assert!(!s.finish(first));
        assert!(!s.busy());
        assert!(s.begin().is_none());
    }
}
