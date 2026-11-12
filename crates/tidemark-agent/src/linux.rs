//! Same-user, bounded Unix socket bridge. Text is untrusted: render only as plain text.
use crate::{
    protocol::{self, MAX_REPLY, MAX_REQUEST, MAX_WAIT},
    Message, Reply,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
fn denied() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "unsafe agent endpoint")
}
fn uid() -> u32 {
    unsafe { libc::geteuid() }
}
fn check_dir(p: &Path, expected: u32) -> io::Result<()> {
    if !p.is_absolute() {
        return Err(denied());
    }
    let mut current = PathBuf::new();
    for c in p.components() {
        if matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        ) {
            return Err(denied());
        }
        current.push(c);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(denied());
        }
    }
    let m = fs::symlink_metadata(p)?;
    if !m.is_dir() || m.uid() != expected || m.mode() & 0o777 != 0o700 {
        return Err(denied());
    }
    Ok(())
}
/// Resolve explicit XDG runtime input or the standard per-UID fallback, without changing environment.
pub fn resolve_runtime(xdg: Option<&Path>, expected: u32) -> io::Result<PathBuf> {
    let p = xdg
        .map(Path::to_owned)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{expected}")));
    check_dir(&p, expected)?;
    Ok(p)
}
struct Directory {
    _runtime: File,
    dir: File,
}
impl Directory {
    fn open(runtime: &Path, create: bool) -> io::Result<Self> {
        check_dir(runtime, uid())?;
        let r = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(runtime)?;
        validate_dir_file(&r)?;
        let name = c"tidemark";
        if create {
            let n = unsafe { libc::mkdirat(r.as_raw_fd(), name.as_ptr(), 0o700) };
            if n < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(io::Error::last_os_error());
            }
        }
        let fd = unsafe {
            libc::openat(
                r.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let dir = unsafe { File::from_raw_fd(fd) };
        validate_dir_file(&dir)?;
        Ok(Self { _runtime: r, dir })
    }
    fn path(&self, name: &str) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}/{}", self.dir.as_raw_fd(), name))
    }
}
fn validate_dir_file(f: &File) -> io::Result<()> {
    let m = f.metadata()?;
    if !m.is_dir() || m.uid() != uid() || m.mode() & 0o777 != 0o700 {
        Err(denied())
    } else {
        Ok(())
    }
}
/// Verify the kernel-authenticated peer's effective UID (both directions).
pub fn verify_peer(s: &UnixStream) -> io::Result<()> {
    verify_peer_uid(s, uid())
}
fn verify_peer_uid(s: &UnixStream, expected: u32) -> io::Result<()> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: !0,
        gid: !0,
    };
    let mut len = std::mem::size_of_val(&cred) as libc::socklen_t;
    let n = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if n != 0 {
        return Err(io::Error::last_os_error());
    }
    if len as usize != std::mem::size_of_val(&cred) || cred.uid != expected {
        return Err(denied());
    }
    Ok(())
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub request_timeout: Duration,
    pub answer_timeout: Duration,
    pub max_clients: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(3),
            answer_timeout: MAX_WAIT,
            max_clients: 16,
        }
    }
}
pub enum Incoming {
    Notice(Message),
    Ask {
        id: u64,
        msg: Message,
        reply: Sender<Option<usize>>,
    },
    Gone(u64),
}
pub struct Server {
    events: Receiver<Incoming>,
    pending: crate::pending::Pending,
    stop: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
}
impl Server {
    pub fn bind(runtime: &Path, limits: Limits) -> io::Result<Self> {
        let directory = Directory::open(runtime, true)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(directory.path("agent.lock"))?;
        let lm = lock.metadata()?;
        if !lm.is_file() || lm.uid() != uid() || lm.mode() & 0o777 != 0o600 || lm.nlink() != 1 {
            return Err(denied());
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "agent already running",
            ));
        }
        let path = directory.path("agent.sock");
        match fs::symlink_metadata(&path) {
            Ok(m) => {
                if !m.file_type().is_socket() || m.uid() != uid() {
                    return Err(denied());
                }
                match connect(&path, Duration::from_millis(100)) {
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "agent already running",
                        ))
                    }
                    Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED) => {}
                    Err(e) => return Err(e),
                }
                remove_same(&path, m.dev(), m.ino())?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let m = fs::symlink_metadata(&path)?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (tx, rx) = mpsc::sync_channel(64);
        let pending = crate::pending::Pending::default();
        let listener_pending = pending.clone();
        let task = thread::Builder::new()
            .name("agent-listener".into())
            .spawn(move || {
                let mut workers: Vec<JoinHandle<()>> = Vec::new();
                let mut id = 0;
                while !flag.load(Ordering::Relaxed) {
                    let mut i = 0;
                    while i < workers.len() {
                        if workers[i].is_finished() {
                            let _ = workers.swap_remove(i).join();
                        } else {
                            i += 1;
                        }
                    }
                    match listener.accept() {
                        Ok((s, _)) => {
                            if workers.len() >= limits.max_clients.clamp(1, 64) {
                                continue;
                            }
                            id += 1;
                            let tx = tx.clone();
                            let stop = flag.clone();
                            let pending = listener_pending.clone();
                            if let Ok(t) = thread::Builder::new().name("agent-client".into()).spawn(
                                move || {
                                    let _ = handle(s, id, &tx, stop, limits, pending);
                                },
                            ) {
                                workers.push(t);
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
                for w in workers {
                    let _ = w.join();
                }
                drop(listener);
                let _ = remove_same(&path, m.dev(), m.ino());
                drop(lock);
                drop(directory);
            })?;
        Ok(Self {
            events: rx,
            pending,
            stop,
            task: Some(task),
        })
    }
    pub fn try_recv(&self) -> Option<Incoming> {
        if let Some(id) = self.pending.canceled_delivered() {
            return Some(Incoming::Gone(id));
        }
        let event = self.events.try_recv().ok()?;
        if let Incoming::Ask { id, .. } = &event {
            if !self.pending.deliver(*id) {
                return Some(Incoming::Gone(*id));
            }
        }
        Some(event)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.task.take() {
            let _ = t.join();
        }
    }
}
fn remove_same(path: &Path, dev: u64, ino: u64) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if !m.file_type().is_socket() || m.uid() != uid() || m.dev() != dev || m.ino() != ino {
        return Err(denied());
    }
    fs::remove_file(path)
}
struct Timed {
    stream: UnixStream,
    deadline: Instant,
    stop: Arc<AtomicBool>,
}
impl Timed {
    fn check(&self) -> io::Result<()> {
        if self.stop.load(Ordering::Relaxed) || Instant::now() >= self.deadline {
            Err(io::Error::new(io::ErrorKind::TimedOut, "agent wait ended"))
        } else {
            Ok(())
        }
    }
}
impl Read for Timed {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.stream.read(b) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                r => return r,
            }
        }
    }
}
impl Write for Timed {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.stream.write(b) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                r => return r,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn connect(path: &Path, wait: Duration) -> io::Result<UnixStream> {
    use std::os::unix::ffi::OsStrExt;
    let name = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if name.len() >= addr.sun_path.len() {
        return Err(protocol::invalid());
    }
    addr.sun_family = libc::AF_UNIX as _;
    for (out, input) in addr.sun_path.iter_mut().zip(name) {
        *out = *input as _;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let s = unsafe { UnixStream::from_raw_fd(fd) };
    let n = unsafe {
        libc::connect(
            fd,
            (&addr as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&addr) as _,
        )
    };
    if n < 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(e);
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let r = unsafe { libc::poll(&mut poll, 1, wait.as_millis().min(3000) as i32) };
        if r <= 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "agent connect ended",
            ));
        }
        if let Some(e) = s.take_error()? {
            return Err(e);
        }
    }
    verify_peer(&s)?;
    Ok(s)
}
/// One exchange; errors mean terminal fallback. No policy decision is synthesized.
pub fn send(runtime: &Path, msg: &Message, wait: Duration) -> io::Result<Option<usize>> {
    let d = Directory::open(runtime, false)?;
    let path = d.path("agent.sock");
    let m = fs::symlink_metadata(&path)?;
    if !m.file_type().is_socket() || m.uid() != uid() || m.mode() & 0o777 != 0o600 {
        return Err(denied());
    }
    let wait = wait.min(MAX_WAIT);
    let deadline = Instant::now() + wait;
    let stream = connect(&path, wait)?;
    let mut io = Timed {
        stream,
        deadline,
        stop: Arc::new(AtomicBool::new(false)),
    };
    io.write_all(&protocol::encode(msg)?)?;
    if msg.kind != "ask" {
        return Ok(None);
    }
    let bytes = protocol::frame(&mut io, MAX_REPLY)?;
    let reply: Reply = serde_json::from_slice(&bytes).map_err(|_| protocol::invalid())?;
    Ok(protocol::validate_choice(reply.choice, msg.options.len()))
}
fn handle(
    stream: UnixStream,
    id: u64,
    tx: &SyncSender<Incoming>,
    stop: Arc<AtomicBool>,
    limits: Limits,
    pending: crate::pending::Pending,
) -> io::Result<()> {
    verify_peer(&stream)?;
    stream.set_nonblocking(true)?;
    let mut io = Timed {
        stream,
        deadline: Instant::now() + limits.request_timeout.min(Duration::from_secs(10)),
        stop,
    };
    let bytes = protocol::frame(&mut io, MAX_REQUEST)?;
    let msg: Message = serde_json::from_slice(&bytes).map_err(|_| protocol::invalid())?;
    if msg.kind == "notice" {
        tx.try_send(Incoming::Notice(msg))
            .map_err(|_| protocol::invalid())?;
        return Ok(());
    }
    if msg.kind != "ask" {
        return Err(protocol::invalid());
    }
    let count = msg.options.len();
    let (reply, rx) = mpsc::channel();
    let registration = pending.register(id).ok_or_else(protocol::invalid)?;
    if tx.try_send(Incoming::Ask { id, msg, reply }).is_err() {
        registration.complete();
        return Err(protocol::invalid());
    }
    io.deadline = Instant::now() + limits.answer_timeout.min(MAX_WAIT);
    let choice = loop {
        if io.check().is_err() {
            return Ok(());
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(c) => break protocol::validate_choice(c, count),
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // Any additional bytes are a protocol violation; EOF means the hook went away.
        let mut byte = [0];
        match (&io.stream).read(&mut byte) {
            Ok(_) => {
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => {
                return Ok(());
            }
        }
    };
    registration.complete();
    io.deadline = Instant::now() + Duration::from_millis(250);
    let mut bytes = serde_json::to_vec(&Reply { choice }).map_err(|_| protocol::invalid())?;
    bytes.push(b'\n');
    io.write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_peer_uid_mismatch_is_rejected() {
        let (a, _b) = UnixStream::pair().unwrap();
        verify_peer_uid(&a, uid()).unwrap();
        assert_eq!(
            verify_peer_uid(&a, uid().wrapping_add(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}

#[cfg(test)]
mod cancellation_regressions {
    use super::*;
    #[test]
    fn disconnected_ask_cancellation_survives_full_event_queue() {
        for delivered in [false, true] {
            let (mut client, stream) = UnixStream::pair().unwrap();
            let (tx, rx) = mpsc::sync_channel(64);
            let pending = crate::pending::Pending::default();
            let stop = Arc::new(AtomicBool::new(false));
            let server = Server {
                events: rx,
                pending: pending.clone(),
                stop: stop.clone(),
                task: None,
            };
            let worker_tx = tx.clone();
            let worker = thread::spawn(move || {
                handle(stream, 7, &worker_tx, stop, Limits::default(), pending)
            });
            let message = Message {
                kind: "ask".into(),
                source: "test".into(),
                title: "test".into(),
                text: "test".into(),
                options: vec!["yes".into()],
            };
            client
                .write_all(&protocol::encode(&message).unwrap())
                .unwrap();
            // Receive and put back the real handler's Ask: this is a synchronization
            // barrier proving the Ask is queued before we fill the remaining slots.
            let ask = server.events.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(tx.try_send(ask).is_ok());
            let visible = if delivered {
                match server.try_recv().unwrap() {
                    Incoming::Ask { reply, .. } => Some(reply),
                    _ => panic!("expected ask"),
                }
            } else {
                None
            };
            for _ in 0..if delivered { 64 } else { 63 } {
                assert!(tx.try_send(Incoming::Notice(message.clone())).is_ok());
            }
            assert!(tx.try_send(Incoming::Notice(message)).is_err());
            drop(client);
            let start = Instant::now();
            while !worker.is_finished() {
                assert!(start.elapsed() < Duration::from_secs(2));
                thread::sleep(Duration::from_millis(5));
            }
            worker.join().unwrap().unwrap();
            // Delivered cancellation is prioritized over 64 notices; queued
            // cancellation replaces Ask, so a stale actionable Ask never escapes.
            assert!(matches!(server.try_recv(), Some(Incoming::Gone(7))));
            while let Some(event) = server.try_recv() {
                assert!(matches!(event, Incoming::Notice(_)));
            }
            drop(visible);
            let start = Instant::now();
            drop(server);
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }
}
