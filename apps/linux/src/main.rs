//! Tidemark for Ubuntu: a GTK4 + libadwaita desktop island.
//!
//! The binary is Linux-only. On other platforms this file still builds and
//! the pure model tests run, but the executable explains that the native UI
//! needs Linux GTK libraries.

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod model;

#[cfg(test)]
#[path = "services/home_online.rs"]
mod home_online_tests;
#[cfg(all(test, not(target_os = "linux")))]
#[allow(dead_code)]
#[path = "services/shell.rs"]
mod shell_protocol_tests;
#[cfg(test)]
#[path = "services/online.rs"]
mod translation_state_tests;

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod services;
#[cfg(target_os = "linux")]
mod ui;

#[cfg(target_os = "linux")]
fn main() {
    let smoke = std::env::args().any(|arg| arg == "--smoke-test");
    app::run(smoke);
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "Tidemark's GTK4 UI is Linux-only. Build and run `cargo run -p tidemark` on Ubuntu 24.04 \
         with libgtk-4-dev and libadwaita-1-dev installed."
    );
    std::process::exit(1);
}
