//! Clipboard history model: pinned first, newest within each group, de-duplicated,
//! capped, and saved as JSON in `<data dir>/clipboard` (images sit next to it
//! as PNG files; see `paths`). No OS APIs here; the Windows reader is
//! `clipboard.rs` in the app crate.

use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    #[serde(default)]
    pub pinned: bool,
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
    Config::data_dir().join("clipboard")
}

const FILE: &str = "history.json";

impl History {
    pub fn load() -> Self {
        Self::load_from(&dir())
    }

    /// Loads `history.json` from the history folder `dir`.
    pub fn load_from(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(FILE)).map(|text| Self::from_json(&text)).unwrap_or_default()
    }

    fn from_json(text: &str) -> Self {
        let mut history: Self = serde_json::from_str(text).unwrap_or_default();
        history.normalize();
        history
    }

    pub fn save(&self) {
        let _ = self.save_to(&dir());
    }

    pub fn save_to(&self, dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        crate::paths::write_atomic(&dir.join(FILE), text.as_bytes())
    }

    /// Adds `entry` as newest in its group. Copying an existing entry preserves
    /// its pin. Returns discarded PNG names for deletion, including the new
    /// image if every slot is pinned and it cannot be kept.
    pub fn push(&mut self, entry: Entry, now: u64) -> Vec<String> {
        if let Entry::Text { text } = &entry {
            if text.trim().is_empty() || text.chars().count() > MAX_TEXT_CHARS {
                return Vec::new();
            }
        }
        let pinned = self.items.iter().any(|i| i.entry == entry && i.pinned);
        self.items.retain(|i| i.entry != entry);
        self.items.insert(0, Item { entry, copied_at: now, pinned });
        self.normalize()
    }

    /// Restores ordering and caps malformed/oversized saved histories. Pinned
    /// entries take priority; if there are over 50 pins, keep the newest 50.
    fn normalize(&mut self) -> Vec<String> {
        self.sort_items();
        let dropped: Vec<Item> = self.items.drain(self.items.len().min(MAX_ITEMS)..).collect();
        dropped
            .into_iter()
            .filter_map(|i| match i.entry {
                Entry::Image { file, .. } => Some(file),
                _ => None,
            })
            .collect()
    }

    fn sort_items(&mut self) {
        self.items.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.copied_at.cmp(&a.copied_at)));
    }

    /// Toggles the row's pin and reorders the list without changing copy time.
    /// Returns true on success, or false for an out-of-range index.
    pub fn toggle_pin(&mut self, index: usize) -> bool {
        let Some(item) = self.items.get_mut(index) else { return false };
        item.pinned = !item.pinned;
        self.sort_items();
        true
    }

    /// Clears unpinned entries, returning only their PNG names for deletion.
    pub fn clear(&mut self) -> Vec<String> {
        let mut files = Vec::new();
        self.items.retain(|i| {
            if !i.pinned {
                if let Entry::Image { file, .. } = &i.entry {
                    files.push(file.clone());
                }
            }
            i.pinned
        });
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
            [] => (Glyph::Files, "0 файлов".into()),
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
        assert!(h.toggle_pin(1));
        let json = serde_json::to_string(&h).unwrap();
        assert!(json.contains("\"kind\":\"text\""));
        let back: History = serde_json::from_str(&json).unwrap();
        assert_eq!(back.items, h.items);
    }

    #[test]
    fn old_json_defaults_to_unpinned() {
        let h: History = serde_json::from_str(
            r#"{"items":[{"entry":{"kind":"text","text":"legacy"},"copied_at":1}]}"#,
        ).unwrap();
        let saved = serde_json::to_value(&h).unwrap();
        assert_eq!(saved["items"][0]["pinned"], false);
    }

    #[test]
    fn empty_file_list_has_a_label() {
        assert_eq!(label(&Entry::Files { paths: vec![] }), (Glyph::Files, "0 файлов".into()));
    }

    #[test]
    fn pinned_item_survives_more_than_capacity_pushes() {
        let mut h = History::default();
        h.push(text("keep"), 0);
        h.items[0].pinned = true;
        for i in 1..=100 {
            h.push(text(&format!("new {i}")), i);
        }
        assert_eq!(h.items.len(), 50);
        assert_eq!(h.items[0].entry, text("keep"));
        assert!(h.items[0].pinned);
        assert_eq!(h.items[1].entry, text("new 100"));
        assert_eq!(h.items[49].entry, text("new 52"));
    }

    #[test]
    fn duplicate_push_preserves_pin_and_updates_group_order() {
        let mut h = History::default();
        h.push(text("a"), 1);
        h.items[0].pinned = true;
        h.push(text("b"), 2);
        h.items.iter_mut().find(|i| i.entry == text("b")).unwrap().pinned = true;
        h.push(text("normal"), 3);
        h.push(text("a"), 4);
        assert_eq!(h.items.iter().map(|i| i.entry.clone()).collect::<Vec<_>>(),
            vec![text("a"), text("b"), text("normal")]);
        assert!(h.items[0].pinned);
        assert!(h.items[1].pinned);
        assert_eq!(h.items[0].copied_at, 4);
    }

    #[test]
    fn full_pinned_history_reports_rejected_image() {
        let mut h = History::default();
        for i in 0..50 {
            h.items.push(Item { entry: text(&i.to_string()), copied_at: i, pinned: true });
        }
        let dropped = h.push(Entry::Image { file: "rejected.png".into(), width: 1, height: 1 }, 100);
        assert_eq!(dropped, vec!["rejected.png"]);
        assert_eq!(h.items.len(), 50);
        assert!(h.items.iter().all(|i| i.pinned));
    }

    #[test]
    fn clear_keeps_pins_and_reports_only_unpinned_images() {
        let mut h = History::default();
        h.push(Entry::Image { file: "keep.png".into(), width: 1, height: 1 }, 1);
        h.items[0].pinned = true;
        h.push(Entry::Image { file: "drop.png".into(), width: 1, height: 1 }, 2);
        h.push(text("drop text"), 3);
        assert_eq!(h.clear(), vec!["drop.png"]);
        assert_eq!(h.items.len(), 1);
        assert!(h.items[0].pinned);
        assert_eq!(h.items[0].entry, Entry::Image { file: "keep.png".into(), width: 1, height: 1 });
    }

    #[test]
    fn toggling_pin_sorts_each_group_by_original_copy_time() {
        let mut h = History::default();
        h.push(text("a"), 1);
        h.push(text("b"), 2);
        h.push(text("c"), 3);
        assert!(h.toggle_pin(2));
        assert_eq!(h.items.iter().map(|i| i.entry.clone()).collect::<Vec<_>>(),
            vec![text("a"), text("c"), text("b")]);
        assert!(h.toggle_pin(2));
        assert_eq!(h.items.iter().map(|i| i.entry.clone()).collect::<Vec<_>>(),
            vec![text("b"), text("a"), text("c")]);
        assert!(h.toggle_pin(1));
        assert_eq!(h.items.iter().map(|i| i.entry.clone()).collect::<Vec<_>>(),
            vec![text("b"), text("c"), text("a")]);
        assert_eq!(h.items.iter().map(|i| i.pinned).collect::<Vec<_>>(), vec![true, false, false]);
        assert_eq!(h.items[2].copied_at, 1);
    }

    #[test]
    fn invalid_pin_index_is_harmless() {
        let mut h = History::default();
        assert!(!h.toggle_pin(0));
        h.push(text("keep"), 1);
        let before = h.items.clone();
        assert!(!h.toggle_pin(1));
        assert!(!h.toggle_pin(usize::MAX));
        assert_eq!(h.items, before);
    }

    #[test]
    fn loading_oversized_history_keeps_pins_and_newest_normal_items() {
        let mut saved = History::default();
        for i in 1..=60 {
            saved.items.push(Item { entry: text(&format!("normal {i}")), copied_at: i, pinned: false });
        }
        saved.items.push(Item { entry: text("old pin"), copied_at: 0, pinned: true });
        saved.items.push(Item { entry: text("new pin"), copied_at: 5, pinned: true });
        let loaded = History::from_json(&serde_json::to_string(&saved).unwrap());
        assert_eq!(loaded.items.len(), 50);
        assert_eq!(loaded.items[0].entry, text("new pin"));
        assert_eq!(loaded.items[1].entry, text("old pin"));
        assert_eq!(loaded.items[2].entry, text("normal 60"));
        assert_eq!(loaded.items[49].entry, text("normal 13"));
    }

    #[test]
    fn loading_more_than_capacity_pins_keeps_newest_pins() {
        let mut saved = History::default();
        for i in 0..55 {
            saved.items.push(Item { entry: text(&i.to_string()), copied_at: i, pinned: true });
        }
        saved.items.push(Item { entry: text("normal"), copied_at: 100, pinned: false });
        let loaded = History::from_json(&serde_json::to_string(&saved).unwrap());
        assert_eq!(loaded.items.len(), 50);
        assert!(loaded.items.iter().all(|i| i.pinned));
        assert_eq!(loaded.items[0].entry, text("54"));
        assert_eq!(loaded.items[49].entry, text("5"));
    }

    #[test]
    fn history_round_trip_in_temp_dir() {
        let tmp = crate::paths::test_dir::TempDir::new("clip");
        let mut h = History::default();
        h.push(text("a"), 1);
        h.push(Entry::Files { paths: vec!["/home/u/x.txt".into()] }, 2);
        h.toggle_pin(1);
        h.save_to(tmp.path()).unwrap();
        let back = History::load_from(tmp.path());
        assert_eq!(back.items, h.items);
    }
}
