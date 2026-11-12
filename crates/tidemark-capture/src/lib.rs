//! Offline capture utilities plus an explicitly invoked Linux screenshot portal.
use thiserror::Error;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("capture transport unavailable")]
    Transport,
    #[error("capture portal owner changed")]
    OwnerChanged,
    #[error("capture portal returned an unexpected request path")]
    RequestPath,
    #[error("capture request canceled by the user")]
    Canceled,
    #[error("capture request failed")]
    Portal,
    #[error("capture operation timed out")]
    Timeout,
    #[error("capture resource limit exceeded")]
    Limit,
    #[error("invalid local capture image or URI")]
    Image,
    #[error("capture rectangle is outside the image")]
    Bounds,
    #[error("Tesseract is unavailable")]
    OcrUnavailable,
    #[error("Tesseract recognition failed; verify rus and eng language data are installed")]
    Ocr,
    #[error("QR code could not be decoded")]
    Qr,
}
pub type Result<T> = std::result::Result<T, Error>;
pub mod raster;
pub use raster::{Raster, Rect};
#[cfg(target_os = "linux")]
pub mod ocr;
#[cfg(target_os = "linux")]
pub mod portal;
