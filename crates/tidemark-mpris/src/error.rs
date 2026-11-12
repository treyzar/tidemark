//! Explicit adapter errors. No silent fallbacks, no fake data.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("D-Bus session bus unavailable: {0}")]
    BusConnect(String),

    #[error("D-Bus call failed: {0}")]
    Dbus(String),

    #[error("Player {bus} is gone; it was owned by {owner}")]
    PlayerGone { bus: String, owner: String },

    #[error("Player {bus} changed owner: was {expected}, now {actual}")]
    OwnerChanged {
        bus: String,
        expected: String,
        actual: String,
    },

    #[error("Track changed: expected {expected:?}, player now reports {actual:?}")]
    TrackMismatch {
        expected: Option<String>,
        actual: Option<String>,
    },

    #[error("Player does not support {0}")]
    CapabilityMissing(&'static str),

    #[error("Volume must be finite and within 0.0..=1.0")]
    InvalidVolume,
}

pub type Result<T> = std::result::Result<T, Error>;
