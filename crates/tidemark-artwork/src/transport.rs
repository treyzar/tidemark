//! Bounded http/https transport: manual redirects (same-scheme only), 8 MiB body cap,
//! caller-set global timeout. No cookies, no Authorization, no compressed responses.

use crate::error::{Error, Result};
use crate::source::MAX_BYTES;
use ureq::Agent;
use url::Url;

pub const MAX_REDIRECTS: usize = 3;

pub fn agent(total_timeout: std::time::Duration) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(total_timeout))
        .max_redirects(0)
        .user_agent("tidemark-artwork/0.1.0")
        .build()
        .into()
}

pub fn fetch_http(agent: &Agent, initial: Url) -> Result<Vec<u8>> {
    let budget = agent
        .config()
        .timeouts()
        .global
        .unwrap_or(crate::TOTAL_TIMEOUT)
        .min(crate::TOTAL_TIMEOUT);
    let deadline = std::time::Instant::now() + budget;
    let mut current = initial;
    for _ in 0..=MAX_REDIRECTS {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(Error::Timeout)?;
        let response = agent
            .get(current.as_str())
            .config()
            .timeout_global(Some(remaining))
            .max_redirects(0)
            .build()
            .call()
            .map_err(map_call_err)?;
        let status = response.status().as_u16();
        if !(300..400).contains(&status) {
            return read_body(response);
        }
        let location = response
            .headers()
            .get("location")
            .ok_or(Error::Redirect("3xx without Location"))?
            .to_str()
            .map_err(|_| Error::Redirect("Location is not valid UTF-8"))?;
        let next = current
            .join(location)
            .map_err(|_| Error::Redirect("Location is not a valid url"))?;
        if next.scheme() != current.scheme() {
            return Err(Error::Redirect("scheme change"));
        }
        if !next.username().is_empty() || next.password().is_some() {
            return Err(Error::Userinfo);
        }
        current = next;
    }
    Err(Error::Redirect("too many redirects"))
}

fn map_call_err(e: ureq::Error) -> Error {
    match e {
        ureq::Error::Timeout(_) => Error::Timeout,
        ureq::Error::BodyExceedsLimit(_) => Error::TooLarge,
        ureq::Error::Io(error) => Error::Io(error),
        ureq::Error::StatusCode(code) => Error::Status(code),
        e => Error::Http(e),
    }
}

fn read_body(mut response: ureq::http::Response<ureq::Body>) -> Result<Vec<u8>> {
    if let Some(len) = response.body().content_length() {
        if len > MAX_BYTES as u64 {
            return Err(Error::TooLarge);
        }
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES as u64)
        .read_to_vec()
        .map_err(map_call_err)?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}
