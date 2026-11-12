//! Explicit GTK clipboard fallback. Reads are asynchronous, bounded and
//! canceled after five seconds or an ownership change; no background polling.
use crate::services::shell::state::{text, MAX_TEXT_BYTES};
use gtk::{gdk, gio, glib, prelude::*};
use std::{cell::RefCell, rc::Rc, time::Duration};

pub fn write_text(text: &str) -> bool {
    let Some(display) = gdk::Display::default() else {
        return false;
    };
    display.clipboard().set_text(text);
    true
}
struct Read<F> {
    callback: RefCell<Option<F>>,
    bytes: RefCell<Vec<u8>>,
    cancel: gio::Cancellable,
    timer: RefCell<Option<glib::SourceId>>,
    changed: RefCell<Option<glib::SignalHandlerId>>,
    clipboard: gdk::Clipboard,
}
impl<F: FnOnce(Option<String>) + 'static> Read<F> {
    fn finish(&self, value: Option<String>) {
        let callback = self.callback.borrow_mut().take();
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
        if let Some(signal) = self.changed.borrow_mut().take() {
            self.clipboard.disconnect(signal);
        }
        self.cancel.cancel();
        if let Some(callback) = callback {
            callback(value);
        }
    }
}
pub fn read_text<F: FnOnce(Option<String>) + 'static>(callback: F) {
    let Some(display) = gdk::Display::default() else {
        callback(None);
        return;
    };
    let clipboard = display.clipboard();
    if clipboard
        .formats()
        .contain_mime_type("x-kde-passwordManagerHint")
    {
        callback(None);
        return;
    }
    let pending = Rc::new(Read {
        callback: RefCell::new(Some(callback)),
        bytes: RefCell::new(Vec::new()),
        cancel: gio::Cancellable::new(),
        timer: RefCell::new(None),
        changed: RefCell::new(None),
        clipboard: clipboard.clone(),
    });
    let timeout = pending.clone();
    let timer = glib::timeout_add_local_once(Duration::from_secs(5), move || {
        timeout.timer.borrow_mut().take(); // the one-shot source has expired
        timeout.finish(None);
    });
    pending.timer.replace(Some(timer));
    let changed = pending.clone();
    let signal = clipboard.connect_changed(move |_| changed.finish(None));
    pending.changed.replace(Some(signal));
    let read = pending.clone();
    clipboard.read_async(
        &["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"],
        glib::Priority::DEFAULT,
        Some(&pending.cancel),
        move |result| {
            if read.callback.borrow().is_none() {
                return;
            }
            match result {
                Ok((stream, _)) => read_chunk(stream, read),
                Err(_) => read.finish(None),
            }
        },
    );
}
fn read_chunk<F: FnOnce(Option<String>) + 'static>(stream: gio::InputStream, pending: Rc<Read<F>>) {
    if pending.callback.borrow().is_none() {
        return;
    }
    let remaining = MAX_TEXT_BYTES + 1 - pending.bytes.borrow().len();
    let next = stream.clone();
    let read = pending.clone();
    stream.read_bytes_async(
        remaining.min(16 * 1024),
        glib::Priority::DEFAULT,
        Some(&pending.cancel),
        move |result| {
            if read.callback.borrow().is_none() {
                return;
            }
            match result {
                Ok(bytes) if bytes.is_empty() => {
                    let result = text(&read.bytes.borrow()).ok();
                    read.finish(result);
                }
                Ok(bytes) => {
                    read.bytes.borrow_mut().extend_from_slice(bytes.as_ref());
                    if read.bytes.borrow().len() > MAX_TEXT_BYTES {
                        read.finish(None);
                    } else {
                        read_chunk(next, read);
                    }
                }
                Err(_) => read.finish(None),
            }
        },
    );
}
