//! Pure owner/generation and clipboard-offer guards, no desktop access.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tidemark_core::{clip_history::MAX_TEXT_CHARS, shell::ClipboardOffer};

/// A privacy policy published outside the bounded command queue. Every toggle
/// advances the revision, including disable/enable before the worker wakes.
#[derive(Clone, Default)]
pub struct CapturePolicy(Arc<AtomicU64>);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureState(u64);
impl CaptureState {
    pub fn enabled(self) -> bool {
        self.0 & 1 != 0
    }
}
impl CapturePolicy {
    pub fn new(enabled: bool) -> Self {
        Self(Arc::new(AtomicU64::new(u64::from(enabled))))
    }
    pub fn publish(&self, enabled: bool) {
        let mut current = self.0.load(Ordering::SeqCst);
        while (current & 1 != 0) != enabled {
            match self.0.compare_exchange(
                current,
                current.wrapping_add(1),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(next) => current = next,
            }
        }
    }
    pub fn snapshot(&self) -> CaptureState {
        CaptureState(self.0.load(Ordering::SeqCst))
    }
    pub fn permits(&self, state: CaptureState) -> bool {
        state.enabled() && self.snapshot() == state
    }
    pub fn accepts_read(&self, key: &ReadKey) -> bool {
        self.permits(key.capture)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub owner: String,
    pub generation: u64,
}
impl Peer {
    pub fn matches(&self, owner: &str, generation: u64) -> bool {
        self.owner == owner && self.generation == generation
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadKey {
    pub peer: Peer,
    pub offer: u64,
    pub mime: String,
    capture: CaptureState,
}
#[derive(Default)]
pub struct ClipboardGuard {
    enabled: bool,
    serial: u64,
    current: Option<ReadKey>,
    capture: CaptureState,
}
impl ClipboardGuard {
    pub fn set_policy(&mut self, capture: CaptureState) {
        if self.capture != capture {
            self.invalidate();
            self.capture = capture;
        }
        self.enable(capture.enabled());
    }
    pub fn enable(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.invalidate();
            self.enabled = enabled;
        }
    }
    pub fn invalidate(&mut self) {
        self.serial = self.serial.wrapping_add(1);
        self.current = None;
    }
    pub fn offer(&mut self, peer: &Peer, offer: &ClipboardOffer) -> Option<ReadKey> {
        self.invalidate();
        if self.enabled
            && !offer.sensitive
            && !offer.mimes.iter().any(|m| m == "x-kde-passwordManagerHint")
        {
            let mime = ["text/uri-list", "text/plain;charset=utf-8", "text/plain"]
                .into_iter()
                .find(|m| offer.mimes.iter().any(|offered| offered == m));
            self.current = mime.map(|mime| ReadKey {
                peer: peer.clone(),
                offer: self.serial,
                mime: mime.into(),
                capture: self.capture,
            });
        }
        self.current.clone()
    }
    pub fn current(&self) -> Option<ReadKey> {
        self.current.clone()
    }
    pub fn accepts(&self, key: &ReadKey) -> bool {
        self.enabled && self.current.as_ref() == Some(key)
    }
}
pub const MAX_TEXT_BYTES: usize = MAX_TEXT_CHARS * 4;
pub const MAX_FILES_BYTES: usize = 256 * 1024;
pub const MAX_FILES: usize = 256;
pub const MAX_PATH_BYTES: usize = 4096;
/// Only the legacy application-generated image namespace may be read/deleted.
/// Stored history is user-editable and cannot grant ownership of other files.
pub fn owned_image_name(name: &str) -> bool {
    name.strip_prefix("img-")
        .and_then(|rest| rest.strip_suffix(".png"))
        .is_some_and(|hash| hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
#[derive(Debug)]
pub enum Content {
    Text(String),
    Files(Vec<String>),
}
pub fn byte_limit(mime: &str) -> usize {
    if mime == "text/uri-list" {
        MAX_FILES_BYTES
    } else {
        MAX_TEXT_BYTES
    }
}
/// Parse references only. Never stat, open, copy, resolve or launch a source.
pub fn files(bytes: &[u8]) -> Result<Vec<String>, String> {
    let invalid = || "Буфер содержит недопустимый список локальных файлов".to_owned();
    if bytes.len() > MAX_FILES_BYTES {
        return Err(invalid());
    }
    let input = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut paths = Vec::new();
    for line in input.split('\n') {
        let uri = line.strip_suffix('\r').unwrap_or(line);
        if uri.is_empty() || uri.starts_with('#') {
            continue;
        }
        if paths.len() >= MAX_FILES {
            return Err(invalid());
        }
        let encoded = uri.strip_prefix("file://").ok_or_else(invalid)?;
        // Empty authority only: network shares and remote hosts are excluded.
        if !encoded.starts_with('/')
            || encoded.starts_with("//")
            || encoded.contains(['?', '#', '\\'])
            || encoded.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        let mut decoded = Vec::with_capacity(encoded.len().min(MAX_PATH_BYTES));
        let mut bytes = encoded.as_bytes().iter().copied();
        while let Some(byte) = bytes.next() {
            let byte = if byte == b'%' {
                let high = bytes
                    .next()
                    .and_then(|v| (v as char).to_digit(16))
                    .ok_or_else(invalid)?;
                let low = bytes
                    .next()
                    .and_then(|v| (v as char).to_digit(16))
                    .ok_or_else(invalid)?;
                (high * 16 + low) as u8
            } else {
                byte
            };
            decoded.push(byte);
            if decoded.len() > MAX_PATH_BYTES {
                return Err(invalid());
            }
        }
        let path = String::from_utf8(decoded).map_err(|_| invalid())?;
        if path.starts_with("//")
            || path.contains('\\')
            || path.chars().any(char::is_control)
            || path.split('/').any(|part| part == "." || part == "..")
        {
            return Err(invalid());
        }
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Err(invalid());
    }
    Ok(paths)
}
pub fn content(mime: &str, bytes: &[u8]) -> Result<Content, String> {
    if mime == "text/uri-list" {
        files(bytes).map(Content::Files)
    } else {
        text(bytes).map(Content::Text)
    }
}
/// One active snapshot plus one replaceable pending snapshot. This keeps slow
/// disks from retaining a full history JSON for every clipboard change.
pub struct SaveSnapshot {
    pub serial: u64,
    pub json: String,
    pub dropped: Vec<String>,
}
#[derive(Default)]
struct SaveState {
    pending: Option<SaveSnapshot>,
    active: bool,
    closed: bool,
    result: Option<(u64, Result<(), String>)>,
}
#[derive(Default)]
pub struct SaveQueue {
    state: std::sync::Mutex<SaveState>,
    changed: std::sync::Condvar,
}
impl SaveQueue {
    pub fn submit(&self, mut snapshot: SaveSnapshot) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return false;
        }
        if let Some(previous) = state.pending.take() {
            for name in previous.dropped {
                if !snapshot.dropped.contains(&name) {
                    snapshot.dropped.push(name);
                }
            }
        }
        state.pending = Some(snapshot);
        // Flush waiters share this condition variable with the worker.
        self.changed.notify_all();
        true
    }
    pub fn take(&self) -> Option<SaveSnapshot> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        while state.pending.is_none() && !state.closed {
            state = self.changed.wait(state).unwrap_or_else(|p| p.into_inner());
        }
        let snapshot = state.pending.take()?;
        state.active = true;
        Some(snapshot)
    }
    pub fn complete(&self, serial: u64, result: Result<(), String>) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.active = false;
        state.result = Some((serial, result));
        self.changed.notify_all();
    }
    pub fn result(&self) -> Option<(u64, Result<(), String>)> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .result
            .take()
    }
    pub fn flush(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        while state.active || state.pending.is_some() {
            state = self.changed.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }
    pub fn close(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).closed = true;
        self.changed.notify_all();
    }
}
pub fn text(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_TEXT_BYTES {
        return Err("Текст в буфере превышает допустимый размер".into());
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| "Буфер содержит текст не в UTF-8".to_owned())?;
    if text.trim().is_empty() || text.chars().count() > MAX_TEXT_CHARS {
        return Err("Текст в буфере пуст или слишком длинный".into());
    }
    Ok(text.to_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_cleanup_only_accepts_legacy_owned_basenames() {
        assert!(owned_image_name("img-0123456789abcdef.png"));
        assert!(owned_image_name("img-0123456789ABCDEF.png"));
        for name in [
            "history.json",
            "notes.png",
            "img-123.png",
            "img-0123456789abcdeg.png",
            "../img-0123456789abcdef.png",
            "/img-0123456789abcdef.png",
            "img-0123456789abcdef.png/other",
            "img-0123456789abcdef.PNG",
        ] {
            assert!(!owned_image_name(name), "{name}");
        }
    }
    fn peer(n: u64) -> Peer {
        Peer {
            owner: ":1.2".into(),
            generation: n,
        }
    }
    fn offer() -> ClipboardOffer {
        ClipboardOffer {
            mimes: vec!["text/plain".into()],
            sensitive: false,
        }
    }
    #[test]
    fn capture_policy_invalidates_disable_enable_aba_even_before_worker_wakes() {
        let policy = CapturePolicy::new(true);
        let original = policy.snapshot();
        assert!(policy.permits(original));
        policy.publish(false);
        assert!(!policy.permits(original));
        policy.publish(true);
        assert!(!policy.permits(original));
        assert!(policy.permits(policy.snapshot()));
    }
    #[test]
    fn queued_gui_read_is_rejected_after_disable_enable_but_not_identical_enable() {
        let policy = CapturePolicy::new(true);
        let mut guard = ClipboardGuard::default();
        guard.set_policy(policy.snapshot());
        let queued = guard.offer(&peer(1), &offer()).unwrap();
        assert!(policy.accepts_read(&queued));
        policy.publish(true);
        assert!(policy.accepts_read(&queued));
        policy.publish(false);
        assert!(!policy.accepts_read(&queued));
        policy.publish(true);
        // GUI may still hold this exact offer while the worker has not woken.
        assert!(guard.accepts(&queued));
        assert!(!policy.accepts_read(&queued));
        guard.set_policy(policy.snapshot());
        assert!(!guard.accepts(&queued));
        let fresh = guard.offer(&peer(1), &offer()).unwrap();
        assert!(policy.accepts_read(&fresh));
        assert_ne!(queued, fresh);
    }
    #[test]
    fn non_mime_x11_target_is_not_requested_from_the_gnome_adapter() {
        let mut guard = ClipboardGuard::default();
        guard.enable(true);
        assert!(guard
            .offer(
                &peer(1),
                &ClipboardOffer {
                    mimes: vec!["UTF8_STRING".into()],
                    sensitive: false,
                }
            )
            .is_none());
    }
    #[test]
    fn same_unique_owner_new_generation_never_accepts_old_mutation() {
        assert!(peer(1).matches(":1.2", 1));
        assert!(!peer(1).matches(":1.2", 2));
        assert!(!peer(1).matches(":1.3", 1));
    }
    #[test]
    fn newer_offer_secret_and_setting_disable_invalidate_reads() {
        let mut guard = ClipboardGuard::default();
        guard.enable(true);
        let first = guard.offer(&peer(1), &offer()).unwrap();
        assert!(guard.accepts(&first));
        let next = guard.offer(&peer(1), &offer()).unwrap();
        assert!(!guard.accepts(&first));
        assert!(guard.accepts(&next));
        let mut secret = offer();
        secret.sensitive = true;
        assert!(guard.offer(&peer(1), &secret).is_none());
        assert!(!guard.accepts(&next));
        secret.sensitive = false;
        secret.mimes.push("x-kde-passwordManagerHint".into());
        assert!(guard.offer(&peer(1), &secret).is_none());
        let current = guard.offer(&peer(1), &offer()).unwrap();
        guard.enable(false);
        assert!(!guard.accepts(&current));
    }
    #[test]
    fn text_bounds_count_unicode_characters_and_reject_invalid_utf8() {
        assert_eq!(text("Пример".as_bytes()).unwrap(), "Пример");
        assert!(text(&[255]).is_err());
        assert!(text("я".repeat(MAX_TEXT_CHARS).as_bytes()).is_ok());
        assert!(text("я".repeat(MAX_TEXT_CHARS + 1).as_bytes()).is_err());
    }
    #[test]
    fn uri_list_prefers_files_over_plain_text_and_decodes_references_only() {
        let mut guard = ClipboardGuard::default();
        guard.enable(true);
        let key = guard
            .offer(
                &peer(1),
                &ClipboardOffer {
                    mimes: vec!["text/plain".into(), "text/uri-list".into()],
                    sensitive: false,
                },
            )
            .unwrap();
        assert_eq!(key.mime, "text/uri-list");
        assert_eq!(byte_limit(&key.mime), MAX_FILES_BYTES);
        assert_eq!(files(b"# copied files\r\nfile:///tmp/a%20b\r\nfile:///tmp/%D1%8F\r\nfile:///tmp/a%20b\r\n").unwrap(), vec!["/tmp/a b", "/tmp/я"]);
        assert!(guard.accepts(&key));
        guard.offer(&peer(2), &offer());
        assert!(!guard.accepts(&key));
    }
    #[test]
    fn uri_list_rejects_remote_malformed_controls_and_traversal() {
        for uri in [
            "https://example.test/a",
            "file://host/a",
            "file:relative",
            "file:////host/a",
            "file:///tmp/%2e%2e/a",
            "file:///a/./b",
            "file:///a%00b",
            "file:///a%0Ab",
            "file:///a%5Cb",
            "file:///a?query",
            "file:///a#fragment",
            "file:///a%ZZ",
            "file:///a%FF",
            "file:///%2Fremote/a",
        ] {
            assert!(files(uri.as_bytes()).is_err(), "{uri}");
        }
        assert!(files(&[255]).is_err());
        assert!(files(b"# only comment\r\n").is_err());
    }
    #[test]
    fn uri_list_bounds_bytes_count_and_decoded_path() {
        let at_limit = (0..MAX_FILES)
            .map(|i| format!("file:///tmp/{i}\n"))
            .collect::<String>();
        assert_eq!(files(at_limit.as_bytes()).unwrap().len(), MAX_FILES);
        assert!(files(format!("{at_limit}file:///one-more\n").as_bytes()).is_err());
        assert!(files(&vec![b'#'; MAX_FILES_BYTES + 1]).is_err());
        let max_path = format!("file:///{}", "a".repeat(MAX_PATH_BYTES - 1));
        assert!(files(max_path.as_bytes()).is_ok());
        assert!(files(format!("{max_path}a").as_bytes()).is_err());
    }
    #[test]
    fn slow_save_coalesces_latest_snapshot_preserves_cleanup_and_flushes() {
        let queue = Arc::new(SaveQueue::default());
        let snapshot = |serial, json: &str, dropped: &str| SaveSnapshot {
            serial,
            json: json.into(),
            dropped: vec![dropped.into()],
        };
        assert!(queue.submit(snapshot(1, "first", "one.png")));
        let first = queue.take().unwrap();
        assert_eq!(first.serial, 1);
        assert!(queue.submit(snapshot(2, "superseded", "two.png")));
        assert!(queue.submit(snapshot(3, "latest", "three.png")));
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let wait_queue = queue.clone();
        let waiter = std::thread::spawn(move || {
            wait_queue.flush();
            done_tx.send(()).unwrap();
        });
        assert!(done_rx.try_recv().is_err());
        queue.complete(first.serial, Ok(()));
        let latest = queue.take().unwrap();
        assert_eq!(latest.serial, 3);
        assert_eq!(latest.json, "latest");
        assert_eq!(latest.dropped, vec!["three.png", "two.png"]);
        assert!(done_rx.try_recv().is_err());
        queue.complete(latest.serial, Ok(()));
        done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        waiter.join().unwrap();
        assert_eq!(queue.result(), Some((3, Ok(()))));
        queue.close();
        assert!(!queue.submit(snapshot(4, "closed", "four.png")));
        assert!(queue.take().is_none());
    }
}
