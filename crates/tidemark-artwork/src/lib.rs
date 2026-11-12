//! Bounded artwork fetch and decode for Tidemark on Ubuntu.
//!
//! Turns an `mpris:artUrl` (https/http/file/data) into a [`Cover`]: a 160x160 RGBA
//! image with a content-derived id. Input bytes and decoder allocations are
//! capped; HTTP redirects share a deadline and are checked for scheme changes
//! and credentials. Local input must be a regular file.

pub mod decode;
pub mod error;
pub mod source;
pub mod transport;

use std::time::Duration;

pub use error::{Error, Result};
pub use source::Cover;
pub use source::COVER_PX;

/// Maximum HTTP budget across redirects and reading the response body.
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Fetch artwork and decode it to a 160x160 RGBA cover.
pub fn fetch(url: &str) -> Result<Cover> {
    let agent = transport::agent(TOTAL_TIMEOUT);
    fetch_with_agent(url, &agent)
}

/// [`fetch`] with a caller-supplied agent (custom timeouts for tests, shared pools for callers).
pub fn fetch_with_agent(url: &str, agent: &ureq::Agent) -> Result<Cover> {
    let source = source::resolve(url, agent)?;
    decode::decode_to_cover(source)
}
