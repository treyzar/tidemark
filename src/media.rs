//! "Now playing": Windows media sessions (SMTC) plus Winamp and AIMP, which
//! do not publish to SMTC. Runs on its own MTA thread; SMTC changes arrive
//! as events, the legacy players are checked every 2 s only while no SMTC
//! session is playing. The UI thread gets a posted WM_MEDIA on changes.

use crate::{audio, legacy_players as legacy};
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{Interface, Result, GUID};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::*;
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_MEDIA: u32 = WM_APP + 3;
/// Cover art is decoded once, cropped square and scaled to this size.
pub const COVER_PX: u32 = 160;
const LEGACY_POLL: Duration = Duration::from_secs(2);

/// Decoded cover art: premultiplied BGRA, COVER_PX x COVER_PX.
#[derive(Debug, Clone)]
pub struct Cover {
    pub id: u64,
    pub pixels: Arc<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Player {
    Smtc,
    Winamp,
    Aimp,
}

#[derive(Debug, Clone)]
pub struct MediaState {
    pub player: Player,
    /// SMTC AppUserModelId, or the legacy player's name.
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub position: Duration,
    pub duration: Duration,
    /// When `position` was sampled; used to extrapolate while playing.
    pub sampled_at: Instant,
    pub can_prev: bool,
    pub can_next: bool,
    pub cover: Option<Cover>,
    /// Volume of the player's own audio session, 0..1, if found.
    pub volume: Option<f32>,
}

impl MediaState {
    pub fn position_now(&self) -> Duration {
        let pos = if self.playing { self.position + self.sampled_at.elapsed() } else { self.position };
        if self.duration > Duration::ZERO { pos.min(self.duration) } else { pos }
    }
}

pub enum Command {
    PlayPause,
    Next,
    Prev,
    SetVolume(f32),
    Refresh,
}

pub struct Service {
    state: Arc<Mutex<Option<MediaState>>>,
    tx: Sender<Command>,
}

impl Service {
    pub fn start(hwnd: HWND) -> Option<Self> {
        let state = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel();
        let shared = state.clone();
        let events_tx = tx.clone();
        let hwnd_raw = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("media".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                let hwnd = HWND(hwnd_raw as *mut _);
                if unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_err() {
                    return;
                }
                let mut worker = Worker::new(hwnd, shared, events_tx);
                worker.refresh();
                loop {
                    let wait = if worker.watch_legacy() { LEGACY_POLL } else { Duration::from_secs(3600) };
                    match rx.recv_timeout(wait) {
                        Ok(cmd) => worker.handle(cmd),
                        Err(RecvTimeoutError::Timeout) => worker.refresh(),
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .ok()?;
        Some(Self { state, tx })
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn get(&self) -> Option<MediaState> {
        self.state.lock().unwrap().clone()
    }
}

struct Worker {
    hwnd: HWND,
    shared: Arc<Mutex<Option<MediaState>>>,
    tx: Sender<Command>,
    manager: Option<GlobalSystemMediaTransportControlsSessionManager>,
    session: Option<(GlobalSystemMediaTransportControlsSession, [i64; 3])>,
    /// (track key, cover) so art is decoded once per track.
    cover_cache: Option<(u64, Option<Cover>)>,
    last: Option<MediaState>,
}

fn track_key(app: &str, title: &str, artist: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (app, title, artist).hash(&mut h);
    h.finish()
}

/// SMTC timestamps: 100 ns ticks since 1601 (same as FILETIME).
fn filetime_now() -> i64 {
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

fn ticks(d: i64) -> Duration {
    Duration::from_nanos(d.max(0) as u64 * 100)
}

impl Worker {
    fn new(hwnd: HWND, shared: Arc<Mutex<Option<MediaState>>>, tx: Sender<Command>) -> Self {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|op| op.get()).ok();
        if let Some(m) = &manager {
            let t = tx.clone();
            let _ = m.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                let _ = t.send(Command::Refresh);
                Ok(())
            }));
        }
        Self { hwnd, shared, tx, manager, session: None, cover_cache: None, last: None }
    }

    /// Legacy players have no events, so poll while no SMTC session plays.
    fn watch_legacy(&self) -> bool {
        !self.last.as_ref().is_some_and(|s| s.player == Player::Smtc && s.playing)
    }

    fn handle(&mut self, cmd: Command) {
        let player = self.last.as_ref().map(|s| s.player);
        match (cmd, player) {
            (Command::Refresh, _) => {}
            (Command::SetVolume(v), Some(Player::Smtc)) => {
                if let Some(s) = &self.last {
                    audio::set_app_volume(&s.app_id, v);
                }
            }
            (cmd, Some(Player::Smtc)) => {
                if let Some((session, _)) = &self.session {
                    let op = match cmd {
                        Command::PlayPause => session.TryTogglePlayPauseAsync(),
                        Command::Next => session.TrySkipNextAsync(),
                        _ => session.TrySkipPreviousAsync(),
                    };
                    let _ = op.and_then(|o| o.get());
                }
            }
            (cmd, Some(p)) => legacy::command(p, &cmd),
            (_, None) => {}
        }
        self.refresh();
    }

    fn watch_session(&mut self, session: Option<GlobalSystemMediaTransportControlsSession>) {
        let same = match (&self.session, &session) {
            (Some((a, _)), Some(b)) => a.as_raw() == b.as_raw() || a == b,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        if let Some((old, [a, b, c])) = self.session.take() {
            let _ = old.RemoveMediaPropertiesChanged(a);
            let _ = old.RemovePlaybackInfoChanged(b);
            let _ = old.RemoveTimelinePropertiesChanged(c);
        }
        if let Some(s) = session {
            let ev = |tx: &Sender<Command>| {
                let t = tx.clone();
                move || {
                    let _ = t.send(Command::Refresh);
                }
            };
            let (f1, f2, f3) = (ev(&self.tx), ev(&self.tx), ev(&self.tx));
            let a = s.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                f1();
                Ok(())
            }));
            let b = s.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                f2();
                Ok(())
            }));
            let c = s.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                f3();
                Ok(())
            }));
            self.session = Some((s, [a.unwrap_or(0), b.unwrap_or(0), c.unwrap_or(0)]));
        }
    }

    fn read_smtc(&mut self) -> Option<MediaState> {
        let session = self.manager.as_ref()?.GetCurrentSession().ok();
        self.watch_session(session.clone());
        let s = session?;
        let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
        let info = s.GetPlaybackInfo().ok()?;
        let playing = info.PlaybackStatus().ok()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
        let controls = info.Controls().ok();
        let timeline = s.GetTimelineProperties().ok();
        let app_id = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
        let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
        let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();

        let (mut position, duration) = timeline
            .as_ref()
            .map(|t| {
                let pos = t.Position().map(|p| ticks(p.Duration)).unwrap_or_default();
                let end = t.EndTime().map(|p| ticks(p.Duration)).unwrap_or_default();
                (pos, end)
            })
            .unwrap_or_default();
        if playing {
            // Position is reported as of LastUpdatedTime; bring it to now.
            if let Some(updated) = timeline.as_ref().and_then(|t| t.LastUpdatedTime().ok()) {
                let age = filetime_now() - updated.UniversalTime;
                if age > 0 && age < 36_000_000_000 {
                    position += ticks(age);
                }
            }
        }

        let key = track_key(&app_id, &title, &artist);
        let cover = match &self.cover_cache {
            Some((k, c)) if *k == key => c.clone(),
            _ => {
                let c = props.Thumbnail().ok().and_then(|t| read_cover(&t));
                self.cover_cache = Some((key, c.clone()));
                c
            }
        };

        Some(MediaState {
            player: Player::Smtc,
            volume: audio::app_volume(&app_id),
            app_id,
            title,
            artist,
            playing,
            position,
            duration,
            sampled_at: Instant::now(),
            can_prev: controls.as_ref().and_then(|c| c.IsPreviousEnabled().ok()).unwrap_or(true),
            can_next: controls.as_ref().and_then(|c| c.IsNextEnabled().ok()).unwrap_or(true),
            cover,
        })
    }

    fn refresh(&mut self) {
        let started = Instant::now();
        let smtc = self.read_smtc();
        // A playing SMTC session wins; otherwise a running legacy player.
        let state = match smtc {
            Some(s) if s.playing => Some(s),
            other => legacy::read().or(other),
        };
        let changed = match (&state, &self.last) {
            (Some(a), Some(b)) => !same_view(a, b),
            (None, None) => false,
            _ => true,
        };
        crate::stats::log_line(&format!("media refresh: {:?} changed={changed}", started.elapsed()));
        *self.shared.lock().unwrap() = state.clone();
        self.last = state;
        if changed {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_MEDIA, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// True when the two states would render identically (positions are
/// extrapolated, so only a jump of more than 2 s counts as a change).
fn same_view(a: &MediaState, b: &MediaState) -> bool {
    let drift = a.position_now().abs_diff(b.position_now());
    a.player == b.player
        && a.app_id == b.app_id
        && a.title == b.title
        && a.artist == b.artist
        && a.playing == b.playing
        && a.duration == b.duration
        && a.can_prev == b.can_prev
        && a.can_next == b.can_next
        && a.cover.as_ref().map(|c| c.id) == b.cover.as_ref().map(|c| c.id)
        && a.volume.map(|v| (v * 100.0).round()) == b.volume.map(|v| (v * 100.0).round())
        && drift < Duration::from_secs(2)
}

fn read_cover(reference: &IRandomAccessStreamReference) -> Option<Cover> {
    let stream = reference.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 8 << 20 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).ok()?).ok()?;
    reader.LoadAsync(size).ok()?.get().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    decode_cover(&bytes).ok()
}

/// Decodes any WIC-supported image, crops it to a centered square and
/// scales it to COVER_PX, as premultiplied BGRA.
pub fn decode_cover(bytes: &[u8]) -> Result<Cover> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = wic.CreateDecoderFromStream(&stream, &GUID::zeroed(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h)?;
        let side = w.min(h).max(1);
        let clipper = wic.CreateBitmapClipper()?;
        let rect = WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
        clipper.Initialize(&frame, &rect)?;
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&clipper, COVER_PX, COVER_PX, WICBitmapInterpolationModeFant)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
        let stride = COVER_PX * 4;
        let mut pixels = vec![0u8; (stride * COVER_PX) as usize];
        converter.CopyPixels(std::ptr::null(), stride, &mut pixels)?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut h);
        Ok(Cover { id: h.finish(), pixels: Arc::new(pixels) })
    }
}

/// "3:07", "1:02:45".
pub fn format_time(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_format() {
        assert_eq!(format_time(Duration::from_secs(0)), "0:00");
        assert_eq!(format_time(Duration::from_secs(187)), "3:07");
        assert_eq!(format_time(Duration::from_secs(3765)), "1:02:45");
    }

    fn state(playing: bool, pos: u64) -> MediaState {
        MediaState {
            player: Player::Smtc,
            app_id: "Spotify.exe".into(),
            title: "On & On".into(),
            artist: "Cartoon".into(),
            playing,
            position: Duration::from_secs(pos),
            duration: Duration::from_secs(200),
            sampled_at: Instant::now(),
            can_prev: true,
            can_next: true,
            cover: None,
            volume: Some(0.5),
        }
    }

    #[test]
    fn small_position_drift_is_not_a_change() {
        assert!(same_view(&state(true, 10), &state(true, 11)));
        assert!(!same_view(&state(true, 10), &state(true, 40)));
        assert!(!same_view(&state(true, 10), &state(false, 10)));
    }

    #[test]
    fn paused_position_does_not_advance_and_playing_is_capped() {
        let mut s = state(false, 50);
        s.sampled_at = Instant::now() - Duration::from_secs(5);
        assert_eq!(s.position_now(), Duration::from_secs(50));
        let mut s = state(true, 199);
        s.sampled_at = Instant::now() - Duration::from_secs(5);
        assert_eq!(s.position_now(), Duration::from_secs(200));
    }

    #[test]
    fn decodes_and_crops_a_png() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        // 2x1 PNG: red, blue. Cropped to the centered 1x1 square, scaled up.
        let png: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x7B, 0x40, 0xE8, 0xDD, 0x00, 0x00, 0x00, 0x0F, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xC0, 0xC0, 0xF0, 0x1F, 0x00, 0x07, 0x00, 0x01, 0xFF, 0x7E, 0x08, 0xB1, 0xD0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82];
        let cover = decode_cover(png).expect("decode");
        assert_eq!(cover.pixels.len(), (COVER_PX * COVER_PX * 4) as usize);
        // BGRA of the first pixel: pure red, opaque.
        assert_eq!(&cover.pixels[0..4], &[0, 0, 255, 255]);
    }
}

/// Live check against this PC: `cargo test media::live -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore]
    fn current_session() {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let (tx, _rx) = mpsc::channel();
        let mut w = Worker::new(HWND::default(), Arc::new(Mutex::new(None)), tx);
        let t = Instant::now();
        let s = w.read_smtc();
        println!("smtc ({:?}): {:?}", t.elapsed(), s.as_ref().map(|s| (&s.app_id, &s.title, &s.artist, s.playing, s.position, s.duration, s.volume, s.cover.as_ref().map(|c| c.pixels.len()))));
        println!("legacy: {:?}", legacy::read().map(|s| (s.app_id, s.title, s.playing)));
    }
}
