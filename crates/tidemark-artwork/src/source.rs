//! URL validation and scheme dispatch. Everything except the http transport is pure data.

use crate::error::{Error, Result};
use crate::transport;
use sha2::{Digest, Sha256};
use std::io::Read;
use url::Url;

pub const COVER_PX: u32 = 160;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cover {
    pub id: u64,
    pub pixels: Vec<u8>,
}

pub fn resolve(url: &str, agent: &ureq::Agent) -> Result<Vec<u8>> {
    let u = Url::parse(url).map_err(|error| Error::Url(error.to_string()))?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err(Error::Userinfo);
    }
    match u.scheme() {
        "http" | "https" => transport::fetch_http(agent, u),
        "file" => file_bytes(&u),
        "data" => data_bytes(u.as_str()),
        s => Err(Error::Scheme(s.to_string())),
    }
}

fn file_bytes(u: &Url) -> Result<Vec<u8>> {
    match u.host_str() {
        None | Some("") | Some("localhost") => {}
        Some(host) => return Err(Error::FileAuthority(host.to_string())),
    }
    let path = u.to_file_path().map_err(|_| Error::FilePath)?;
    if !path.is_absolute() {
        return Err(Error::FilePath);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // Opening a FIFO must not block before we can inspect the handle.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NONBLOCK);
    let file = options.open(&path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::NonRegularFile);
    }
    if meta.len() > MAX_BYTES as u64 {
        return Err(Error::TooLarge);
    }
    let mut bytes = Vec::with_capacity(meta.len().min(MAX_BYTES as u64) as usize);
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}

fn data_bytes(full: &str) -> Result<Vec<u8>> {
    let rest = full.strip_prefix("data:").ok_or(Error::DataUri("prefix"))?;
    let (header, encoded) = rest.split_once(',').ok_or(Error::DataUri("comma"))?;
    let header = header.to_ascii_lowercase();
    if !header.starts_with("image/") {
        return Err(Error::DataUri("not an image mime type"));
    }
    if !header.ends_with(";base64") {
        return Err(Error::DataUri("only base64 payloads are supported"));
    }
    // 3 bytes of output per 4 chars: reject before decoding allocates anything.
    if encoded.len() / 4 * 3 > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| Error::DataUri("invalid base64"))?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}

pub fn id_from_bytes(bytes: &[u8]) -> u64 {
    let hash = Sha256::digest(bytes);
    let mut head = [0u8; 8];
    head.copy_from_slice(&hash[..8]);
    u64::from_be_bytes(head)
}
