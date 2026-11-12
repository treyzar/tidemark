use crate::{Error, Result};
use std::{
    io::Read,
    os::fd::AsRawFd,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
pub(crate) fn run(mut command: Command, timeout: Duration, cap: usize) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::Unsupported,
            std::io::ErrorKind::PermissionDenied => Error::Denied,
            _ => Error::Transport,
        })?;
    let mut child = Running(child);
    let mut stdout = child.0.stdout.take().ok_or(Error::Transport)?;
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(Error::Transport);
    }
    let mut output = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        let read = match stdout.read(&mut chunk) {
            Ok(n) => Some(n),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Error::Transport),
        };
        if let Some(n) = read {
            if n > cap - output.len() {
                return Err(Error::Limit);
            }
            output.extend_from_slice(&chunk[..n]);
            if n > 0 {
                continue;
            }
        }
        if let Some(status) = child.0.try_wait().map_err(|_| Error::Transport)? {
            // Read all remaining buffered bytes after the child exited. A pipe
            // held open by a descendant is not accepted as complete output.
            if read == Some(0) {
                if !status.success() {
                    return Err(if matches!(status.code(), Some(2 | 3)) {
                        Error::Unsupported
                    } else {
                        Error::Transport
                    });
                }
                return String::from_utf8(output)
                    .map(|s| s.trim().to_owned())
                    .map_err(|_| Error::Transport);
            }
            // Exit may race the first nonblocking read; drain again on the next
            // iteration. A descendant retaining the pipe eventually times out.
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(crate) fn read_text(path: &Path, cap: usize) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::Unsupported,
        std::io::ErrorKind::PermissionDenied => Error::Denied,
        _ => Error::Transport,
    })?;
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Transport)?;
    if bytes.len() > cap {
        return Err(Error::Limit);
    }
    String::from_utf8(bytes).map_err(|_| Error::InvalidResponse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn fixture_process() {
        match std::env::var("TIDEMARK_SYSTEM_PROCESS_FIXTURE")
            .ok()
            .as_deref()
        {
            Some("success") => println!("SYNTHETIC_WPCTL_RESPONSE"),
            Some("overflow") => {
                let _ = std::io::stdout().write_all(&vec![b'x'; 131072]);
            }
            Some("timeout") => std::thread::sleep(Duration::from_secs(10)),
            _ => (),
        }
    }
    fn fixture(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "process::tests::fixture_process", "--nocapture"])
            .env("TIDEMARK_SYSTEM_PROCESS_FIXTURE", mode);
        command
    }
    #[test]
    fn subprocess_output_and_lifetime_are_bounded_without_a_shell() {
        assert!(run(fixture("success"), Duration::from_secs(2), 8192)
            .unwrap()
            .contains("SYNTHETIC_WPCTL_RESPONSE"));
        assert_eq!(
            run(fixture("overflow"), Duration::from_secs(2), 1024),
            Err(Error::Limit)
        );
        let start = Instant::now();
        assert_eq!(
            run(fixture("timeout"), Duration::from_millis(100), 8192),
            Err(Error::Timeout)
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
