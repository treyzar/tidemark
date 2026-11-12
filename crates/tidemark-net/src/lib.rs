//! Caller-driven, blocking online services. Run these outside the GUI thread.
//! No timers, disk writes, cookies, credentials or background uploads.
#![forbid(unsafe_code)]
pub mod lyrics;
pub mod translate;
mod transport;
pub mod weather;
pub use transport::{Client, Error, Result};
