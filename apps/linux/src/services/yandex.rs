//! Explicit favorite-operation worker. Construction is inert; no GTK values enter it.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
    Arc,
};
use tidemark_mpris::{MprisPlayerId, TrackSnapshot};
#[path = "yandex_state.rs"]
pub mod state;
pub use state::{Request, StartError, TrackKey};

#[derive(Debug)]
pub enum Failure {
    Cancelled,
    PlayerUnavailable,
    TrackChanged,
    Accessibility(tidemark_yandex::Error),
    WorkerStopped,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Трек изменён: операция отменена."),
            Self::PlayerUnavailable => {
                f.write_str("Не удалось проверить исходный MPRIS-плеер. Лайк не подтверждён.")
            }
            Self::TrackChanged => {
                f.write_str("В исходном плеере уже другой трек. Лайк не изменён.")
            }
            Self::Accessibility(error) => {
                write!(f, "Яндекс Музыка не подтвердила состояние лайка: {error}")
            }
            Self::WorkerStopped => {
                f.write_str("Управление лайками недоступно: рабочий поток остановлен.")
            }
        }
    }
}
#[derive(Debug)]
pub struct Outcome {
    pub request: Request,
    pub result: Result<bool, Failure>,
    pub current: bool,
}
struct Completion {
    request: Request,
    result: Result<bool, Failure>,
}
/// UI-owned service. Use Rc<RefCell<Service>> if GTK callbacks share it.
/// Call observe on EVERY music update, including None; this preserves ABA history.
pub struct Service {
    state: state::State,
    rx: Option<Receiver<Completion>>,
    cancel: Option<Arc<AtomicBool>>,
    enabled: bool,
}
impl Default for Service {
    fn default() -> Self {
        Self::new()
    }
}
impl Service {
    pub fn new() -> Self {
        Self {
            state: state::State::default(),
            rx: None,
            cancel: None,
            enabled: true,
        }
    }
    pub fn inactive() -> Self {
        let mut service = Self::new();
        service.enabled = false;
        service
    }
    pub fn available(&self) -> bool {
        self.enabled
    }
    pub fn busy(&self) -> bool {
        self.state.busy()
    }
    /// Do this before draining completion events on each GTK music update.
    pub fn observe(&mut self, snapshot: Option<&TrackSnapshot>) {
        let key = snapshot.and_then(capture);
        if self
            .state
            .pending()
            .is_some_and(|request| Some(&request.key) != key.as_ref())
        {
            if let Some(cancel) = &self.cancel {
                cancel.store(true, Ordering::Release);
            }
        }
        self.state.observe(key);
    }
    /// Starts at most one explicit desired-state operation. Never queues a second
    /// request, redirects to the newly selected player, or optimistically likes.
    pub fn start(&mut self, snapshot: &TrackSnapshot, want: bool) -> Result<Request, StartError> {
        if !self.enabled {
            return Err(StartError::Inactive);
        }
        if self.busy() {
            return Err(StartError::Busy);
        }
        let key = capture(snapshot).ok_or(StartError::InvalidMetadata)?;
        self.state.observe(Some(key));
        let request = self.state.begin(want)?;
        let task = request.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let started = std::thread::Builder::new()
            .name("yandex-favorite".into())
            .spawn(move || {
                let result = run(&task, &flag);
                let _ = tx.try_send(Completion {
                    request: task,
                    result,
                });
            });
        if started.is_err() {
            self.state.complete(&request);
            self.enabled = false;
            return Err(StartError::WorkerUnavailable);
        }
        self.rx = Some(rx);
        self.cancel = Some(cancel);
        Ok(request)
    }
    /// Poll on the GTK thread. Render a completion only when current == true.
    /// Stale results still release busy state, but may never update the new track.
    pub fn try_recv(&mut self) -> Option<Outcome> {
        let received = self.rx.as_ref()?.try_recv();
        let completion = match received {
            Ok(completion) => completion,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                self.enabled = false;
                Completion {
                    request: self.state.pending()?.clone(),
                    result: Err(Failure::WorkerStopped),
                }
            }
        };
        self.rx = None;
        self.cancel = None;
        let current = self.state.complete(&completion.request);
        Some(Outcome {
            request: completion.request,
            result: completion.result,
            current,
        })
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Release);
        }
    }
}
/// A missing/NoTrack ID cannot establish exact track identity; fail closed.
pub fn capture(snapshot: &TrackSnapshot) -> Option<TrackKey> {
    let key = TrackKey {
        player_bus: snapshot.player.bus_name.clone(),
        player_owner: snapshot.player.owner.clone(),
        track_id: snapshot.track_id.clone()?,
        title: snapshot.title.clone(),
        artists: snapshot.artists.clone(),
    };
    key.valid().then_some(key)
}
fn run(request: &Request, cancel: &AtomicBool) -> Result<bool, Failure> {
    let cancelled = || {
        if cancel.load(Ordering::Acquire) {
            Err(Failure::Cancelled)
        } else {
            Ok(())
        }
    };
    cancelled()?;
    let player = MprisPlayerId::new(
        request.key.player_bus.clone(),
        request.key.player_owner.clone(),
    );
    let mpris = tidemark_mpris::Client::connect().map_err(|_| Failure::PlayerUnavailable)?;
    let snapshot = mpris
        .snapshot(&player)
        .map_err(|_| Failure::PlayerUnavailable)?;
    if capture(&snapshot).as_ref() != Some(&request.key) {
        return Err(Failure::TrackChanged);
    }
    cancelled()?;
    let yandex = tidemark_yandex::Client::connect().map_err(Failure::Accessibility)?;
    cancelled()?;
    // Browser connection/discovery may have taken time; verify the pinned MPRIS
    // identity and exact metadata again immediately before entering the mutation.
    let snapshot = mpris
        .snapshot(&player)
        .map_err(|_| Failure::PlayerUnavailable)?;
    if capture(&snapshot).as_ref() != Some(&request.key) {
        return Err(Failure::TrackChanged);
    }
    cancelled()?;
    yandex
        .set_liked_with_guard(
            &request.key.title,
            &request.key.artists.join(", "),
            request.want,
            || !cancel.load(Ordering::Acquire),
        )
        .map_err(Failure::Accessibility)
}
