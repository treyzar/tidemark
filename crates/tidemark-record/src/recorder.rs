use crate::{
    portal::Session,
    process::{Encoder, Output},
    Error, Options, Result, Status,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
static BUSY: AtomicBool = AtomicBool::new(false);
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
/// Blocking start/stop belong on a UI worker. poll/status are nonblocking.
/// Dropping requests abort; only stop() or a successful poll proves a saved file.
pub struct Recorder {
    state: Arc<Mutex<Progress>>,
    control: Arc<AtomicU8>,
    worker: Option<JoinHandle<()>>,
    stop_wait: Duration,
}
impl Recorder {
    pub fn start(options: Options, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        options.validate(path)?;
        if path.exists() {
            return Err(Error::Output);
        }
        let permit = Permit::acquire()?;
        if !std::fs::metadata(crate::process::EXECUTABLE).is_ok_and(|m| m.is_file()) {
            return Err(Error::Encoder);
        }
        let (mut session, remote) = Session::open(&options)?;
        let output = Output::create(path)?;
        let encoder = Encoder::launch(&remote, session.node, session.serial, &options, &output)?;
        drop(remote);
        session.health()?;
        let state = Arc::new(Mutex::new(Progress {
            status: Status::Starting,
            result: None,
        }));
        let shared = state.clone();
        let control = Arc::new(AtomicU8::new(0));
        let flag = control.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let startup_wait = options.startup_timeout + Duration::from_secs(2);
        let stop_wait = options.stop_timeout + Duration::from_secs(4);
        let worker = thread::Builder::new()
            .name("screen-recording".into())
            .spawn(move || {
                let _permit = permit;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(
                        encoder, session, output, &options, &flag, &shared, &ready_tx,
                    )
                }))
                .unwrap_or(Err(Error::Worker));
                let _ = ready_tx.try_send(result.as_ref().map(|_| ()).map_err(Clone::clone));
                let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
                state.status = match &result {
                    Ok(_) => Status::Finished,
                    Err(e) => Status::Failed(e.clone()),
                };
                state.result = Some(result);
            })
            .map_err(|_| Error::Worker)?;
        let recorder = Self {
            state,
            control,
            worker: Some(worker),
            stop_wait,
        };
        match ready_rx.recv_timeout(startup_wait) {
            Ok(Ok(())) => Ok(recorder),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                recorder.control.store(2, Ordering::Release);
                Err(Error::Timeout)
            }
        }
    }
    pub fn status(&self) -> Status {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .status
            .clone()
    }
    pub fn poll(&self) -> Option<Result<PathBuf>> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .result
            .clone()
    }
    pub fn request_stop(&self) {
        let _ = self
            .control
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
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
                self.control.store(2, Ordering::Release);
                return Err(Error::Timeout);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.control.store(2, Ordering::Release);
        if self.worker.as_ref().is_some_and(|w| w.is_finished()) {
            if let Some(w) = self.worker.take() {
                let _ = w.join();
            }
        }
    }
}
fn run(
    mut encoder: Encoder,
    mut session: Session,
    mut output: Output,
    options: &Options,
    control: &AtomicU8,
    state: &Mutex<Progress>,
    ready: &mpsc::SyncSender<Result<()>>,
) -> Result<PathBuf> {
    let start = Instant::now();
    let mut announced = false;
    let mut next_health = Instant::now();
    let result = (|| loop {
        let command = control.load(Ordering::Acquire);
        if command == 2 {
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
            state.lock().unwrap_or_else(|p| p.into_inner()).status = Status::Recording;
            let _ = ready.try_send(Ok(()));
        }
        if !announced && start.elapsed() >= options.startup_timeout {
            return Err(Error::Timeout);
        }
        if command == 1 || start.elapsed() >= options.max_duration {
            state.lock().unwrap_or_else(|p| p.into_inner()).status = Status::Stopping;
            encoder.finish(options.stop_timeout)?;
            return output.commit();
        }
        thread::sleep(Duration::from_millis(20));
    })();
    // Close only after EOS/child termination so the muxer can finalize while the
    // approved source remains available. Any failure drops/kills the owned child.
    drop(encoder);
    let closed = session.close();
    match result {
        Ok(path) => {
            closed?;
            Ok(path)
        }
        Err(error) => Err(error),
    }
}
