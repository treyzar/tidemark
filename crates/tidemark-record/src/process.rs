//! Owned encoder process, private output descriptor and bounded shutdown.
use crate::{Error, Options, Result};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, ChildStderr, Command, Stdio},
    time::{Duration, Instant},
};
pub(crate) const EXECUTABLE: &str = "/usr/bin/gst-launch-1.0";
const STDERR_CAP: usize = 64 * 1024;
pub(crate) struct Output {
    file: File,
    path: PathBuf,
    dev: u64,
    ino: u64,
    committed: bool,
}
impl Output {
    pub fn create(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| Error::Output)?;
        let m = file.metadata().map_err(|_| Error::Output)?;
        if !m.is_file() {
            return Err(Error::Output);
        }
        Ok(Self {
            file,
            path: path.into(),
            dev: m.dev(),
            ino: m.ino(),
            committed: false,
        })
    }
    pub fn len(&self) -> Result<u64> {
        self.file
            .metadata()
            .map(|m| m.len())
            .map_err(|_| Error::Output)
    }
    fn same_path(&self) -> bool {
        std::fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.is_file() && m.dev() == self.dev && m.ino() == self.ino)
    }
    pub fn ready(&self) -> Result<bool> {
        use std::os::unix::fs::FileExt;
        let mut bytes = vec![0u8; 65536];
        let n = self
            .file
            .read_at(&mut bytes, 0)
            .map_err(|_| Error::Output)?;
        bytes.truncate(n);
        Ok(bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
            && bytes.windows(4).any(|w| w == b"webm")
            && bytes.windows(4).any(|w| w == [0x1f, 0x43, 0xb6, 0x75]))
    }
    pub fn commit(&mut self) -> Result<PathBuf> {
        if !self.same_path() || !self.ready()? {
            return Err(Error::Finalize);
        }
        self.file.sync_all().map_err(|_| Error::Output)?;
        self.committed = true;
        Ok(self.path.clone())
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        if !self.committed && self.same_path() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
pub(crate) struct Encoder {
    child: Child,
    stderr: ChildStderr,
    diagnostic_bytes: usize,
    reaped: bool,
}
impl Encoder {
    pub fn launch(
        remote: &OwnedFd,
        node: u32,
        serial: Option<u64>,
        options: &Options,
        output: &Output,
    ) -> Result<Self> {
        // Duplicate into an occupied high descriptor. Do not dup2 over Rust's private
        // exec-error pipe or mutate CLOEXEC in the parent's descriptor table.
        let raw = unsafe { libc::fcntl(remote.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
        if raw < 0 {
            return Err(Error::Transport);
        }
        let staged = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut command = Command::new(EXECUTABLE);
        command.args(crate::pipeline(raw, node, serial, options.fps)?);
        let encoder = Self::spawn(command, Some(&staged), options, output);
        drop(staged);
        encoder
    }
    fn spawn(
        mut command: Command,
        remote: Option<&OwnedFd>,
        options: &Options,
        output: &Output,
    ) -> Result<Self> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                output.file.try_clone().map_err(|_| Error::Output)?,
            ))
            .stderr(Stdio::piped())
            .process_group(0)
            .env_remove("GST_DEBUG")
            .env_remove("GST_DEBUG_FILE")
            .env_remove("GST_DEBUG_DUMP_DOT_DIR")
            .env_remove("GST_TRACERS");
        let raw = remote.map(AsRawFd::as_raw_fd);
        let max = options.max_bytes;
        let parent_pid = unsafe { libc::getpid() };
        unsafe {
            command.pre_exec(move || {
                // Recorder creates the child from the persistent monitor
                // thread, which owns it through reaping. Linux ties this
                // signal to that creator THREAD, not just its process ID.
                if libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGKILL as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                    0 as libc::c_ulong,
                ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                // Covers parent-process death between fork and prctl.
                if libc::getppid() != parent_pid {
                    return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
                }
                if let Some(fd) = raw {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                let limit = libc::rlimit {
                    rlim_cur: max as libc::rlim_t,
                    rlim_max: max as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|_| Error::Encoder)?;
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Encoder);
            }
        };
        let mut encoder = Self {
            child,
            stderr,
            diagnostic_bytes: 0,
            reaped: false,
        };
        let fd = encoder.stderr.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(Error::Encoder);
        }
        encoder.drain()?;
        Ok(encoder)
    }
    fn drain(&mut self) -> Result<()> {
        let mut chunk = [0u8; 4096];
        for _ in 0..16 {
            match self.stderr.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    self.diagnostic_bytes = self.diagnostic_bytes.saturating_add(n);
                    if self.diagnostic_bytes > STDERR_CAP {
                        return Err(Error::Limit);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Encoder),
            }
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Result<Option<bool>> {
        self.drain()?;
        let status = self.child.try_wait().map_err(|_| Error::Encoder)?;
        if let Some(status) = status {
            self.reaped = true;
            return Ok(Some(status.success()));
        }
        Ok(None)
    }
    #[cfg(test)]
    pub fn finish(&mut self, timeout: Duration) -> Result<()> {
        self.finish_with_abort(timeout, || false)
    }
    pub fn finish_with_abort(&mut self, timeout: Duration, abort: impl Fn() -> bool) -> Result<()> {
        if abort() {
            self.kill();
            return Err(Error::Canceled);
        }
        if let Some(success) = self.poll()? {
            return if success { Ok(()) } else { Err(Error::Encoder) };
        }
        let pid = i32::try_from(self.child.id()).map_err(|_| Error::Encoder)?;
        if unsafe { libc::kill(-pid, libc::SIGINT) } != 0 {
            return Err(Error::Encoder);
        }
        let deadline = Instant::now() + timeout;
        loop {
            if abort() {
                self.kill();
                return Err(Error::Canceled);
            }
            if let Some(success) = self.poll()? {
                return if success {
                    Ok(())
                } else {
                    Err(Error::Finalize)
                };
            }
            if Instant::now() >= deadline {
                self.kill();
                return Err(Error::Timeout);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn kill(&mut self) {
        if !self.reaped {
            if let Ok(pid) = i32::try_from(self.child.id()) {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.reaped = true;
        }
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::unix::net::UnixStream,
        sync::atomic::{AtomicBool, Ordering},
    };
    static INTERRUPTED: AtomicBool = AtomicBool::new(false);
    extern "C" fn interrupt(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::Release);
    }
    #[test]
    fn child_fixture() {
        let Ok(mode) = std::env::var("TIDEMARK_RECORD_FIXTURE") else {
            return;
        };
        unsafe {
            libc::ftruncate(1, 0);
            libc::lseek(1, 0, libc::SEEK_SET);
        }
        if mode == "ignore" {
            unsafe {
                libc::signal(libc::SIGINT, libc::SIG_IGN);
            }
        } else {
            unsafe {
                libc::signal(libc::SIGINT, interrupt as *const () as libc::sighandler_t);
            }
        }
        if let Ok(fd) = std::env::var("TIDEMARK_RECORD_FD") {
            let fd = fd.parse::<i32>().unwrap();
            assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
        }
        let mut stdout = std::io::stdout();
        stdout
            .write_all(b"\x1a\x45\xdf\xa3synthetic-webm\x1f\x43\xb6\x75synthetic-frame")
            .unwrap();
        stdout.flush().unwrap();
        if mode == "error" {
            std::process::exit(1);
        }
        if mode == "stderr" {
            let _ = std::io::stderr().write_all(&vec![b'x'; STDERR_CAP + 8192]);
        }
        let start = Instant::now();
        while !INTERRUPTED.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(8) {
            std::thread::sleep(Duration::from_millis(5));
        }
        stdout.write_all(b"synthetic-finalized").unwrap();
        stdout.flush().unwrap();
        std::process::exit(0);
    }
    fn fixture(mode: &str, remote: Option<&OwnedFd>) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "process::tests::child_fixture", "--nocapture"])
            .env("TIDEMARK_RECORD_FIXTURE", mode);
        if let Some(fd) = remote {
            cmd.env("TIDEMARK_RECORD_FD", fd.as_raw_fd().to_string());
        }
        cmd
    }
    fn wait_ready(output: &Output, encoder: &mut Encoder) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !output.ready().unwrap() {
            assert!(encoder.poll().unwrap().is_none());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn fixed_descriptor_handoff_and_sigint_flush_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("name ! filesink location=untrusted.webm");
        let mut output = Output::create(&path).unwrap();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let raw = unsafe { libc::fcntl(socket.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
        assert!(raw >= 64);
        let staged = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut encoder = Encoder::spawn(
            fixture("graceful", Some(&staged)),
            Some(&staged),
            &Options::default(),
            &output,
        )
        .unwrap();
        // The child may inherit the approved remote, but launching it must
        // not make the parent's descriptor leak into unrelated future execs.
        assert_ne!(
            unsafe { libc::fcntl(staged.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(staged);
        wait_ready(&output, &mut encoder);
        encoder.finish(Duration::from_secs(2)).unwrap();
        assert_eq!(output.commit().unwrap(), path);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(std::fs::read(&path)
            .unwrap()
            .ends_with(b"synthetic-finalized"));
    }
    #[test]
    fn forced_stop_and_stderr_limit_never_commit_output() {
        for mode in ["ignore", "stderr"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("partial.webm");
            {
                let output = Output::create(&path).unwrap();
                let result =
                    Encoder::spawn(fixture(mode, None), None, &Options::default(), &output);
                if mode == "stderr" {
                    if let Ok(mut encoder) = result {
                        let deadline = Instant::now() + Duration::from_secs(2);
                        loop {
                            match encoder.poll() {
                                Err(Error::Limit) => break,
                                Ok(_) => {}
                                Err(e) => panic!("unexpected {e}"),
                            }
                            assert!(Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                } else {
                    let mut encoder = result.unwrap();
                    wait_ready(&output, &mut encoder);
                    let start = Instant::now();
                    assert_eq!(
                        encoder.finish(Duration::from_millis(100)),
                        Err(Error::Timeout)
                    );
                    assert!(start.elapsed() < Duration::from_secs(2));
                }
            }
            assert!(!path.exists());
        }
    }
    #[test]
    fn output_never_overwrites_or_deletes_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.webm");
        std::fs::write(&path, b"existing").unwrap();
        assert!(matches!(Output::create(&path), Err(Error::Output)));
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        std::fs::remove_file(&path).unwrap();
        let output = Output::create(&path).unwrap();
        std::fs::rename(&path, dir.path().join("moved.webm")).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        drop(output);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }
    #[test]
    fn failed_encoder_cannot_be_treated_as_finalized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("failed.webm");
        {
            let output = Output::create(&path).unwrap();
            let mut encoder =
                Encoder::spawn(fixture("error", None), None, &Options::default(), &output).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(success) = encoder.poll().unwrap() {
                    assert!(!success);
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(encoder.finish(Duration::from_secs(1)), Err(Error::Encoder));
        }
        assert!(!path.exists());
    }
    #[test]
    fn shutdown_abort_interrupts_an_encoder_ignoring_eos() {
        let dir = tempfile::tempdir().unwrap();
        let output = Output::create(&dir.path().join("shutdown-abort.webm")).unwrap();
        let mut encoder =
            Encoder::spawn(fixture("ignore", None), None, &Options::default(), &output).unwrap();
        wait_ready(&output, &mut encoder);
        let abort = std::sync::Arc::new(AtomicBool::new(false));
        let signal = abort.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            signal.store(true, Ordering::Release);
        });
        let start = Instant::now();
        assert_eq!(
            encoder.finish_with_abort(Duration::from_secs(5), || abort.load(Ordering::Acquire)),
            Err(Error::Canceled)
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        setter.join().unwrap();
    }
    #[test]
    fn creator_thread_death_kills_even_an_intentionally_unreaped_encoder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parent-death.webm");
        let (tx, rx) = std::sync::mpsc::channel();
        let creator = std::thread::spawn(move || {
            let output = Output::create(&path).unwrap();
            let mut encoder =
                Encoder::spawn(fixture("ignore", None), None, &Options::default(), &output)
                    .unwrap();
            wait_ready(&output, &mut encoder);
            let pid = encoder.child.id() as libc::pid_t;
            // Bypass RAII deliberately: this test proves the kernel lifetime
            // guard, not Encoder::drop's explicit SIGKILL.
            std::mem::forget(encoder);
            tx.send(pid).unwrap();
        });
        let pid = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        creator.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut status = 0;
            let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if waited == pid {
                assert!(libc::WIFSIGNALED(status));
                assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
                break;
            }
            assert!(waited >= 0, "owned child was not waitable");
            if Instant::now() >= deadline {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                    libc::waitpid(pid, &mut status, 0);
                }
                panic!("encoder survived its creator thread");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn caller_thread_can_exit_while_monitor_owns_the_encoder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("monitor-owned.webm");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let caller = std::thread::spawn(move || {
            std::thread::spawn(move || {
                let mut output = Output::create(&path).unwrap();
                let mut encoder = Encoder::spawn(
                    fixture("graceful", None),
                    None,
                    &Options::default(),
                    &output,
                )
                .unwrap();
                wait_ready(&output, &mut encoder);
                started_tx.send(encoder.child.id() as libc::pid_t).unwrap();
                stop_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                encoder.finish(Duration::from_secs(2)).unwrap();
                output.commit().unwrap();
            })
        });
        let monitor = caller.join().unwrap();
        let pid = started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            0,
            "caller exit killed a monitor-owned child"
        );
        stop_tx.send(()).unwrap();
        monitor.join().unwrap();
    }
    #[test]
    #[ignore = "requires installed GStreamer Good/base plugins; synthetic videotestsrc only"]
    fn installed_gstreamer_encodes_only_synthetic_video() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.webm");
        let mut output = Output::create(&path).unwrap();
        let mut command = Command::new(EXECUTABLE);
        command.args([
            "-q",
            "-e",
            "-f",
            "--no-position",
            "videotestsrc",
            "is-live=true",
            "!",
            "videoconvert",
            "!",
            "video/x-raw,format=I420,width=160,height=120,framerate=30/1",
            "!",
            "vp8enc",
            "deadline=1",
            "!",
            "webmmux",
            "!",
            "fdsink",
            "fd=1",
            "sync=false",
        ]);
        let mut encoder = Encoder::spawn(command, None, &Options::default(), &output).unwrap();
        wait_ready(&output, &mut encoder);
        encoder.finish(Duration::from_secs(5)).unwrap();
        output.commit().unwrap();
        assert!(std::fs::metadata(path).unwrap().len() > 128);
    }
}
