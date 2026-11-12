use std::{fmt, io::Read, time::Duration};
pub type Result<T> = std::result::Result<T, Error>;
/// Deliberately contains no request URL, user text, or upstream error string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    Transport,
    Status(u16),
    RateLimited { retry_after: Duration },
    TooLarge,
    InvalidResponse,
    NotFound,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateLimited { retry_after } => write!(
                f,
                "online service rate limit; retry after {} seconds",
                retry_after.as_secs()
            ),
            Self::Status(s) => write!(f, "online service returned HTTP {s}"),
            other => f.write_str(match other {
                Self::InvalidInput => "invalid online service input",
                Self::Transport => "online service connection failed",
                Self::TooLarge => "online service size limit exceeded",
                Self::InvalidResponse => "invalid online service response",
                Self::NotFound => "online service record not found",
                Self::Status(_) | Self::RateLimited { .. } => unreachable!(),
            }),
        }
    }
}
impl std::error::Error for Error {}
#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
}
impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}
impl Client {
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .https_only(true)
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(20)))
                .timeout_connect(Some(Duration::from_secs(5)))
                .timeout_per_read(Some(Duration::from_secs(8)))
                .user_agent("Tidemark/0.1 (https://github.com/treyzar/tidemark)")
                .build()
                .into(),
        }
    }
    pub(crate) fn get(&self, url: &str) -> Result<String> {
        let mut response = self.agent.get(url).call().map_err(|_| Error::Transport)?;
        match response.status().as_u16() {
            200 => (),
            404 => return Err(Error::NotFound),
            429 => {
                return Err(Error::RateLimited {
                    retry_after: retry_delay(
                        response
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok()),
                    ),
                })
            }
            status => return Err(Error::Status(status)),
        }
        read_bounded(response.body_mut().as_reader(), 1 << 20)
    }
}
fn retry_delay(value: Option<&str>) -> Duration {
    Duration::from_secs(
        value
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60)
            .max(1),
    )
}
fn read_bounded(reader: impl Read, limit: usize) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Transport)?;
    if bytes.len() > limit {
        return Err(Error::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| Error::InvalidResponse)
}
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_and_utf8() {
        assert_eq!(read_bounded(&b"12345"[..], 4), Err(Error::TooLarge));
        assert_eq!(read_bounded(&b"1234"[..], 4).unwrap(), "1234");
        assert_eq!(read_bounded(&b"\xff"[..], 4), Err(Error::InvalidResponse));
        assert_eq!(encode("a&b=é /"), "a%26b%3D%C3%A9%20%2F");
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    fn serve(response: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/synthetic", listener.local_addr().unwrap());
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 2048];
            let _ = stream.read(&mut request).unwrap();
            let _ = stream.write_all(&response);
        });
        (url, thread)
    }
    fn local_client() -> Client {
        Client {
            agent: ureq::Agent::config_builder()
                .proxy(None)
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(2)))
                .build()
                .into(),
        }
    }
    #[test]
    fn transport_reads_chunked_body_and_rejects_oversize_without_content_length() {
        let (url, server) = serve(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ntest\r\n0\r\n\r\n".to_vec(),
        );
        assert_eq!(local_client().get(&url).unwrap(), "test");
        server.join().unwrap();
        let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        response.extend(vec![b'a'; (1 << 20) + 1]);
        let (url, server) = serve(response);
        assert_eq!(local_client().get(&url), Err(Error::TooLarge));
        server.join().unwrap();
    }
    #[test]
    fn redirects_and_error_bodies_are_never_returned() {
        let (url, server) = serve(b"HTTP/1.1 302 Found\r\nLocation: https://example.invalid/private\r\nContent-Length: 0\r\n\r\n".to_vec());
        assert_eq!(local_client().get(&url), Err(Error::Status(302)));
        server.join().unwrap();
        let (url, server) =
            serve(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 7\r\n\r\nprivate".to_vec());
        let err = local_client().get(&url).unwrap_err();
        assert_eq!(
            err,
            Error::RateLimited {
                retry_after: Duration::from_secs(60)
            }
        );
        assert!(!format!("{err:?} {err}").contains("private"));
        server.join().unwrap();
    }
    #[test]
    fn production_agent_rejects_insecure_transport() {
        assert_eq!(
            Client::new().get("http://127.0.0.1:1/synthetic"),
            Err(Error::Transport)
        );
    }
}

#[cfg(test)]
mod rate_limit_test {
    use super::*;
    #[test]
    fn rate_limit_has_retry_delay_without_disclosing_body() {
        assert_eq!(retry_delay(Some("120")), Duration::from_secs(120));
        assert_eq!(retry_delay(Some("invalid")), Duration::from_secs(60));
        let error = Error::RateLimited {
            retry_after: Duration::from_secs(120),
        };
        assert!(error.to_string().contains("120"));
    }
}
