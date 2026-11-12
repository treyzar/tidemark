//! Linux service threads. Nothing here touches GTK objects: services talk to
//! the UI through `std::sync::mpsc` and the UI drains the channel on the GLib
//! main context.

pub mod autostart;
pub mod clipboard;
pub mod file_manager;
pub mod music;
