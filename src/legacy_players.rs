//! Winamp and AIMP, which do not publish to Windows media sessions. Both
//! expose window-message IPC: Winamp's classic WM_USER API and AIMP's
//! Remote API (window + shared memory "AIMP2_RemoteInfo"). All calls use
//! SendMessageTimeout so a hung player cannot freeze the media thread.

use crate::media::{Command, MediaState, Player};
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ};
use windows::Win32::UI::WindowsAndMessaging::*;

fn send(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(hwnd, msg, WPARAM(wparam), LPARAM(lparam), SMTO_ABORTIFHUNG | SMTO_BLOCK, 300, Some(&mut result))
    };
    (ok.0 != 0).then_some(result as isize)
}

// ---- Winamp -------------------------------------------------------------

const IPC_ISPLAYING: isize = 104;
const IPC_GETOUTPUTTIME: isize = 105;
const IPC_SETVOLUME: isize = 122;
const WINAMP_PREV: usize = 40044;
const WINAMP_PLAY: usize = 40045;
const WINAMP_PAUSE: usize = 40046;
const WINAMP_NEXT: usize = 40048;

fn winamp() -> Option<HWND> {
    unsafe { FindWindowW(w!("Winamp v1.x"), None).ok() }
}

fn window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// "12. Artist - Title - Winamp [Paused]" -> ("Artist", "Title").
pub fn parse_winamp_title(caption: &str) -> (String, String) {
    let mut s = caption.trim();
    for tail in [" [Stopped]", " [Paused]"] {
        s = s.strip_suffix(tail).unwrap_or(s);
    }
    s = s.strip_suffix(" - Winamp").unwrap_or(s);
    if let Some((num, rest)) = s.split_once(". ") {
        if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
            s = rest;
        }
    }
    match s.split_once(" - ") {
        Some((artist, title)) => (artist.trim().to_string(), title.trim().to_string()),
        None => (String::new(), s.trim().to_string()),
    }
}

fn read_winamp(h: HWND) -> Option<MediaState> {
    let status = send(h, WM_USER, 0, IPC_ISPLAYING)?;
    if status == 0 {
        return None; // Stopped: nothing to show.
    }
    let pos_ms = send(h, WM_USER, 0, IPC_GETOUTPUTTIME).unwrap_or(0).max(0) as u64;
    let len_s = send(h, WM_USER, 1, IPC_GETOUTPUTTIME).unwrap_or(0).max(0) as u64;
    let vol = send(h, WM_USER, (-666isize) as usize, IPC_SETVOLUME).map(|v| (v.clamp(0, 255) as f32) / 255.0);
    let (artist, title) = parse_winamp_title(&window_text(h));
    Some(MediaState {
        player: Player::Winamp,
        app_id: "Winamp".into(),
        title,
        artist,
        playing: status == 1,
        position: Duration::from_millis(pos_ms),
        duration: Duration::from_secs(len_s),
        sampled_at: Instant::now(),
        can_prev: true,
        can_next: true,
        cover: None,
        volume: vol,
    })
}

// ---- AIMP ---------------------------------------------------------------

const WM_AIMP_COMMAND: u32 = WM_USER + 0x75;
const WM_AIMP_PROPERTY: u32 = WM_USER + 0x77;
const PROP_GET: usize = 0;
const PROP_SET: usize = 1;
const PROP_POSITION: usize = 0x20;
const PROP_DURATION: usize = 0x30;
const PROP_STATE: usize = 0x40;
const PROP_VOLUME: usize = 0x50;
const CMD_PLAYPAUSE: usize = 14;
const CMD_NEXT: usize = 17;
const CMD_PREV: usize = 18;
const AIMP_MAP_SIZE: usize = 2048;
/// Size of TAIMPRemoteFileInfo before its UTF-16 strings.
const AIMP_HEADER: usize = 88;

fn aimp() -> Option<HWND> {
    unsafe { FindWindowW(w!("AIMP2_RemoteInfo"), None).ok() }
}

fn u32_at(buf: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

/// Parses AIMP's shared TAIMPRemoteFileInfo: a packed header with string
/// lengths (in UTF-16 units) at offsets 40..64, followed by the strings
/// Album, Artist, Date, FileName, Genre, Title. Returns (artist, title).
pub fn parse_aimp_info(buf: &[u8]) -> Option<(String, String)> {
    let lens: Vec<usize> = (0..6).map(|i| u32_at(buf, 40 + i * 4).map(|v| v as usize)).collect::<Option<_>>()?;
    let mut off = AIMP_HEADER;
    let mut strings = Vec::with_capacity(6);
    for len in lens {
        let bytes = buf.get(off..off + len * 2)?;
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        strings.push(String::from_utf16_lossy(&units));
        off += len * 2;
    }
    let (artist, title) = (strings[1].clone(), strings[5].clone());
    if title.is_empty() {
        // Untagged files: fall back to the file name without extension.
        let name = strings[3].rsplit(['\\', '/']).next().unwrap_or_default();
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        return Some((artist, stem.to_string()));
    }
    Some((artist, title))
}

fn aimp_track() -> Option<(String, String)> {
    unsafe {
        let map = OpenFileMappingW(FILE_MAP_READ.0, false, w!("AIMP2_RemoteInfo")).ok()?;
        let view = MapViewOfFile(map, FILE_MAP_READ, 0, 0, AIMP_MAP_SIZE);
        let result = (!view.Value.is_null()).then(|| {
            let buf = std::slice::from_raw_parts(view.Value as *const u8, AIMP_MAP_SIZE);
            parse_aimp_info(buf)
        });
        if !view.Value.is_null() {
            let _ = UnmapViewOfFile(view);
        }
        let _ = CloseHandle(map);
        result.flatten()
    }
}

fn read_aimp(h: HWND) -> Option<MediaState> {
    let state = send(h, WM_AIMP_PROPERTY, PROP_STATE | PROP_GET, 0)?;
    if state == 0 {
        return None;
    }
    let pos = send(h, WM_AIMP_PROPERTY, PROP_POSITION | PROP_GET, 0).unwrap_or(0).max(0) as u64;
    let dur = send(h, WM_AIMP_PROPERTY, PROP_DURATION | PROP_GET, 0).unwrap_or(0).max(0) as u64;
    let vol = send(h, WM_AIMP_PROPERTY, PROP_VOLUME | PROP_GET, 0).map(|v| v.clamp(0, 100) as f32 / 100.0);
    let (artist, title) = aimp_track().unwrap_or_default();
    Some(MediaState {
        player: Player::Aimp,
        app_id: "AIMP".into(),
        title,
        artist,
        playing: state == 2,
        position: Duration::from_millis(pos),
        duration: Duration::from_millis(dur),
        sampled_at: Instant::now(),
        can_prev: true,
        can_next: true,
        cover: None,
        volume: vol,
    })
}

// ---- Shared -------------------------------------------------------------

/// The first running legacy player with something loaded, playing first.
pub fn read() -> Option<MediaState> {
    let found: Vec<MediaState> = [winamp().and_then(read_winamp), aimp().and_then(read_aimp)].into_iter().flatten().collect();
    found.iter().find(|s| s.playing).or(found.first()).cloned()
}

pub fn command(player: Player, cmd: &Command) {
    match player {
        Player::Winamp => {
            let Some(h) = winamp() else { return };
            let id = match cmd {
                Command::PlayPause => {
                    // Toggle pause while playing/paused; start when stopped.
                    if send(h, WM_USER, 0, IPC_ISPLAYING).unwrap_or(0) == 0 { WINAMP_PLAY } else { WINAMP_PAUSE }
                }
                Command::Next => WINAMP_NEXT,
                Command::Prev => WINAMP_PREV,
                Command::SetVolume(v) => {
                    send(h, WM_USER, (v.clamp(0.0, 1.0) * 255.0).round() as usize, IPC_SETVOLUME);
                    return;
                }
                Command::Refresh => return,
            };
            send(h, WM_COMMAND, id, 0);
        }
        Player::Aimp => {
            let Some(h) = aimp() else { return };
            match cmd {
                Command::PlayPause => send(h, WM_AIMP_COMMAND, CMD_PLAYPAUSE, 0),
                Command::Next => send(h, WM_AIMP_COMMAND, CMD_NEXT, 0),
                Command::Prev => send(h, WM_AIMP_COMMAND, CMD_PREV, 0),
                Command::SetVolume(v) => {
                    send(h, WM_AIMP_PROPERTY, PROP_VOLUME | PROP_SET, (v.clamp(0.0, 1.0) * 100.0).round() as isize)
                }
                Command::Refresh => None,
            };
        }
        Player::Smtc => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winamp_captions() {
        let p = |s| parse_winamp_title(s);
        assert_eq!(p("12. Cartoon - On & On - Winamp"), ("Cartoon".into(), "On & On".into()));
        assert_eq!(p("3. Artist - Song - Winamp [Paused]"), ("Artist".into(), "Song".into()));
        assert_eq!(p("Just a title - Winamp"), (String::new(), "Just a title".into()));
        assert_eq!(p("2024. Year - Song - Winamp"), ("Year".into(), "Song".into()));
    }

    fn aimp_buf(album: &str, artist: &str, file: &str, title: &str) -> Vec<u8> {
        let fields = [album, artist, "", file, "", title];
        let mut buf = vec![0u8; AIMP_HEADER];
        for (i, f) in fields.iter().enumerate() {
            let n = f.encode_utf16().count() as u32;
            buf[40 + i * 4..44 + i * 4].copy_from_slice(&n.to_le_bytes());
        }
        for f in fields {
            for u in f.encode_utf16() {
                buf.extend_from_slice(&u.to_le_bytes());
            }
        }
        buf.resize(AIMP_MAP_SIZE, 0);
        buf
    }

    #[test]
    fn aimp_shared_info() {
        let buf = aimp_buf("Album", "Cartoon", "C:\\m\\x.mp3", "On & On");
        assert_eq!(parse_aimp_info(&buf), Some(("Cartoon".into(), "On & On".into())));
        let untagged = aimp_buf("", "", "C:\\Music\\Утро у моря.flac", "");
        assert_eq!(parse_aimp_info(&untagged), Some((String::new(), "Утро у моря".into())));
        assert_eq!(parse_aimp_info(&[0u8; 10]), None);
    }
}
