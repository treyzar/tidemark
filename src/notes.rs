//! The scratch note and reminders, saved in %APPDATA%\win-island.
//! Reminders are typed as plain Russian phrases:
//! "через 15 мин купить хлеб", "в 18:30 позвонить маме",
//! "завтра в 9 созвон", "через 2 ч забрать посылку".

use crate::config::Config;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    /// Unix seconds.
    pub at: u64,
    pub text: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub note: String,
    pub reminders: Vec<Reminder>,
}

fn path() -> std::path::PathBuf {
    Config::dir().join("notes.json")
}

impl Store {
    pub fn load() -> Self {
        std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(Config::dir());
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), text);
        }
    }

    pub fn add(&mut self, r: Reminder) {
        let i = self.reminders.partition_point(|x| x.at <= r.at);
        self.reminders.insert(i, r);
    }

    /// Reminders due at `now`, removed from the list.
    pub fn take_due(&mut self, now: u64) -> Vec<Reminder> {
        let n = self.reminders.partition_point(|r| r.at <= now);
        self.reminders.drain(..n).collect()
    }

    pub fn next_due(&self) -> Option<u64> {
        self.reminders.first().map(|r| r.at)
    }
}

fn unit_seconds(word: &str) -> Option<u64> {
    let w = word.trim_end_matches('.');
    Some(match w {
        "с" | "сек" | "секунд" | "секунду" | "секунды" => 1,
        "м" | "мин" | "минут" | "минуту" | "минуты" => 60,
        "ч" | "час" | "часа" | "часов" => 3600,
        "д" | "дн" | "день" | "дня" | "дней" => 86_400,
        _ => return None,
    })
}

/// "18:30", "18.30", "9" -> seconds since midnight.
fn clock_time(word: &str) -> Option<u64> {
    let (h, m) = match word.split_once([':', '.']) {
        Some((h, m)) => (h.parse::<u64>().ok()?, m.parse::<u64>().ok()?),
        None => (word.parse::<u64>().ok()?, 0),
    };
    (h < 24 && m < 60).then_some(h * 3600 + m * 60)
}

/// Parses a reminder phrase. `now` is unix seconds, `since_midnight` the
/// local seconds since midnight at `now`. Returns (due unix time, text).
pub fn parse(input: &str, now: u64, since_midnight: u64) -> Option<(u64, String)> {
    let words: Vec<&str> = input.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let rest = |from: usize| words[from.min(words.len())..].join(" ");
    let midnight = now - since_midnight;

    match lower.first().map(String::as_str) {
        // "через 15 мин ...", "через час ...", "через 1.5 ч ..."
        Some("через") => {
            let (amount, unit, used) = match (lower.get(1), lower.get(2)) {
                (Some(n), Some(u)) if n.replace(',', ".").parse::<f64>().is_ok() => {
                    (n.replace(',', ".").parse::<f64>().ok()?, unit_seconds(u)?, 3)
                }
                (Some(u), _) => (1.0, unit_seconds(u)?, 2),
                _ => return None,
            };
            let secs = (amount * unit as f64).round() as u64;
            (secs > 0).then(|| (now + secs, rest(used)))
        }
        // "завтра в 9 ...", "завтра 9:30 ..."
        Some("завтра") => {
            let i = if lower.get(1).map(String::as_str) == Some("в") { 2 } else { 1 };
            let t = clock_time(lower.get(i)?)?;
            Some((midnight + 86_400 + t, rest(i + 1)))
        }
        // "в 18:30 ...", "18:30 ..."
        Some(first) => {
            let i = if first == "в" { 1 } else { 0 };
            let word = lower.get(i)?;
            if i == 0 && !word.contains([':', '.']) {
                return None; // A bare number is not a time.
            }
            let t = clock_time(word)?;
            let mut due = midnight + t;
            if due <= now {
                due += 86_400; // Already past today: tomorrow.
            }
            Some((due, rest(i + 1)))
        }
        None => None,
    }
    .map(|(at, text)| (at, if text.is_empty() { "Напоминание".to_string() } else { text }))
}

/// "в 18:30", "завтра в 9:00", "через 5 мин" for the list.
pub fn when_label(at: u64, now: u64, since_midnight: u64) -> String {
    let midnight = now - since_midnight;
    let left = at.saturating_sub(now);
    if left < 3600 {
        return format!("через {} мин", left.div_ceil(60).max(1));
    }
    let day = (at.saturating_sub(midnight)) / 86_400;
    let t = (at.saturating_sub(midnight)) % 86_400;
    let hm = format!("{}:{:02}", t / 3600, t / 60 % 60);
    match day {
        0 => format!("в {hm}"),
        1 => format!("завтра в {hm}"),
        d => format!("через {d} дн в {hm}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-11 14:00 local; midnight at 1_000_000.
    const MIDNIGHT: u64 = 1_000_000;
    const NOW: u64 = MIDNIGHT + 14 * 3600;
    const SINCE: u64 = 14 * 3600;

    #[test]
    fn relative_phrases() {
        assert_eq!(parse("через 15 мин купить хлеб", NOW, SINCE), Some((NOW + 900, "купить хлеб".into())));
        assert_eq!(parse("Через час созвон", NOW, SINCE), Some((NOW + 3600, "созвон".into())));
        assert_eq!(parse("через 1,5 ч выйти", NOW, SINCE), Some((NOW + 5400, "выйти".into())));
        assert_eq!(parse("через 2 дня оплатить", NOW, SINCE), Some((NOW + 2 * 86_400, "оплатить".into())));
        assert_eq!(parse("через 10 сек", NOW, SINCE), Some((NOW + 10, "Напоминание".into())));
        assert_eq!(parse("через бублик", NOW, SINCE), None);
    }

    #[test]
    fn clock_phrases() {
        assert_eq!(parse("в 18:30 позвонить маме", NOW, SINCE), Some((MIDNIGHT + 18 * 3600 + 1800, "позвонить маме".into())));
        // 9:00 already passed today -> tomorrow.
        assert_eq!(parse("в 9 зарядка", NOW, SINCE), Some((MIDNIGHT + 86_400 + 9 * 3600, "зарядка".into())));
        assert_eq!(parse("15.45 чай", NOW, SINCE), Some((MIDNIGHT + 15 * 3600 + 45 * 60, "чай".into())));
        assert_eq!(parse("завтра в 9:30 созвон", NOW, SINCE), Some((MIDNIGHT + 86_400 + 9 * 3600 + 1800, "созвон".into())));
        assert_eq!(parse("в 25:00 x", NOW, SINCE), None);
        assert_eq!(parse("5 яблок", NOW, SINCE), None);
        assert_eq!(parse("купить хлеб", NOW, SINCE), None);
    }

    #[test]
    fn store_keeps_order_and_takes_due() {
        let mut s = Store::default();
        s.add(Reminder { at: 30, text: "c".into() });
        s.add(Reminder { at: 10, text: "a".into() });
        s.add(Reminder { at: 20, text: "b".into() });
        assert_eq!(s.next_due(), Some(10));
        let due: Vec<_> = s.take_due(20).into_iter().map(|r| r.text).collect();
        assert_eq!(due, ["a", "b"]);
        assert_eq!(s.next_due(), Some(30));
    }

    #[test]
    fn when_labels() {
        assert_eq!(when_label(NOW + 300, NOW, SINCE), "через 5 мин");
        assert_eq!(when_label(MIDNIGHT + 18 * 3600 + 1800, NOW, SINCE), "в 18:30");
        assert_eq!(when_label(MIDNIGHT + 86_400 + 9 * 3600, NOW, SINCE), "завтра в 9:00");
    }
}
