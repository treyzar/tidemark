//! How Linux background services talk to the UI thread.
//!
//! Rules (architecture §4.2): one `std::thread` per service, sleeping on its
//! event source; commands go in through `Service::send`, which never blocks;
//! results come out as `Event`s through an `EventSink`. The GTK app's sink
//! forwards into the GLib main loop; tests use `std::sync::mpsc`.

use crate::event::Event;

pub trait EventSink: Send {
    /// Delivers `event`; `false` once the UI side is gone, so the service
    /// thread should stop.
    fn emit(&self, event: Event) -> bool;
}

impl EventSink for std::sync::mpsc::Sender<Event> {
    fn emit(&self, event: Event) -> bool {
        self.send(event).is_ok()
    }
}

impl EventSink for std::sync::mpsc::SyncSender<Event> {
    fn emit(&self, event: Event) -> bool {
        self.send(event).is_ok()
    }
}

pub trait Service {
    type Command: Send;
    /// Queues `command` for the service thread; must not block the caller.
    fn send(&self, command: Self::Command);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Osd, ServiceId};

    fn spawn_echo(sink: impl EventSink + 'static) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            assert!(sink.emit(Event::Osd { osd: Osd::CapsLock { on: true } }));
        })
    }

    #[test]
    fn mpsc_sender_is_event_sink() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_echo(tx).join().unwrap();
        assert_eq!(rx.recv().unwrap(), Event::Osd { osd: Osd::CapsLock { on: true } });
    }

    #[test]
    fn closed_sink_reports_false() {
        let (tx, rx) = std::sync::mpsc::channel::<Event>();
        drop(rx);
        assert!(!tx.emit(Event::ServiceError { service: ServiceId::Media, message: String::new() }));
    }
}
