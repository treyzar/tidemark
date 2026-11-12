//! Pure launcher matching, arithmetic and URL/path validation. No shell strings.
use std::path::{Path, PathBuf};
pub const MAX_RESULTS: usize = 20;
pub fn score(name: &str, query: &str) -> u32 {
    let name = name.to_lowercase();
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return 0;
    }
    if name == query {
        return 1000;
    }
    if name.starts_with(&query) {
        return 800 - name.chars().count().min(200) as u32;
    }
    if name
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with(&query))
    {
        return 600 - name.chars().count().min(200) as u32;
    }
    if name.contains(&query) {
        return 400 - name.chars().count().min(200) as u32;
    }
    let initials: String = name
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| w.chars().next())
        .collect();
    if query.chars().count() >= 2 && initials.starts_with(&query) {
        return 300;
    }
    0
}
pub fn ranked(names: &[String], query: &str, limit: usize) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut hits: Vec<_> = names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            let s = score(n, &q);
            (q.is_empty() || s > 0).then_some((s, i))
        })
        .collect();
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| names[a.1].to_lowercase().cmp(&names[b.1].to_lowercase()))
    });
    hits.into_iter()
        .take(limit.min(MAX_RESULTS))
        .map(|(_, i)| i)
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_prefix_word_and_initials_rank_in_order() {
        assert!(score("TELEGRAM", "telegram") > score("Telegram Desktop", "tele"));
        assert!(score("Telegram Desktop", "tele") > score("Google Chrome", "chrome"));
        assert!(score("Google Chrome", "chrome") > score("Photoshop", "shop"));
        assert!(score("Photoshop", "shop") > score("Visual Studio Code", "vsc"));
    }
    #[test]
    fn search_is_unicode_case_safe_and_empty_is_bounded() {
        let names = vec!["Яндекс Музыка".into(), "Firefox".into(), "Telegram".into()];
        assert_eq!(ranked(&names, "яндекс", 20), vec![0]);
        assert_eq!(ranked(&names, "", 2), vec![1, 2]);
        assert!(ranked(&names, "absent", 20).is_empty());
        let many = vec!["App".into(); 40];
        assert_eq!(ranked(&many, "", 100).len(), 20);
    }
}
/// A tiny calculator: + - * / % ^ and parentheses, "," as decimal point.
pub fn calc(input: &str) -> Option<f64> {
    if input.len() > 256 || input.chars().filter(|&c| c == '(').count() > 32 {
        return None;
    }
    let s: Vec<char> = input
        .replace(',', ".")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    // Must look like arithmetic: a digit and an operator.
    if !s.iter().any(|c| c.is_ascii_digit()) || !s.iter().any(|c| "+-*/%^(".contains(*c)) {
        return None;
    }
    let mut p = Parser { s, i: 0 };
    let v = p.expr()?;
    (p.i == p.s.len() && v.is_finite()).then_some(v)
}

struct Parser {
    s: Vec<char>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(op @ ('+' | '-')) = self.peek() {
            self.i += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        while let Some(op @ ('*' | '/' | '%')) = self.peek() {
            self.i += 1;
            let r = self.power()?;
            v = match op {
                '*' => v * r,
                '/' => v / r,
                _ => v % r,
            };
        }
        Some(v)
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.peek() == Some('^') {
            self.i += 1;
            let exp = self.power()?;
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        if self.peek() == Some('-') {
            self.i += 1;
            return Some(-self.unary()?);
        }
        self.atom()
    }

    fn atom(&mut self) -> Option<f64> {
        if self.peek() == Some('(') {
            self.i += 1;
            let v = self.expr()?;
            if self.peek() != Some(')') {
                return None;
            }
            self.i += 1;
            return Some(v);
        }
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
            self.i += 1;
        }
        self.s[start..self.i]
            .iter()
            .collect::<String>()
            .parse()
            .ok()
    }
}

/// "42", "3.5", "0.333333" — no trailing zeros, at most 8 decimals.
pub fn format_number(v: f64) -> String {
    let s = format!("{:.8}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

pub fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
/// Only an HTTP(S) template with {q} in the URL path/query/fragment is allowed.
/// The query is always encoded data; it can never replace the scheme or host.
pub fn search_url(template: &str, query: &str) -> Option<String> {
    if template.len() > 4096
        || query.len() > 1024
        || query.trim().is_empty()
        || !template.contains("{q}")
        || template
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return None;
    }
    let (scheme, rest) = template.split_once("://")?;
    let without_query = template.replace("{q}", "");
    if without_query.contains(['{', '}']) {
        return None;
    }
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains('%')
        || authority.contains('{')
        || authority.contains('}')
    {
        return None;
    }
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']')?;
        authority[1..end].parse::<std::net::Ipv6Addr>().ok()?;
        let tail = &authority[end + 1..];
        (
            &authority[..=end],
            if tail.is_empty() {
                None
            } else {
                Some(tail.strip_prefix(':')?)
            },
        )
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty()
        || (!host.starts_with('[')
            && !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
    {
        return None;
    }
    if !host.starts_with('[')
        && (host.len() > 253
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || !label.as_bytes()[0].is_ascii_alphanumeric()
                    || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            }))
    {
        return None;
    }
    if let Some(port) = port {
        if port.parse::<u16>().ok()? == 0 {
            return None;
        }
    }
    Some(template.replace("{q}", &percent_encode(query.trim())))
}
pub fn absolute_existing_path(query: &str) -> Option<PathBuf> {
    let p = Path::new(query.trim());
    if p.is_absolute() && p.exists() {
        Some(p.to_owned())
    } else {
        None
    }
}
pub fn next_selection(current: Option<usize>, delta: isize, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let current = current
        .filter(|&i| i < count)
        .map(|i| i as isize)
        .unwrap_or(if delta < 0 { 0 } else { -1 });
    Some((current + delta).rem_euclid(count as isize) as usize)
}

#[cfg(target_os = "linux")]
pub mod desktop {
    use gtk::{gio, prelude::*};
    #[derive(Clone)]
    pub struct App {
        pub info: gio::AppInfo,
        pub name: String,
        pub detail: String,
    }
    /// Read installed desktop metadata. Exec lines are never parsed or interpreted.
    pub fn installed() -> Vec<App> {
        let mut seen = std::collections::HashSet::new();
        let mut apps: Vec<_> = gio::AppInfo::all()
            .into_iter()
            .filter(|info| info.should_show())
            .filter_map(|info| {
                let name = info.display_name().to_string();
                if name.trim().is_empty() {
                    return None;
                }
                let key = info
                    .id()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{}|{}", name, info.executable().display()));
                if !seen.insert(key) {
                    return None;
                }
                let detail = info
                    .description()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "Приложение".into());
                Some(App { info, name, detail })
            })
            .take(4096)
            .collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        apps
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;
    #[test]
    fn calculator_preserves_windows_precedence_and_formats() {
        assert_eq!(calc("2+2*3"), Some(8.));
        assert_eq!(calc("(1,5 + 0,5)^3"), Some(8.));
        assert_eq!(calc("2^3^2"), Some(512.));
        assert_eq!(calc("-3*-2"), Some(6.));
        assert_eq!(calc("7%3"), Some(1.));
        assert_eq!(calc("42"), None);
        assert_eq!(calc("1/0"), None);
        assert_eq!(calc("2+"), None);
        assert_eq!(format_number(1. / 3.), "0.33333333");
        assert_eq!(format_number(-0.), "0");
        assert!(calc(&"(".repeat(1000)).is_none());
    }
    #[test]
    fn web_search_encodes_query_without_changing_authority() {
        let url = search_url("https://www.google.com/search?q={q}", "тест &x=1 #").unwrap();
        assert_eq!(
            url,
            "https://www.google.com/search?q=%D1%82%D0%B5%D1%81%D1%82%20%26x%3D1%20%23"
        );
        assert!(search_url("http://localhost:8080/?q={q}", "x").is_some());
        for template in [
            "javascript:alert('{q}')",
            "file:///tmp/{q}",
            "https://{q}/",
            "https://user@example.test/?q={q}",
            "https://example.test/search",
            "https://example.test\\evil/?q={q}",
            "https://example.test:99999/?q={q}",
            "https://-/?q={q}",
            "https://example..test/?q={q}",
            "https://example.test/?q={q}&x={other}",
        ] {
            assert!(search_url(template, "x").is_none(), "{template}");
        }
    }
    #[test]
    fn paths_require_absolute_existing_objects() {
        assert!(absolute_existing_path("relative/path").is_none());
        let path = std::env::current_exe().unwrap();
        assert_eq!(absolute_existing_path(path.to_str().unwrap()), Some(path));
        assert!(absolute_existing_path("https://example.test").is_none());
    }
    #[test]
    fn keyboard_selection_is_bounded_and_wraps() {
        assert_eq!(next_selection(None, 1, 3), Some(0));
        assert_eq!(next_selection(Some(0), -1, 3), Some(2));
        assert_eq!(next_selection(Some(2), 1, 3), Some(0));
        assert_eq!(next_selection(Some(99), 1, 3), Some(0));
        assert_eq!(next_selection(Some(0), 1, 0), None);
    }
}
