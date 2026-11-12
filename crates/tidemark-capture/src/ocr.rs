//! Offline Tesseract 5 subprocess, rus+eng only, without a shell or text upload.
use crate::{Error, Raster, Result};
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
const MAX_OUTPUT: usize = 1 << 20;
/// Recognize a validated image. Install tesseract-ocr, tesseract-ocr-rus and
/// tesseract-ocr-eng. A timeout kills and reaps the direct Tesseract child.
pub fn recognize(image: &Raster, timeout: Duration) -> Result<String> {
    if timeout.is_zero() {
        return Err(Error::Timeout);
    }
    let png = image.png()?;
    let mut input = tempfile::Builder::new()
        .prefix("tidemark-ocr-")
        .suffix(".png")
        .tempfile()
        .map_err(|_| Error::Ocr)?;
    input.write_all(&png).map_err(|_| Error::Ocr)?;
    input.flush().map_err(|_| Error::Ocr)?;
    let mut command = Command::new("tesseract");
    command
        .arg(input.path())
        .arg("stdout")
        .args(["-l", "rus+eng", "--oem", "1", "--psm", "3"])
        .env("OMP_THREAD_LIMIT", "2");
    run(command, timeout.min(Duration::from_secs(60)), MAX_OUTPUT)
}
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn run(mut command: Command, timeout: Duration, cap: usize) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::OcrUnavailable)?;
    let mut child = Running(child);
    let mut stdout = child.0.stdout.take().ok_or(Error::Ocr)?;
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(Error::Ocr);
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
            Err(_) => return Err(Error::Ocr),
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
        if let Some(status) = child.0.try_wait().map_err(|_| Error::Ocr)? {
            // Read all remaining buffered bytes after the child exited. A pipe
            // held open by a descendant is not accepted as complete output.
            if read == Some(0) {
                if !status.success() {
                    return Err(Error::Ocr);
                }
                return String::from_utf8(output)
                    .map(|s| s.trim().to_owned())
                    .map_err(|_| Error::Ocr);
            }
            // Exit may race the first nonblocking read; drain again on the next
            // iteration. A descendant retaining the pipe eventually times out.
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(mode: &str) -> Command {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "ocr::tests::process_fixture", "--nocapture"])
            .env("TIDEMARK_CAPTURE_FIXTURE", mode);
        c
    }
    #[test]
    fn process_fixture() {
        match std::env::var("TIDEMARK_CAPTURE_FIXTURE").ok().as_deref() {
            Some("timeout") => std::thread::sleep(Duration::from_secs(4)),
            Some("overflow") => {
                let _ = std::io::stdout().write_all(&vec![b'x'; MAX_OUTPUT + 8192]);
            }
            Some("success") => println!("Synthetic offline OCR text"),
            _ => (),
        }
    }
    #[test]
    fn subprocess_output_and_timeout_are_bounded() {
        assert!(run(fixture("success"), Duration::from_secs(2), MAX_OUTPUT)
            .unwrap()
            .contains("Synthetic offline OCR text"));
        assert_eq!(
            run(fixture("overflow"), Duration::from_secs(2), 1024),
            Err(Error::Limit)
        );
        let start = Instant::now();
        assert_eq!(
            run(fixture("timeout"), Duration::from_millis(100), MAX_OUTPUT),
            Err(Error::Timeout)
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    #[ignore = "requires installed Tesseract with rus+eng; synthetic blank image only"]
    fn installed_tesseract_reads_synthetic_image_offline() {
        let image = Raster::from_rgba(128, 64, vec![255; 128 * 64 * 4]).unwrap();
        assert_eq!(recognize(&image, Duration::from_secs(15)).unwrap(), "");
    }
}
