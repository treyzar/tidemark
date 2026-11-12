#[derive(Debug, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub secure: bool,
    pub prefix: String,
}
pub fn parse(url: &str) -> Result<Endpoint, String> {
    let bad = || "Нужен адрес bb: http://127.0.0.1:порт или https://сервер".to_string();
    if url.chars().any(|c| c.is_whitespace() || c.is_control())
        || url.contains(['?', '#', '@', '\\'])
    {
        return Err(bad());
    }
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(bad());
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if path.split('/').any(|p| p == ".." || p == ".") || path.contains('%') {
        return Err(bad());
    }
    let (host, port) = if let Some(r) = authority.strip_prefix('[') {
        let (h, tail) = r.split_once(']').ok_or_else(bad)?;
        let p = if tail.is_empty() {
            None
        } else {
            Some(tail.strip_prefix(':').ok_or_else(bad)?)
        };
        (h, p)
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c))
    {
        return Err(bad());
    }
    if !secure && !matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return Err("Для удалённого bb нужен HTTPS".into());
    }
    let port = match port {
        Some(p) => p.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(bad)?,
        None => {
            if secure {
                443
            } else {
                80
            }
        }
    };
    let path = path.trim_end_matches('/');
    Ok(Endpoint {
        host: host.into(),
        port,
        secure,
        prefix: if path.is_empty() {
            String::new()
        } else {
            format!("/{path}")
        },
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_http() {
        let e = parse("http://127.0.0.1:38886/").unwrap();
        assert_eq!(e.port, 38886);
        assert!(!e.secure);
        assert_eq!(e.prefix, "");
    }
    #[test]
    fn remote_https() {
        let e = parse("https://bb.example.org/bb/").unwrap();
        assert_eq!(e.host, "bb.example.org");
        assert_eq!(e.prefix, "/bb");
        assert!(e.secure);
    }
    #[test]
    fn reject_unsafe_urls() {
        for s in [
            "http://example.org",
            "https://user:pass@example.org",
            "https://bb.org/?token=x",
            "https://bb.org/#x",
            "file:///bb",
            "https://bb.org\r\nx:",
            "https://bb.org/../x",
            "http://127.0.0.1.evil.org",
        ] {
            assert!(parse(s).is_err(), "{s}");
        }
    }
    #[test]
    fn ipv6_loopback() {
        assert_eq!(parse("http://[::1]:38886").unwrap().host, "::1");
    }
}
