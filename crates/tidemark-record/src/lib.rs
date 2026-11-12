//! ScreenCast portal + owned GStreamer WebM recording. Constructors never record.
use std::{path::Path, time::Duration};
use thiserror::Error;
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("another recording is active")]
    Busy,
    #[error("invalid recording options or output path")]
    Options,
    #[error("system audio recording is not supported; no microphone is selected")]
    AudioUnsupported,
    #[error("recording portal transport unavailable")]
    Transport,
    #[error("recording portal owner changed")]
    OwnerChanged,
    #[error("unexpected portal request or session path")]
    Path,
    #[error("recording was canceled by the user")]
    Canceled,
    #[error("recording portal rejected the request")]
    Portal,
    #[error("the recording session was closed or revoked")]
    Closed,
    #[error("recording operation timed out")]
    Timeout,
    #[error("recording resource limit exceeded")]
    Limit,
    #[error("GStreamer or required recording plug-ins are unavailable")]
    Encoder,
    #[error("the encoder did not finalize a valid WebM recording")]
    Finalize,
    #[error("recording output is unavailable; existing files are never overwritten")]
    Output,
    #[error("recording worker unavailable")]
    Worker,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sources {
    Monitor,
    Window,
    Any,
}
impl Sources {
    pub fn bits(self) -> u32 {
        match self {
            Self::Monitor => 1,
            Self::Window => 2,
            Self::Any => 3,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Options {
    pub parent_window: String,
    pub sources: Sources,
    pub show_cursor: bool,
    pub audio: bool,
    pub fps: u32,
    pub portal_timeout: Duration,
    pub startup_timeout: Duration,
    pub stop_timeout: Duration,
    pub max_duration: Duration,
    pub max_bytes: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            parent_window: String::new(),
            sources: Sources::Any,
            show_cursor: true,
            audio: false,
            fps: 30,
            portal_timeout: Duration::from_secs(60),
            startup_timeout: Duration::from_secs(5),
            stop_timeout: Duration::from_secs(5),
            max_duration: Duration::from_secs(3600),
            max_bytes: 2 << 30,
        }
    }
}
impl Options {
    pub fn validate(&self, path: &Path) -> Result<()> {
        if self.audio {
            return Err(Error::AudioUnsupported);
        }
        if !path.is_absolute()
            || !path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("webm"))
            || self.parent_window.len() > 1024
            || self.parent_window.chars().any(char::is_control)
            || !(self.parent_window.is_empty()
                || self.parent_window.starts_with("x11:")
                || self.parent_window.starts_with("wayland:"))
            || !(1..=60).contains(&self.fps)
            || self.portal_timeout.is_zero()
            || self.portal_timeout > Duration::from_secs(120)
            || self.startup_timeout.is_zero()
            || self.startup_timeout > Duration::from_secs(15)
            || self.stop_timeout.is_zero()
            || self.stop_timeout > Duration::from_secs(10)
            || self.max_duration.is_zero()
            || self.max_duration > Duration::from_secs(4 * 3600)
            || !(1 << 20..=16 << 30).contains(&self.max_bytes)
        {
            return Err(Error::Options);
        }
        Ok(())
    }
}
/// Only typed numeric properties enter the pipeline. Output paths never do.
pub fn pipeline(fd: i32, node: u32, serial: Option<u64>, fps: u32) -> Result<Vec<String>> {
    if fd < 3 || node == 0 || node == u32::MAX || serial == Some(0) || !(1..=60).contains(&fps) {
        return Err(Error::Options);
    }
    let target = serial
        .map(|id| format!("target-object={id}"))
        .unwrap_or_else(|| format!("path={node}"));
    let mut args = vec![
        "-q".into(),
        "-e".into(),
        "-f".into(),
        "--no-position".into(),
        "pipewiresrc".into(),
        format!("fd={fd}"),
        target,
    ];
    args.extend(
        [
            "do-timestamp=true",
            "!",
            "queue",
            "max-size-buffers=3",
            "max-size-bytes=0",
            "max-size-time=0",
            "leaky=downstream",
            "!",
            "videoconvert",
            "!",
            "videorate",
            "drop-only=true",
            "!",
        ]
        .into_iter()
        .map(str::to_string),
    );
    args.push(format!(
        "video/x-raw,format=I420,framerate={fps}/1,width=[1,8192],height=[1,8192]"
    ));
    args.extend(
        [
            "!",
            "vp8enc",
            "deadline=1",
            "cpu-used=4",
            "threads=2",
            "!",
            "webmmux",
            "!",
            "fdsink",
            "fd=1",
            "sync=false",
        ]
        .into_iter()
        .map(str::to_string),
    );
    Ok(args)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Starting,
    Recording,
    Stopping,
    Finished,
    Failed(Error),
}
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod process;
#[cfg(target_os = "linux")]
mod recorder;
#[cfg(target_os = "linux")]
pub use recorder::Recorder;
#[cfg(test)]
mod tests {
    use super::*;
    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join("synthetic recording ! filesink.webm")
    }
    #[test]
    fn options_are_video_only_and_bounded() {
        let mut o = Options::default();
        o.validate(&path()).unwrap();
        o.audio = true;
        assert_eq!(o.validate(&path()), Err(Error::AudioUnsupported));
        o.audio = false;
        o.fps = 61;
        assert_eq!(o.validate(&path()), Err(Error::Options));
        assert!(Options::default()
            .validate(Path::new("relative.webm"))
            .is_err());
    }
    #[test]
    fn pipeline_uses_only_numeric_ids_and_fixed_sink() {
        let args = pipeline(64, 42, None, 30).unwrap();
        assert!(args.contains(&"fd=64".into()));
        assert!(args.contains(&"path=42".into()));
        assert!(args.contains(&"fdsink".into()));
        assert!(args.contains(&"fd=1".into()));
        assert!(!args
            .iter()
            .any(|a| a.starts_with("location=") || a == "filesink"));
        let serial = pipeline(64, 42, Some(999), 30).unwrap();
        assert!(serial.contains(&"target-object=999".into()));
        assert!(!serial.contains(&"path=42".into()));
    }
    #[test]
    fn pipeline_rejects_invalid_descriptors_nodes_and_rates() {
        for (fd, node, serial, fps) in [
            (1, 42, None, 30),
            (64, 0, None, 30),
            (64, u32::MAX, None, 30),
            (64, 42, Some(0), 30),
            (64, 42, None, 0),
            (64, 42, None, 61),
        ] {
            assert!(pipeline(fd, node, serial, fps).is_err());
        }
    }
}
