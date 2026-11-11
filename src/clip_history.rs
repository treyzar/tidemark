//! Clipboard history model: what was copied, newest first, de-duplicated,
//! capped, and saved as JSON in %APPDATA%\win-island\clipboard (images sit
//! next to it as PNG files). No Windows APIs here; see `clipboard.rs`.

use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MAX_ITEMS: usize = 50;
/// Longer texts are not kept (and not shown) to keep the file small.
pub const MAX_TEXT_CHARS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Entry {
    Text { text: String },
    /// `file` is a PNG name inside the history folder.
    Image { file: String, width: u32, height: u32 },
    Files { paths: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub entry: Entry,
    /// Unix seconds.
    pub copied_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Text,
    Link,
    Image,
    Files,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub items: Vec<Item>,
}

pub fn dir() -> PathBuf {
    Config::dir().join("clipboard")
}

fn file() -> PathBuf {
    dir().join("history.json")
}

impl History {
    pub fn load() -> Self {
        std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(dir());
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(file(), text);
        }
    }

    /// Adds `entry` on top. Copying something already in the history moves
    /// it up instead of duplicating it. Returns the PNG files that fell off
    /// the end, so the caller can delete them.
    pub fn push(&mut self, entry: Entry, now: u64) -> Vec<String> {
        if let Entry::Text { text } = &entry {
            if text.trim().is_empty() || text.chars().count() > MAX_TEXT_CHARS {
                return Vec::new();
            }
        }
        self.items.retain(|i| i.entry != entry);
        self.items.insert(0, Item { entry, copied_at: now });
        let dropped: Vec<Item> = self.items.drain(self.items.len().min(MAX_ITEMS)..).collect();
        dropped
            .into_iter()
            .filter_map(|i| match i.entry {
                Entry::Image { file, .. } => Some(file),
                _ => None,
            })
            .collect()
    }

    pub fn clear(&mut self) -> Vec<String> {
        let files = self
            .items
            .drain(..)
            .filter_map(|i| match i.entry {
                Entry::Image { file, .. } => Some(file),
                _ => None,
            })
            .collect();
        files
    }
}

/// "1 файл", "3 файла", "11 файлов".
pub fn files_word(n: usize) -> &'static str {
    let (d10, d100) = (n % 10, n % 100);
    if d10 == 1 && d100 != 11 {
        "файл"
    } else if (2..=4).contains(&d10) && !(12..=14).contains(&d100) {
        "файла"
    } else {
        "файлов"
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(path)
}

/// One-line label and glyph for the history list.
pub fn label(entry: &Entry) -> (Glyph, String) {
    match entry {
        Entry::Text { text } => {
            let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
            let short: String = first.chars().take(120).collect();
            let is_link = first.starts_with("http://") || first.starts_with("https://");
            (if is_link { Glyph::Link } else { Glyph::Text }, short)
        }
        Entry::Image { width, height, .. } => (Glyph::Image, format!("Изображение {width}×{height}")),
        Entry::Files { paths } => match paths.as_slice() {
            [one] => (Glyph::Files, file_name(one).to_string()),
            many => (Glyph::Files, format!("{} {}: {}", many.len(), files_word(many.len()), file_name(&many[0]))),
        },
    }
}

/// "сейчас", "5 мин", "2 ч", "3 дн".
pub fn ago(copied_at: u64, now: u64) -> String {
    let s = now.saturating_sub(copied_at);
    match s {
        0..=59 => "сейчас".into(),
        60..=3599 => format!("{} мин", s / 60),
        3600..=86_399 => format!("{} ч", s / 3600),
        _ => format!("{} дн", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Entry {
        Entry::Text { text: t.into() }
    }

    #[test]
    fn newest_first_and_no_duplicates() {
        let mut h = History::default();
        h.push(text("a"), 1);
        h.push(text("b"), 2);
        h.push(text("a"), 3);
        let order: Vec<_> = h.items.iter().map(|i| i.entry.clone()).collect();
        assert_eq!(order, vec![text("a"), text("b")]);
        assert_eq!(h.items[0].copied_at, 3);
    }

    #[test]
    fn caps_size_and_reports_dropped_images() {
        let mut h = History::default();
        h.push(Entry::Image { file: "old.png".into(), width: 1, height: 1 }, 0);
        for i in 0..MAX_ITEMS {
            let dropped = h.push(text(&i.to_string()), i as u64 + 1);
            if i == MAX_ITEMS - 1 {
                assert_eq!(dropped, vec!["old.png".to_string()]);
            }
        }
        assert_eq!(h.items.len(), MAX_ITEMS);
    }

    #[test]
    fn skips_blank_and_huge_text() {
        let mut h = History::default();
        h.push(text("   \n"), 1);
        h.push(text(&"x".repeat(MAX_TEXT_CHARS + 1)), 1);
        assert!(h.items.is_empty());
    }

    #[test]
    fn labels() {
        assert_eq!(label(&text("\n  Адрес: ул. Лесная, 12\nвторая")), (Glyph::Text, "Адрес: ул. Лесная, 12".into()));
        assert_eq!(label(&text("https://figma.com/file/island")).0, Glyph::Link);
        assert_eq!(
            label(&Entry::Image { file: "x.png".into(), width: 640, height: 400 }),
            (Glyph::Image, "Изображение 640×400".into())
        );
        let files = Entry::Files { paths: vec!["C:\\a\\Отчёт.docx".into(), "C:\\a\\b.txt".into(), "C:\\a\\c".into()] };
        assert_eq!(label(&files), (Glyph::Files, "3 файла: Отчёт.docx".into()));
        assert_eq!(label(&Entry::Files { paths: vec!["C:\\x\\dir\\".into()] }).1, "dir");
    }

    #[test]
    fn russian_plural() {
        let w: Vec<_> = [1, 2, 5, 11, 12, 21, 22, 25, 101, 111].iter().map(|&n| files_word(n)).collect();
        assert_eq!(w, ["файл", "файла", "файлов", "файлов", "файлов", "файл", "файла", "файлов", "файл", "файлов"]);
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(100, 130), "сейчас");
        assert_eq!(ago(0, 300), "5 мин");
        assert_eq!(ago(0, 7300), "2 ч");
        assert_eq!(ago(0, 3 * 86_400 + 5), "3 дн");
    }

    #[test]
    fn json_roundtrip() {
        let mut h = History::default();
        h.push(Entry::Files { paths: vec!["C:\\x".into()] }, 5);
        h.push(text("hi"), 6);
        let json = serde_json::to_string(&h).unwrap();
        assert!(json.contains("\"kind\":\"text\""));
        let back: History = serde_json::from_str(&json).unwrap();
        assert_eq!(back.items, h.items);
    }
}
