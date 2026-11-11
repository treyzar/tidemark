//! Minimal HTTPS GET on WinHTTP: TLS, proxies and certificates come from
//! Windows itself, so no TLS crate is linked into the binary.

use std::ffi::c_void;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn handle(h: *mut c_void) -> Option<Handle> {
    (!h.is_null()).then_some(Handle(h))
}

/// GETs `https://{host}{path}` and returns the body when the status is 200.
pub fn get(host: &str, path: &str) -> Option<String> {
    unsafe {
        let session = handle(WinHttpOpen(
            w!("WinIsland/0.1"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 5_000, 5_000, 5_000, 8_000).ok()?;
        let connect = handle(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT as u16, 0))?;
        let request = handle(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;

        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .ok()?;
        if status != 200 {
            return None;
        }

        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available).ok()?;
            if available == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + available as usize, 0u8);
            let mut read = 0u32;
            WinHttpReadData(request.0, body[start..].as_mut_ptr() as *mut c_void, available, &mut read).ok()?;
            body.truncate(start + read as usize);
            if body.len() > 1 << 20 {
                return None; // Nothing we ask for is this big.
            }
        }
        String::from_utf8(body).ok()
    }
}

/// Percent-encodes a query value (UTF-8, RFC 3986 unreserved kept as is).
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn encodes_cyrillic_and_spaces() {
        assert_eq!(encode("Санкт-Петербург"), "%D0%A1%D0%B0%D0%BD%D0%BA%D1%82-%D0%9F%D0%B5%D1%82%D0%B5%D1%80%D0%B1%D1%83%D1%80%D0%B3");
        assert_eq!(encode("New York"), "New%20York");
        assert_eq!(encode("a&b=c"), "a%26b%3Dc");
    }
}
