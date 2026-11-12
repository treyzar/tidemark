//! Bounded ask lifecycle, independent of the bounded notice/event channel.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
const MAX_PENDING: usize = 128; // 64 queued cards plus at most 64 active workers.
#[derive(Default)]
struct State {
    delivered: bool,
    canceled: bool,
}
#[derive(Clone, Default)]
pub(crate) struct Pending(Arc<Mutex<BTreeMap<u64, State>>>);
impl Pending {
    pub(crate) fn register(&self, id: u64) -> Option<Registration> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= MAX_PENDING || entries.contains_key(&id) {
            return None;
        }
        entries.insert(id, State::default());
        Some(Registration {
            pending: self.clone(),
            id,
            complete: false,
        })
    }
    /// Cancellation of a visible card precedes new cards/notices.
    pub(crate) fn canceled_delivered(&self) -> Option<u64> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let id = entries
            .iter()
            .find_map(|(&id, s)| (s.delivered && s.canceled).then_some(id))?;
        entries.remove(&id);
        Some(id)
    }
    /// Returns false for a card canceled while it was still in the event queue.
    pub(crate) fn deliver(&self, id: u64) -> bool {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match entries.get_mut(&id) {
            Some(state) if !state.canceled => {
                state.delivered = true;
                true
            }
            _ => {
                entries.remove(&id);
                false
            }
        }
    }
}
pub(crate) struct Registration {
    pending: Pending,
    id: u64,
    complete: bool,
}
impl Registration {
    pub(crate) fn complete(mut self) {
        self.pending
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.complete = true;
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        if !self.complete {
            if let Some(state) = self
                .pending
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(&self.id)
            {
                state.canceled = true;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_survives_a_stalled_ui_and_releases_slots() {
        let pending = Pending::default();
        let registrations: Vec<_> = (0..MAX_PENDING as u64)
            .map(|id| pending.register(id).unwrap())
            .collect();
        assert!(pending.register(999).is_none());
        assert!(pending.deliver(0));
        drop(registrations);
        assert_eq!(pending.canceled_delivered(), Some(0));
        for id in 1..MAX_PENDING as u64 {
            assert!(!pending.deliver(id));
        }
        assert!(pending.canceled_delivered().is_none());
        assert!(pending.register(999).is_some());
    }
    #[test]
    fn completed_reply_does_not_cancel_but_unwinding_worker_does() {
        let pending = Pending::default();
        let active = pending.register(1).unwrap();
        assert!(pending.deliver(1));
        active.complete();
        assert!(pending.canceled_delivered().is_none());
        let active = pending.register(2).unwrap();
        assert!(pending.deliver(2));
        drop(active);
        assert_eq!(pending.canceled_delivered(), Some(2));
    }
}
