//! Time-synced lyrics from LRCLIB (free, no key). One request per track,
//! cached on disk (including "not found", re-checked after a few days), on
//! a background thread that sleeps until the UI asks for a new track.

use crate::config::Config;
use crate::http;
use serde::Deserialize;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_LYRICS: u32 = WM_APP + 4;
const NOT_FOUND_TTL: Duration = Duration::from_secs(3 * 24 * 3600);
const NOT_FOUND_MARK: &str = "#not-found";

/// One timed line; `text` is empty for instrumental gaps.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub at: Duration,
    pub text: String,
}

pub type Lines = Arc<Vec<Line>>;

/// Parses LRC: `[mm:ss.xx] text`, several stamps per line allowed,
/// metadata tags (`[ar:...]`) ignored. Sorted by time.
pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            rest = &stripped[end + 1..];
            if let Some(t) = parse_stamp(tag) {
                stamps.push(t);
            }
        }
        let line = rest.trim();
        for at in stamps {
            out.push(Line { at, text: line.to_string() });
        }
    }
    out.sort_by_key(|l| l.at);
    out
}

/// "01:02.5", "01:02.53", "01:02.530", "01:02".
fn parse_stamp(tag: &str) -> Option<Duration> {
    let (m, s) = tag.split_once(':')?;
    let minutes: u64 = m.trim().parse().ok()?;
    let (sec, frac) = s.split_once('.').unwrap_or((s, ""));
    let seconds: u64 = sec.trim().parse().ok()?;
    if seconds >= 60 {
        return None;
    }
    let millis = match frac.len() {
        0 => 0,
        1 => frac.parse::<u64>().ok()? * 100,
        2 => frac.parse::<u64>().ok()? * 10,
        _ => frac[..3].parse::<u64>().ok()?,
    };
    Some(Duration::from_millis((minutes * 60 + seconds) * 1000 + millis))
}

/// Index of the line being sung at `pos`.
pub fn line_at(lines: &[Line], pos: Duration) -> Option<usize> {
    let n = lines.partition_point(|l| l.at <= pos);
    n.checked_sub(1)
}

/// Time from `pos` until the next line starts.
pub fn until_next(lines: &[Line], pos: Duration) -> Option<Duration> {
    let n = lines.partition_point(|l| l.at <= pos);
    lines.get(n).map(|l| l.at - pos)
}

/// Drops decorations that stop titles from matching: "(Official Video)",
/// "[Lyrics]", "- Slowed", "(feat. X)".
pub fn clean_title(title: &str) -> String {
    let mut s = title.to_string();
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let (Some(a), Some(b)) = (s.find(open), s.find(close)) {
            if b <= a {
                break;
            }
            s.replace_range(a..=b, "");
        }
    }
    if let Some((head, _)) = s.split_once(" - ") {
        s = head.to_string();
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Deserialize)]
struct Record {
    #[serde(rename = "syncedLyrics")]
    synced: Option<String>,
    duration: Option<f64>,
}

fn query(params: &[(&str, &str)]) -> String {
    params.iter().map(|(k, v)| format!("{k}={}", http::encode(v))).collect::<Vec<_>>().join("&")
}

/// Picks a synced record whose duration is within 4 s (when known).
pub fn pick_synced(json: &str, duration: Duration) -> Option<String> {
    let records: Vec<Record> = serde_json::from_str(json).ok()?;
    let close = |r: &&Record| {
        duration.is_zero() || r.duration.is_none_or(|d| (d - duration.as_secs_f64()).abs() <= 4.0)
    };
    records.iter().filter(|r| r.synced.as_ref().is_some_and(|s| !s.is_empty())).find(close)?.synced.clone()
}

fn fetch(artist: &str, title: &str, duration: Duration) -> Option<String> {
    let secs = duration.as_secs().to_string();
    let mut params = vec![("artist_name", artist), ("track_name", title)];
    if !duration.is_zero() {
        params.push(("duration", secs.as_str()));
    }
    if let Some(body) = http::get("lrclib.net", &format!("/api/get?{}", query(&params))) {
        let r: Option<Record> = serde_json::from_str(&body).ok();
        if let Some(s) = r.and_then(|r| r.synced).filter(|s| !s.is_empty()) {
            return Some(s);
        }
    }
    let cleaned = clean_title(title);
    let body = http::get("lrclib.net", &format!("/api/search?{}", query(&[("track_name", &cleaned), ("artist_name", artist)])))?;
    pick_synced(&body, duration)
}

fn cache_path(key: u64) -> PathBuf {
    Config::dir().join("lyrics").join(format!("{key:016x}.lrc"))
}

/// Cached lyrics: Some(Some) = have them, Some(None) = known missing.
fn from_cache(key: u64) -> Option<Option<String>> {
    let path = cache_path(key);
    let text = std::fs::read_to_string(&path).ok()?;
    if text == NOT_FOUND_MARK {
        let age = std::fs::metadata(&path).ok()?.modified().ok()?.elapsed().ok()?;
        return (age < NOT_FOUND_TTL).then_some(None);
    }
    Some(Some(text))
}

fn to_cache(key: u64, text: Option<&str>) {
    let path = cache_path(key);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text.unwrap_or(NOT_FOUND_MARK));
}

pub fn track_key(artist: &str, title: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (artist.trim().to_lowercase(), title.trim().to_lowercase()).hash(&mut h);
    h.finish()
}

struct Request {
    key: u64,
    artist: String,
    title: String,
    duration: Duration,
}

pub struct Service {
    tx: Sender<Request>,
    latest: Arc<Mutex<Option<(u64, Lines)>>>,
}

impl Service {
    pub fn start(hwnd: HWND) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Request>();
        let latest = Arc::new(Mutex::new(None));
        let shared = latest.clone();
        let hwnd_raw = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("lyrics".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let hwnd = HWND(hwnd_raw as *mut _);
                while let Ok(mut req) = rx.recv() {
                    // Skip to the newest request if tracks changed quickly.
                    while let Ok(newer) = rx.try_recv() {
                        req = newer;
                    }
                    let text = match from_cache(req.key) {
                        Some(cached) => cached,
                        None => {
                            let got = fetch(&req.artist, &req.title, req.duration);
                            to_cache(req.key, got.as_deref());
                            got
                        }
                    };
                    let lines: Lines = Arc::new(text.map(|t| parse_lrc(&t)).unwrap_or_default());
                    *shared.lock().unwrap() = Some((req.key, lines));
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_LYRICS, WPARAM(0), LPARAM(0));
                    }
                }
            })
            .ok()?;
        Some(Self { tx, latest })
    }

    pub fn request(&self, artist: &str, title: &str, duration: Duration) -> u64 {
        let key = track_key(artist, title);
        let _ = self.tx.send(Request { key, artist: artist.into(), title: title.into(), duration });
        key
    }

    pub fn get(&self) -> Option<(u64, Lines)> {
        self.latest.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LRC: &str = "[ar:Cartoon]\n[00:10.83] Hold me close\n[00:13.78]Time is barely\n[00:20.00]\n[00:05.1][01:00.123] Chorus\nno stamp here";

    #[test]
    fn parses_and_sorts() {
        let l = parse_lrc(LRC);
        let got: Vec<(u128, &str)> = l.iter().map(|l| (l.at.as_millis(), l.text.as_str())).collect();
        assert_eq!(
            got,
            vec![(5100, "Chorus"), (10830, "Hold me close"), (13780, "Time is barely"), (20000, ""), (60123, "Chorus")]
        );
    }

    #[test]
    fn current_and_next_line() {
        let l = parse_lrc(LRC);
        assert_eq!(line_at(&l, Duration::from_secs(1)), None);
        assert_eq!(line_at(&l, Duration::from_millis(10830)), Some(1));
        assert_eq!(line_at(&l, Duration::from_secs(12)), Some(1));
        assert_eq!(line_at(&l, Duration::from_secs(90)), Some(4));
        assert_eq!(until_next(&l, Duration::from_secs(12)), Some(Duration::from_millis(1780)));
        assert_eq!(until_next(&l, Duration::from_secs(90)), None);
    }

    #[test]
    fn stamps() {
        assert_eq!(parse_stamp("01:02"), Some(Duration::from_secs(62)));
        assert_eq!(parse_stamp("01:02.5"), Some(Duration::from_millis(62500)));
        assert_eq!(parse_stamp("ar"), None);
        assert_eq!(parse_stamp("00:75.00"), None);
    }

    #[test]
    fn title_cleanup() {
        assert_eq!(clean_title("It's Just a Dream - Slowed"), "It's Just a Dream");
        assert_eq!(clean_title("On & On (Official Video) [NCS Release]"), "On & On");
        assert_eq!(clean_title("Song (feat. Someone)"), "Song");
        assert_eq!(clean_title("Plain"), "Plain");
    }

    #[test]
    fn picks_record_with_matching_duration() {
        let json = r#"[{"syncedLyrics":null,"duration":200.0},{"syncedLyrics":"[00:01.00] far","duration":300.0},{"syncedLyrics":"[00:01.00] near","duration":201.5}]"#;
        assert_eq!(pick_synced(json, Duration::from_secs(200)).as_deref(), Some("[00:01.00] near"));
        assert_eq!(pick_synced(json, Duration::ZERO).as_deref(), Some("[00:01.00] far"));
        assert_eq!(pick_synced("[]", Duration::from_secs(200)), None);
    }
}
