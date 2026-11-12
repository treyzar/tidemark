//! MPRIS worker built on the reviewed `tidemark-mpris` client.
//!
//! The worker owns the blocking D-Bus connection, samples the current player
//! and performs transport commands; the GTK main thread only reads snapshots
//! from a channel. A sleep timer is pinned to the player identity captured
//! when it was armed, exactly as `SleepTimer` specifies.

use std::io::Read;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use tidemark_mpris::{Client, MprisPlayerId, SleepTimer, TrackSnapshot};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(250);
const MAX_ARTWORK: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub snapshot: TrackSnapshot,
    pub cover: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub enum MusicEvent {
    Snapshot(Option<NowPlaying>),
    Error(String),
    SleepArmed(u32),
    SleepCancelled,
    SleepFired,
}

#[derive(Debug, Clone)]
pub enum MusicCommand {
    Refresh,
    PlayPause,
    Next,
    Previous,
    Seek(f64),
    SetVolume(f64),
    Sleep(u32),
    CancelSleep,
}

/// Sender half. `inactive` is used by the deterministic smoke test so the
/// harness never talks to MPRIS or performs playback actions.
#[derive(Clone)]
pub struct MusicService {
    tx: Option<Sender<MusicCommand>>,
}

impl MusicService {
    pub fn send(&self, command: MusicCommand) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(command);
        }
    }

    pub fn inactive() -> Self {
        Self { tx: None }
    }
}

pub fn spawn(events: Sender<MusicEvent>) -> MusicService {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("tidemark-mpris".into())
        .spawn(move || worker(rx, events))
        .expect("starting the MPRIS worker");
    MusicService { tx: Some(tx) }
}

fn worker(commands: Receiver<MusicCommand>, events: Sender<MusicEvent>) {
    let mut client: Option<Client> = None;
    let mut current: Option<MprisPlayerId> = None;
    let mut last_snapshot: Option<TrackSnapshot> = None;
    let mut sleep: Option<SleepTimer> = None;
    let mut artwork = ArtworkCache::default();
    let mut last_error: Option<String> = None;
    let mut next_poll = Instant::now();

    loop {
        let due_sleep = sleep
            .as_mut()
            .and_then(|timer| timer.take_due(Instant::now()));
        if let Some(player) = due_sleep {
            let mut confirmed = false;
            match client.as_ref() {
                Some(connection) => match connection.pause_exact(&player) {
                    Ok(()) => match connection.snapshot(&player) {
                        Ok(snapshot) if !snapshot.playing => {
                            last_snapshot = Some(snapshot);
                            confirmed = true;
                        }
                        Ok(_) => emit_error(
                            &events,
                            &mut last_error,
                            "Таймер сна: плеер всё ещё сообщает воспроизведение после паузы."
                                .into(),
                        ),
                        Err(error) => emit_error(
                            &events,
                            &mut last_error,
                            format!("Таймер сна: пауза не подтверждена: {error}"),
                        ),
                    },
                    Err(error) => emit_error(
                        &events,
                        &mut last_error,
                        format!("Таймер сна: пауза отклонена: {error}"),
                    ),
                },
                None => emit_error(
                    &events,
                    &mut last_error,
                    "Таймер сна: нет соединения MPRIS.".into(),
                ),
            }
            sleep = None;
            if confirmed {
                let _ = events.send(MusicEvent::SleepFired);
            }
        }

        if Instant::now() >= next_poll {
            refresh(
                &mut client,
                &mut current,
                &mut last_snapshot,
                &mut artwork,
                &mut last_error,
                &events,
            );
            next_poll = Instant::now() + POLL_INTERVAL;
        }

        match commands.recv_timeout(TICK) {
            Ok(command) => {
                handle_command(
                    command,
                    &mut client,
                    &mut current,
                    &mut last_snapshot,
                    &mut sleep,
                    &mut last_error,
                    &events,
                );
                next_poll = Instant::now() + Duration::from_millis(100);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    command: MusicCommand,
    client: &mut Option<Client>,
    current: &mut Option<MprisPlayerId>,
    last_snapshot: &mut Option<TrackSnapshot>,
    sleep: &mut Option<SleepTimer>,
    last_error: &mut Option<String>,
    events: &Sender<MusicEvent>,
) {
    if matches!(command, MusicCommand::Refresh) {
        refresh(
            client,
            current,
            last_snapshot,
            &mut None,
            last_error,
            events,
        );
        return;
    }

    let Some(connection) = ensure_client(client, events) else {
        return;
    };
    let Some(player) = current.clone() else {
        emit_error(
            events,
            last_error,
            "Нет активного MPRIS-плеера. Запустите музыку в любом поддерживаемом плеере.".into(),
        );
        return;
    };

    let result = match command {
        MusicCommand::Refresh => unreachable!("handled before borrowing the client"),
        MusicCommand::PlayPause => connection.play_pause(&player),
        MusicCommand::Next => connection.next(&player),
        MusicCommand::Previous => connection.previous(&player),
        MusicCommand::Seek(fraction) => {
            let Some(snapshot) = last_snapshot.as_ref() else {
                emit_error(events, last_error, "Позиция трека ещё не получена.".into());
                return;
            };
            if !snapshot.can_seek {
                emit_error(
                    events,
                    last_error,
                    "Плеер не поддерживает перемотку.".into(),
                );
                return;
            }
            let (Some(track_id), Some(duration)) =
                (snapshot.track_id.as_deref(), snapshot.duration)
            else {
                emit_error(
                    events,
                    last_error,
                    "Плеер не сообщает длительность трека.".into(),
                );
                return;
            };
            let target = duration.mul_f64(fraction.clamp(0.0, 1.0));
            connection.seek(&player, track_id, target)
        }
        MusicCommand::SetVolume(volume) => connection.player_volume(&player, volume),
        MusicCommand::Sleep(minutes) => {
            if let Some(previous) = sleep.as_mut() {
                previous.cancel();
            }
            *sleep = Some(SleepTimer::arm(
                player,
                Duration::from_secs(minutes as u64 * 60),
            ));
            let _ = events.send(MusicEvent::SleepArmed(minutes));
            return;
        }
        MusicCommand::CancelSleep => {
            if let Some(timer) = sleep.as_mut() {
                timer.cancel();
            }
            *sleep = None;
            let _ = events.send(MusicEvent::SleepCancelled);
            return;
        }
    };

    match result {
        Ok(()) => {
            *last_error = None;
        }
        Err(error) => emit_error(events, last_error, format!("Плеер: {error}")),
    }
}

fn ensure_client(client: &mut Option<Client>, events: &Sender<MusicEvent>) -> Option<&Client> {
    if client.is_none() {
        match Client::connect() {
            Ok(connection) => *client = Some(connection),
            Err(error) => {
                let _ = events.send(MusicEvent::Error(format!(
                    "MPRIS недоступен: {error}. Проверьте сеанс D-Bus."
                )));
                return None;
            }
        }
    }
    client.as_ref()
}

#[allow(clippy::too_many_arguments)]
fn refresh(
    client: &mut Option<Client>,
    current: &mut Option<MprisPlayerId>,
    last_snapshot: &mut Option<TrackSnapshot>,
    artwork: &mut ArtworkCache,
    last_error: &mut Option<String>,
    events: &Sender<MusicEvent>,
) {
    let Some(connection) = ensure_client(client, events) else {
        return;
    };

    let players = match connection.list_players() {
        Ok(players) => players,
        Err(error) => {
            emit_error(events, last_error, format!("Список плееров: {error}"));
            return;
        }
    };

    if players.is_empty() {
        *current = None;
        *last_snapshot = None;
        *last_error = None;
        let _ = events.send(MusicEvent::Snapshot(None));
        return;
    }

    let mut chosen = current.clone().filter(|id| players.iter().any(|p| p == id));
    if chosen.is_none() {
        for player in &players {
            if let Ok(snapshot) = connection.snapshot(player) {
                if snapshot.playing {
                    chosen = Some(player.clone());
                    break;
                }
            }
        }
    }
    let player = chosen
        .or_else(|| players.first().cloned())
        .expect("players is not empty");

    match connection.snapshot(&player) {
        Ok(snapshot) => {
            let identity = track_identity(&snapshot);
            let cover = snapshot
                .art_url
                .as_deref()
                .and_then(|url| artwork.cover(&identity, url));
            *current = Some(player);
            *last_snapshot = Some(snapshot.clone());
            *last_error = None;
            let _ = events.send(MusicEvent::Snapshot(Some(NowPlaying { snapshot, cover })));
        }
        Err(error) => {
            emit_error(events, last_error, format!("Чтение плеера: {error}"));
        }
    }
}

fn emit_error(events: &Sender<MusicEvent>, last_error: &mut Option<String>, message: String) {
    if last_error.as_deref() != Some(message.as_str()) {
        *last_error = Some(message.clone());
        let _ = events.send(MusicEvent::Error(message));
    }
}

/// Interim cache keyed by the full track identity, not just the artwork URI:
/// players may reuse one URI while the bytes behind it change after a track
/// switch. Retries are bounded and spaced. The reviewed artwork crate will
/// replace the fetch path once its API is frozen.
#[derive(Default)]
struct ArtworkCache {
    key: String,
    url: String,
    bytes: Option<Vec<u8>>,
    attempts: u8,
    next_try: Option<Instant>,
}

impl ArtworkCache {
    fn cover(&mut self, identity: &str, url: &str) -> Option<Vec<u8>> {
        if self.key != identity || self.url != url {
            self.key = identity.to_string();
            self.url = url.to_string();
            self.bytes = None;
            self.attempts = 0;
            self.next_try = None;
        }
        if let Some(bytes) = &self.bytes {
            return Some(bytes.clone());
        }
        if self.attempts >= 5 {
            return None;
        }
        if self.next_try.is_some_and(|next| Instant::now() < next) {
            return None;
        }
        let fetched = fetch_cover(url);
        self.attempts += 1;
        let delay = match self.attempts {
            1 => 2,
            2 => 5,
            3 => 10,
            4 => 20,
            _ => 30,
        };
        self.next_try = Some(Instant::now() + Duration::from_secs(delay));
        if let Some(bytes) = fetched {
            self.bytes = Some(bytes.clone());
            Some(bytes)
        } else {
            None
        }
    }
}

fn track_identity(snapshot: &TrackSnapshot) -> String {
    format!(
        "{}|{}|{}|{}",
        snapshot.track_id.as_deref().unwrap_or(""),
        snapshot.title,
        snapshot.artists.join(","),
        snapshot.album.as_deref().unwrap_or("")
    )
}

fn fetch_cover(url: &str) -> Option<Vec<u8>> {
    if url.starts_with("file://") {
        let path = crate::model::art_file_path(url)?;
        let metadata = std::fs::metadata(&path).ok()?;
        if metadata.len() > MAX_ARTWORK {
            return None;
        }
        return std::fs::read(path).ok();
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let response = ureq::get(url).timeout(Duration::from_secs(5)).call().ok()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_ARTWORK)
        .read_to_end(&mut bytes)
        .ok()?;
    (!bytes.is_empty()).then_some(bytes)
}
