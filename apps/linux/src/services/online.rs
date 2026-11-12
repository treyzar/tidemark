//! Request identity used by the text tab. No GTK objects enter a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    generation: u64,
    revision: u64,
}
#[derive(Default)]
pub struct TranslationState {
    generation: u64,
    revision: u64,
    pending: Option<Request>,
}
impl TranslationState {
    pub fn edited(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn begin(&mut self) -> Option<Request> {
        if self.pending.is_some() {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        let request = Request {
            generation: self.generation,
            revision: self.revision,
        };
        self.pending = Some(request);
        Some(request)
    }
    pub fn finish(&mut self, request: Request) -> bool {
        if self.pending != Some(request) {
            return false;
        }
        self.pending = None;
        request.revision == self.revision
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edited_text_invalidates_pending_result() {
        let mut s = TranslationState::default();
        let request = s.begin().unwrap();
        s.edited();
        assert!(!s.finish(request));
        assert!(s.begin().is_some());
    }
    #[test]
    fn only_one_request_can_be_in_flight() {
        let mut s = TranslationState::default();
        let request = s.begin().unwrap();
        assert!(s.begin().is_none());
        assert!(s.finish(request));
        assert!(s.begin().is_some());
    }
    #[test]
    fn old_completion_cannot_finish_new_request() {
        let mut s = TranslationState::default();
        let old = s.begin().unwrap();
        assert!(s.finish(old));
        let fresh = s.begin().unwrap();
        assert!(!s.finish(old));
        assert!(s.begin().is_none());
        assert!(s.finish(fresh));
    }
}
