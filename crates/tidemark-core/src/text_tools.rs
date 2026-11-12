//! Text transforms for the Text tab: case, layout fix, transliteration,
//! whitespace cleanup, JSON and Base64. All pure functions.

use crate::keyboard::swap_layout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Layout,
    Upper,
    Lower,
    Title,
    Sentence,
    Translit,
    JoinLines,
    Spaces,
    JsonPretty,
    JsonCompact,
    ToBase64,
    FromBase64,
}

pub const OPS: [Op; 12] = [
    Op::Layout,
    Op::Upper,
    Op::Lower,
    Op::Title,
    Op::Sentence,
    Op::Translit,
    Op::JoinLines,
    Op::Spaces,
    Op::JsonPretty,
    Op::JsonCompact,
    Op::ToBase64,
    Op::FromBase64,
];

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Op::Layout => "Раскладка",
            Op::Upper => "ВЕРХНИЙ",
            Op::Lower => "нижний",
            Op::Title => "Каждое Слово",
            Op::Sentence => "Предложение",
            Op::Translit => "Транслит",
            Op::JoinLines => "Убрать переносы",
            Op::Spaces => "Лишние пробелы",
            Op::JsonPretty => "JSON красиво",
            Op::JsonCompact => "JSON в строку",
            Op::ToBase64 => "→ Base64",
            Op::FromBase64 => "Base64 →",
        }
    }
}

/// Applies `op`; None when it does not apply (invalid JSON or Base64).
pub fn apply(op: Op, s: &str) -> Option<String> {
    Some(match op {
        Op::Layout => swap_layout_keep_case(s),
        Op::Upper => s.to_uppercase(),
        Op::Lower => s.to_lowercase(),
        Op::Title => title_case(s),
        Op::Sentence => sentence_case(s),
        Op::Translit => translit(s),
        Op::JoinLines => join_lines(s),
        Op::Spaces => squeeze_spaces(s),
        Op::JsonPretty => serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(s.trim()).ok()?).ok()?,
        Op::JsonCompact => serde_json::to_string(&serde_json::from_str::<serde_json::Value>(s.trim()).ok()?).ok()?,
        Op::ToBase64 => base64_encode(s.as_bytes()),
        Op::FromBase64 => String::from_utf8(base64_decode(s.trim())?).ok()?,
    })
}

fn swap_layout_keep_case(s: &str) -> String {
    s.chars()
        .map(|c| {
            let swapped = swap_layout(&c.to_string()).chars().next().unwrap_or(c);
            if c.is_uppercase() { swapped.to_uppercase().next().unwrap_or(swapped) } else { swapped }
        })
        .collect()
}

fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = c.is_whitespace() || c == '-';
        }
    }
    out
}

fn sentence_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if c.is_alphabetic() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            if matches!(c, '.' | '!' | '?' | '\n') {
                start = true;
            }
        }
    }
    out
}

/// Russian -> Latin (passport-style, ГОСТ 52535.1-2006 simplified).
pub fn translit(s: &str) -> String {
    let map = |c: char| -> Option<&'static str> {
        Some(match c {
            'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' => "e", 'ё' => "e", 'ж' => "zh",
            'з' => "z", 'и' => "i", 'й' => "i", 'к' => "k", 'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o",
            'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t", 'у' => "u", 'ф' => "f", 'х' => "kh", 'ц' => "ts",
            'ч' => "ch", 'ш' => "sh", 'щ' => "shch", 'ъ' => "ie", 'ы' => "y", 'ь' => "", 'э' => "e", 'ю' => "iu",
            'я' => "ia",
            _ => return None,
        })
    };
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let lower = c.to_lowercase().next().unwrap_or(c);
        match map(lower) {
            Some(t) if c.is_uppercase() => {
                let mut it = t.chars();
                if let Some(f) = it.next() {
                    out.extend(f.to_uppercase());
                    out.push_str(it.as_str());
                }
            }
            Some(t) => out.push_str(t),
            None => out.push(c),
        }
    }
    out
}

/// Joins wrapped lines into paragraphs: blank lines stay paragraph breaks,
/// hyphenated line ends ("пере-\nнос") are glued back.
pub fn join_lines(s: &str) -> String {
    let mut out = String::new();
    for para in s.replace("\r\n", "\n").split("\n\n") {
        let mut p = String::new();
        for line in para.lines().map(str::trim) {
            if line.is_empty() {
                continue;
            }
            if p.ends_with('-') && line.chars().next().is_some_and(|c| c.is_lowercase()) {
                p.pop();
                p.push_str(line);
            } else {
                if !p.is_empty() {
                    p.push(' ');
                }
                p.push_str(line);
            }
        }
        if !p.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&p);
        }
    }
    out
}

/// Collapses runs of spaces/tabs, trims every line, drops extra blank lines.
pub fn squeeze_spaces(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in s.replace("\r\n", "\n").lines() {
        let squeezed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if squeezed.is_empty() && lines.last().is_some_and(|l| l.is_empty()) {
            continue;
        }
        lines.push(squeezed);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Accepts standard and URL-safe alphabets, with or without padding.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'=').collect();
    if clean.is_empty() || clean.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// "12 симв · 3 слова · 2 стр" for the Text tab footer.
pub fn stats(s: &str) -> String {
    let chars = s.chars().count();
    let words = s.split_whitespace().count();
    let lines = if s.is_empty() { 0 } else { s.lines().count().max(1) };
    let plural = |n: usize, one: &'static str, few: &'static str, many: &'static str| {
        let (d10, d100) = (n % 10, n % 100);
        if d10 == 1 && d100 != 11 {
            one
        } else if (2..=4).contains(&d10) && !(12..=14).contains(&d100) {
            few
        } else {
            many
        }
    };
    format!("{chars} симв · {words} {} · {lines} стр", plural(words, "слово", "слова", "слов"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases() {
        assert_eq!(apply(Op::Upper, "Привет, world").unwrap(), "ПРИВЕТ, WORLD");
        assert_eq!(apply(Op::Title, "встреча в пятницу-утро").unwrap(), "Встреча В Пятницу-Утро");
        assert_eq!(apply(Op::Sentence, "ПРИВЕТ. КАК ДЕЛА? ok").unwrap(), "Привет. Как дела? Ok");
    }

    #[test]
    fn layout_keeps_case() {
        assert_eq!(apply(Op::Layout, "Ghbdtn").unwrap(), "Привет");
        assert_eq!(apply(Op::Layout, "Руддщ").unwrap(), "Hello");
    }

    #[test]
    fn transliteration() {
        assert_eq!(translit("Щука и Ёж, Юля"), "Shchuka i Ezh, Iulia");
        assert_eq!(translit("Чебоксары"), "Cheboksary");
    }

    #[test]
    fn whitespace() {
        assert_eq!(join_lines("Встреча перене-\nсена на\nпятницу.\n\nВторой абзац\nтут"), "Встреча перенесена на пятницу.\n\nВторой абзац тут");
        assert_eq!(squeeze_spaces("  a   b \t c  \n\n\n  d  \n\n"), "a b c\n\nd");
    }

    #[test]
    fn json() {
        assert_eq!(apply(Op::JsonCompact, "{ \"a\" : [1, 2] }").unwrap(), "{\"a\":[1,2]}");
        assert!(apply(Op::JsonPretty, "{\"a\":1}").unwrap().contains("\n"));
        assert_eq!(apply(Op::JsonPretty, "не json"), None);
    }

    #[test]
    fn base64_roundtrip() {
        for s in ["", "f", "fo", "foo", "foob", "Привет, мир!"] {
            let enc = base64_encode(s.as_bytes());
            if s.is_empty() {
                assert_eq!(enc, "");
                continue;
            }
            assert_eq!(String::from_utf8(base64_decode(&enc).unwrap()).unwrap(), s);
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_decode("Zm9vYg"), Some(b"foob".to_vec())); // no padding
        assert_eq!(base64_decode("Zm9v!"), None);
        assert_eq!(apply(Op::FromBase64, "0J/RgNC40LLQtdGC"), Some("Привет".into()));
    }

    #[test]
    fn footer_stats() {
        assert_eq!(stats("Всем привет!\nВстречу перенесли"), "30 симв · 4 слова · 2 стр");
        assert_eq!(stats(""), "0 симв · 0 слов · 0 стр");
        assert_eq!(stats("одно"), "4 симв · 1 слово · 1 стр");
    }
}
