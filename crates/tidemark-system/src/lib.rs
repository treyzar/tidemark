//! Explicit Linux controls. Construction never changes system settings.
use thiserror::Error;
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("system feature or device is unavailable")]
    Unsupported,
    #[error("system action was denied")]
    Denied,
    #[error("system service transport failed")]
    Transport,
    #[error("system operation timed out")]
    Timeout,
    #[error("system response limit exceeded")]
    Limit,
    #[error("invalid system response")]
    InvalidResponse,
    #[error("invalid system control input")]
    InvalidInput,
    #[error("system target identity changed")]
    Changed,
    #[error("requested system state was not confirmed")]
    NotConfirmed,
    #[error("another sample is required")]
    NotReady,
}
pub type Result<T> = std::result::Result<T, Error>;
#[cfg(target_os = "linux")]
pub mod brightness;
#[cfg(target_os = "linux")]
pub mod client;
pub mod metrics;
#[cfg(target_os = "linux")]
mod process;
pub mod volume;
#[cfg(target_os = "linux")]
pub use client::Client;
