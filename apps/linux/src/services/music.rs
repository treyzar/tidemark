//! MPRIS worker built on the reviewed `tidemark-mpris` client.
//!
//! The worker owns the blocking D-Bus connection, samples the current player
//! and performs transport commands; the GTK main thread only reads snapshots
//! from a channel. A sleep timer is pinned to the player identity captured
//! when it was armed, exactly as `SleepTimer` specifies.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tidemark_artwork::Cover;
use tidemark_core::media_artwork::ArtworkSchedule;
use tidemark_mpris::{Client, MprisPlayerId, SleepTimer, TrackSnapshot};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub snapshot: TrackSnapshot,
    pub cover: Option<Arc<Cover>>,
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
            let now = Instant::now();
            // An unavailable D-Bus provider must consume due retries too.
            artwork.schedule.finish_attempt(now);
            next_poll = now
                + artwork
                    .schedule
                    .wait(now)
                    .unwrap_or(POLL_INTERVAL)
                    .min(POLL_INTERVAL);
        }

        match commands.recv_timeout(TICK) {
            Ok(command) => {
                handle_command(
                    command,
                    &mut client,
                    &mut current,
                    &mut last_snapshot,
                    &mut sleep,
                    &mut artwork,
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
    artwork: &mut ArtworkCache,
    last_error: &mut Option<String>,
    events: &Sender<MusicEvent>,
) {
    if matches!(command, MusicCommand::Refresh) {
        artwork.schedule.invalidate();
        refresh(client, current, last_snapshot, artwork, last_error, events);
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

fn ensure_client<'a>(
    client: &'a mut Option<Client>,
    events: &Sender<MusicEvent>,
) -> Option<&'a Client> {
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
        artwork.reset();
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
            let cover = artwork.cover(&snapshot);
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

/// Worker cache keyed by session owner, track metadata and URI. Scheduled
/// reads also refetch successful covers because players can replace bytes
/// at an unchanged URL. Fetching and decoding never run on the GTK thread.
#[derive(Default)]
struct ArtworkCache {
    key: Option<u64>,
    cover: Option<Arc<Cover>>,
    schedule: ArtworkSchedule,
}

impl ArtworkCache {
    fn reset(&mut self) {
        self.key = None;
        self.cover = None;
        self.schedule.reset();
    }

    fn cover(&mut self, snapshot: &TrackSnapshot) -> Option<Arc<Cover>> {
        let key = artwork_key(snapshot);
        if self.key != Some(key) {
            self.key = Some(key);
            self.cover = None;
        }
        let now = Instant::now();
        if self.schedule.needs_read(key, now) {
            self.schedule.read(key, now);
            // Missing URLs consume retries too, allowing late metadata
            // without leaving a permanent busy loop or sticky None.
            if let Some(url) = snapshot.art_url.as_deref() {
                if let Ok(cover) = tidemark_artwork::fetch(url) {
                    self.cover = Some(Arc::new(cover));
                }
            }
            self.schedule.finish_attempt(Instant::now());
        }
        self.cover.clone()
    }
}

fn artwork_key(snapshot: &TrackSnapshot) -> u64 {
    let mut hash = DefaultHasher::new();
    (
        &snapshot.player,
        &snapshot.track_id,
        &snapshot.title,
        &snapshot.artists,
        &snapshot.album,
        &snapshot.art_url,
    )
        .hash(&mut hash);
    hash.finish()
}
