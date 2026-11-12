//! GTK clipboard access. The island reads the clipboard only while it has
//! focus; background history needs the GNOME Shell adapter and stays disabled
//! until `org.tidemark.Shell1` exists.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;

pub fn write_text(text: &str) -> bool {
    let Some(display) = gdk::Display::default() else {
        return false;
    };
    display.clipboard().set_text(text);
    true
}

pub fn read_text<F>(callback: F)
where
    F: FnOnce(Option<String>) + 'static,
{
    let Some(display) = gdk::Display::default() else {
        callback(None);
        return;
    };
    let callback = Rc::new(RefCell::new(Some(callback)));
    let pending = callback.clone();
    display
        .clipboard()
        .read_text_async(None::<&gio::Cancellable>, move |result| {
            if let Some(callback) = pending.borrow_mut().take() {
                callback(result.ok().flatten().map(|text| text.to_string()));
            }
        });
}
