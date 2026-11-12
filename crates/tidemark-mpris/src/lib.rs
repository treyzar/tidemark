//! Real MPRIS2 adapter for Tidemark on Ubuntu.
//!
//! Model, error and sleep-schedule modules are portable and tested on every
//! platform. The D-Bus transport ([`client::Client`]) is Linux-only: on other
//! platforms the crate exposes only the pure model so CI can run without
//! libdbus.

pub mod error;
pub mod model;
pub mod sleep;

#[cfg(target_os = "linux")]
pub mod client;

pub use error::{Error, Result};
pub use model::{MprisPlayerId, TrackSnapshot};
pub use sleep::SleepTimer;

#[cfg(target_os = "linux")]
pub use client::Client;
