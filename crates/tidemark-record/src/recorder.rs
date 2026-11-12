use crate::{
    portal::Session,
    process::{Encoder, Output},
    shutdown_action, Error, Options, Result, ShutdownAction, StartCancellation, Status,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc, Mutex, Weak,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
static BUSY: AtomicBool = AtomicBool::new(false);
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static ACTIVE: Mutex<Option<Weak<Runtime>>> = Mutex::new(None);
struct Permit;
impl Permit {
    fn acquire() -> Result<Self> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| Error::Busy)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}
struct Progress {
    status: Status,
    result: Option<Result<PathBuf>>,
}
struct Runtime {
    state: Mutex<Progress>,
    control: AtomicU8,
    child_live: AtomicBool,
    cancellation: StartCancellation,
}
impl Runtime {
    fn new() -> Self {
        Self {
            state: Mutex::new(Progress {
                status: Status::Starting,
                result: None,
            }),
            control: AtomicU8::new(0),
            child_live: AtomicBool::new(false),
            cancellation: StartCancellation::default(),
        }
    }
    fn status(&self) -> Status {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .status
            .clone()
    }
    fn canceled(&self) -> bool {
        self.control.load(Ordering::Acquire) == 2
            || self.cancellation.is_canceled()
            || SHUTTING_DOWN.load(Ordering::Acquire) && self.status() == Status::Starting
    }
    fn request_stop(&self) {
        let _ = self
            .control
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
    }
    fn cancel(&self) {
        self.control.store(2, Ordering::Release);
    }
}
/// Blocking start/stop belong on a UI worker. poll/status are nonblocking.
/// Dropping requests abort; only stop() or a successful poll proves a saved file.
pub struct Recorder {
    runtime: Arc<Runtime>,
    worker: Option<JoinHandle<()>>,
    stop_wait: Duration,
}
impl Recorder {
    pub fn start(options: Options, path: impl AsRef<Path>) -> Result<Self> {
        Self::start_with_cancel(options, path, StartCancellation::default())
    }
    pub fn start_with_cancel(
        options: Options,
        path: impl AsRef<Path>,
        cancellation: StartCancellation,
    ) -> Result<Self> {
        let path = path.as_ref();
        options.validate(path)?;
        if cancellation.is_canceled() || SHUTTING_DOWN.load(Ordering::Acquire) {
            return Err(Error::Canceled);
        }
        if path.exists() {
            return Err(Error::Output);
        }
        let permit = Permit::acquire()?;
        if !std::fs::metadata(crate::process::EXECUTABLE).is_ok_and(|m| m.is_file()) {
            return Err(Error::Encoder);
        }
        let runtime = Arc::new(Runtime {
            cancellation,
            ..Runtime::new()
        });
        {
            let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            if SHUTTING_DOWN.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            *active = Some(Arc::downgrade(&runtime));
        }
        let shared = runtime.clone();
        let path = path.to_owned();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let startup_wait =
            options.portal_timeout + options.startup_timeout + Duration::from_secs(10);
        let stop_wait = options.stop_timeout + Duration::from_secs(4);
        let worker = thread::Builder::new()
            .name("screen-recording".into())
            .spawn(move || {
                let _permit = permit;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    prepare_and_run(&options, &path, &shared, &ready_tx)
                }))
                .unwrap_or(Err(Error::Worker));
                // Child-owning locals have all been dropped before publication.
                shared.child_live.store(false, Ordering::Release);
                let _ = ready_tx.try_send(result.as_ref().map(|_| ()).map_err(Clone::clone));
                let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
                state.status = match &result {
                    Ok(_) => Status::Finished,
                    Err(e) => Status::Failed(e.clone()),
                };
                state.result = Some(result);
            })
            .map_err(|_| Error::Worker)?;
        let recorder = Self {
            runtime,
            worker: Some(worker),
            stop_wait,
        };
        match ready_rx.recv_timeout(startup_wait) {
            Ok(Ok(())) => Ok(recorder),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                recorder.runtime.cancel();
                Err(Error::Timeout)
            }
        }
    }
    pub fn status(&self) -> Status {
        self.runtime.status()
    }
    pub fn poll(&self) -> Option<Result<PathBuf>> {
        self.runtime
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .result
            .clone()
    }
    pub fn request_stop(&self) {
        self.runtime.request_stop();
    }
    pub fn stop(mut self) -> Result<PathBuf> {
        self.request_stop();
        let deadline = Instant::now() + self.stop_wait;
        loop {
            if let Some(result) = self.poll() {
                if self.worker.as_ref().is_some_and(|w| w.is_finished()) {
                    if let Some(w) = self.worker.take() {
                        let _ = w.join();
                    }
                    return result;
                }
            }
            if Instant::now() >= deadline {
                self.runtime.cancel();
                return Err(Error::Timeout);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    /// Irreversible process-shutdown gate. Rejects future starts, cancels
    /// consent waits without waiting for their dialog, and requests EOS for an
    /// existing child. Grace is capped at five seconds plus one second abort.
    /// The result only reports child cleanup, never a saved-file claim.
    pub fn shutdown_all(grace: Duration) -> bool {
        SHUTTING_DOWN.store(true, Ordering::Release);
        let runtime = ACTIVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        runtime.is_none_or(|runtime| shutdown_runtime(&runtime, grace))
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.runtime.cancel();
        if self.worker.as_ref().is_some_and(|w| w.is_finished()) {
            if let Some(w) = self.worker.take() {
                let _ = w.join();
            }
        }
    }
}
fn shutdown_runtime(runtime: &Runtime, grace: Duration) -> bool {
    match shutdown_action(&runtime.status()) {
        ShutdownAction::None => return !runtime.child_live.load(Ordering::Acquire),
        ShutdownAction::CancelStart => runtime.cancel(),
        ShutdownAction::FinishRecording => runtime.request_stop(),
    }
    if !runtime.child_live.load(Ordering::Acquire) {
        return true;
    }
    let deadline = Instant::now() + grace.min(Duration::from_secs(5));
    while runtime.child_live.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !runtime.child_live.load(Ordering::Acquire) {
        return true;
    }
    runtime.cancel();
    let deadline = Instant::now() + Duration::from_secs(1);
    while runtime.child_live.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    !runtime.child_live.load(Ordering::Acquire)
}
fn prepare_and_run(
    options: &Options,
    path: &Path,
    runtime: &Runtime,
    ready: &mpsc::SyncSender<Result<()>>,
) -> Result<PathBuf> {
    prepare_with_portal(options, path, runtime, ready, || {
        Session::open_with_cancel(options, &|| runtime.canceled())
    })
}
fn prepare_with_portal(
    options: &Options,
    path: &Path,
    runtime: &Runtime,
    ready: &mpsc::SyncSender<Result<()>>,
    open: impl FnOnce() -> Result<(Session, std::os::fd::OwnedFd)>,
) -> Result<PathBuf> {
    if runtime.canceled() {
        return Err(Error::Canceled);
    }
    let (mut session, remote) = open()?;
    if runtime.canceled() {
        return Err(Error::Canceled);
    }
    let output = Output::create(path)?;
    if runtime.canceled() {
        return Err(Error::Canceled);
    }
    runtime.child_live.store(true, Ordering::Release);
    // Pair this second guard with shutdown's observation of child_live: a
    // canceled consent worker cannot launch an encoder after shutdown returns.
    if runtime.canceled() {
        return Err(Error::Canceled);
    }
    // This monitor creates AND owns the encoder until it is reaped. The Linux
    // parent-death signal therefore follows this persistent thread's lifetime.
    let encoder = Encoder::launch(&remote, session.node, session.serial, options, &output)?;
    drop(remote);
    session.health()?;
    run(encoder, session, output, options, runtime, ready)
}
#[cfg(test)]
pub(crate) fn test_prepare_with_portal(
    options: &Options,
    path: &Path,
    cancellation: StartCancellation,
    open: impl FnOnce() -> Result<(Session, std::os::fd::OwnedFd)>,
) -> Result<PathBuf> {
    let runtime = Runtime {
        cancellation,
        ..Runtime::new()
    };
    let (ready, _) = mpsc::sync_channel(1);
    prepare_with_portal(options, path, &runtime, &ready, open)
}
fn run(
    mut encoder: Encoder,
    mut session: Session,
    mut output: Output,
    options: &Options,
    runtime: &Runtime,
    ready: &mpsc::SyncSender<Result<()>>,
) -> Result<PathBuf> {
    let start = Instant::now();
    let mut announced = false;
    let mut next_health = Instant::now();
    let result = (|| loop {
        let command = runtime.control.load(Ordering::Acquire);
        if command == 2 || runtime.cancellation.is_canceled() {
            return Err(Error::Canceled);
        }
        if output.len()? >= options.max_bytes {
            return Err(Error::Limit);
        }
        if Instant::now() >= next_health {
            session.health()?;
            next_health = Instant::now() + Duration::from_millis(500);
        }
        if let Some(success) = encoder.poll()? {
            if !success {
                return Err(Error::Encoder);
            }
            if !announced {
                return Err(Error::Finalize);
            }
            return output.commit();
        }
        if !announced && output.ready()? {
            announced = true;
            runtime
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .status = Status::Recording;
            let _ = ready.try_send(Ok(()));
        }
        if !announced && start.elapsed() >= options.startup_timeout {
            return Err(Error::Timeout);
        }
        if command == 1 || start.elapsed() >= options.max_duration {
            runtime
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .status = Status::Stopping;
            encoder.finish_with_abort(options.stop_timeout, || {
                runtime.control.load(Ordering::Acquire) == 2 || runtime.cancellation.is_canceled()
            })?;
            return output.commit();
        }
        thread::sleep(Duration::from_millis(20));
    })();
    // Close only after EOS/child termination so the muxer can finalize while the
    // approved source remains available. Any failure drops/kills the owned child.
    drop(encoder);
    runtime.child_live.store(false, Ordering::Release);
    let closed = session.close();
    match result {
        Ok(path) => {
            closed?;
            Ok(path)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    #[test]
    fn pending_portal_shutdown_does_not_wait_for_consent() {
        let runtime = Runtime::new();
        let start = Instant::now();
        assert!(shutdown_runtime(&runtime, Duration::from_secs(5)));
        assert_eq!(runtime.control.load(Ordering::Acquire), 2);
        assert!(start.elapsed() < Duration::from_millis(100));
    }
    #[test]
    fn active_shutdown_requests_eos_and_waits_for_child_cleanup() {
        let runtime = Arc::new(Runtime::new());
        runtime.state.lock().unwrap().status = Status::Recording;
        runtime.child_live.store(true, Ordering::Release);
        let copy = runtime.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(1);
            while copy.control.load(Ordering::Acquire) != 1 {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            copy.child_live.store(false, Ordering::Release);
        });
        assert!(shutdown_runtime(&runtime, Duration::from_secs(1)));
        worker.join().unwrap();
    }
    #[test]
    fn shutdown_never_downgrades_an_already_requested_abort() {
        let runtime = Runtime::new();
        runtime.state.lock().unwrap().status = Status::Recording;
        runtime.cancel();
        assert!(shutdown_runtime(&runtime, Duration::ZERO));
        assert_eq!(runtime.control.load(Ordering::Acquire), 2);
    }
}
