use crate::{transport::encode, Client, Error, Result};
use serde::Deserialize;
use std::time::Duration;
/// One timed line; `text` is empty for instrumental gaps.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub at: Duration,
    pub text: String,
}

pub type Lines = Vec<Line>;

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
            out.push(Line {
                at,
                text: line.to_string(),
            });
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
    // Lyrics come from a remote server: never slice untrusted text at a
    // byte offset that may not be a char boundary.
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
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
    Some(Duration::from_millis(
        minutes
            .checked_mul(60)?
            .checked_add(seconds)?
            .checked_mul(1000)?
            .checked_add(millis)?,
    ))
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

/// Only metadata of the track explicitly requested by the caller is sent.
#[derive(Debug, Clone)]
pub struct TrackQuery {
    pub artist: String,
    pub title: String,
    pub duration: Duration,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Lyrics {
    pub synced: Vec<Line>,
    pub plain: Option<String>,
    pub instrumental: bool,
}
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "syncedLyrics")]
    synced: Option<String>,
    #[serde(rename = "plainLyrics")]
    plain: Option<String>,
    #[serde(default)]
    instrumental: bool,
    duration: Option<f64>,
}
impl Record {
    fn into_lyrics(self) -> Option<Lyrics> {
        let synced = self.synced.as_deref().map(parse_lrc).unwrap_or_default();
        let plain = self.plain.filter(|s| !s.trim().is_empty());
        (!synced.is_empty() || plain.is_some() || self.instrumental).then_some(Lyrics {
            synced,
            plain,
            instrumental: self.instrumental,
        })
    }
    fn close(&self, duration: Duration) -> bool {
        duration.is_zero()
            || self
                .duration
                .is_none_or(|d| d.is_finite() && (d - duration.as_secs_f64()).abs() <= 4.0)
    }
}
pub fn parse_search(json: &str, duration: Duration) -> Result<Option<Lyrics>> {
    let records: Vec<Record> = serde_json::from_str(json).map_err(|_| Error::InvalidResponse)?;
    let mut plain = None;
    for record in records.into_iter().filter(|r| r.close(duration)) {
        if let Some(lyrics) = record.into_lyrics() {
            if !lyrics.synced.is_empty() || lyrics.instrumental {
                return Ok(Some(lyrics));
            }
            if plain.is_none() {
                plain = Some(lyrics);
            }
        }
    }
    Ok(plain)
}
impl Client {
    pub fn lyrics(&self, query: &TrackQuery) -> Result<Option<Lyrics>> {
        let artist = query.artist.trim();
        let title = query.title.trim();
        if title.is_empty() || artist.len() > 1024 || title.len() > 1024 {
            return Err(Error::InvalidInput);
        }
        let mut url = format!(
            "https://lrclib.net/api/get?artist_name={}&track_name={}",
            encode(artist),
            encode(title)
        );
        if !query.duration.is_zero() {
            url.push_str(&format!("&duration={}", query.duration.as_secs()));
        }
        let mut exact_plain = None;
        match self.get(&url) {
            Ok(raw) => {
                let record: Record =
                    serde_json::from_str(&raw).map_err(|_| Error::InvalidResponse)?;
                if record.close(query.duration) {
                    if let Some(lyrics) = record.into_lyrics() {
                        if !lyrics.synced.is_empty() || lyrics.instrumental {
                            return Ok(Some(lyrics));
                        }
                        exact_plain = Some(lyrics);
                    }
                }
            }
            Err(Error::NotFound) => (),
            Err(e) => return Err(e),
        }
        let url = format!(
            "https://lrclib.net/api/search?track_name={}&artist_name={}",
            encode(&clean_title(title)),
            encode(artist)
        );
        match self.get(&url) {
            Ok(raw) => Ok(parse_search(&raw, query.duration)?.or(exact_plain)),
            Err(Error::NotFound) => Ok(exact_plain),
            Err(e) if exact_plain.is_some() => {
                let _ = e;
                Ok(exact_plain)
            }
            Err(e) => Err(e),
        }
    }
}
