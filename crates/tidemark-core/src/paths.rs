//! Where Tidemark keeps its files.
//!
//! - Windows: `%APPDATA%\win-island` for both config and data (unchanged since
//!   Win Island, so upgrades keep settings, history and notes).
//! - Linux and other Unix: XDG Base Directory spec —
//!   `$XDG_CONFIG_HOME/tidemark` (default `~/.config/tidemark`) for
//!   `config.toml`, `$XDG_DATA_HOME/tidemark` (default
//!   `~/.local/share/tidemark`) for notes, clipboard history, lyrics, shelf.
//!
//! `resolve` takes the environment as a function, so tests never look at (or
//! write to) the real user directories.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// Holds `config.toml`.
    pub config: PathBuf,
    /// Holds notes, clipboard history, caches and the shelf.
    pub data: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Unix,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Unix
        }
    }
}

/// Windows folder name, kept from the Win Island days for compatibility.
pub const WINDOWS_DIR: &str = "win-island";
/// Folder name under the XDG base directories.
pub const XDG_DIR: &str = "tidemark";

pub fn resolve(platform: Platform, env: impl Fn(&str) -> Option<OsString>) -> AppDirs {
    match platform {
        Platform::Windows => {
            let base = env("APPDATA").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| ".".into());
            let dir = base.join(WINDOWS_DIR);
            AppDirs { config: dir.clone(), data: dir }
        }
        Platform::Unix => {
            // XDG: a relative value is invalid and must be ignored. `has_root`
            // (not `is_absolute`) so "/home/u" is judged the Unix way even
            // when the resolver is exercised on a Windows host.
            let xdg = |name: &str| env(name).map(PathBuf::from).filter(|p| p.has_root());
            let home = env("HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| ".".into());
            let config = xdg("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
            let data = xdg("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"));
            AppDirs { config: config.join(XDG_DIR), data: data.join(XDG_DIR) }
        }
    }
}

/// The directories for this user, from the real environment.
pub fn current() -> AppDirs {
    resolve(Platform::current(), |name| std::env::var_os(name))
}

/// Writes `bytes` to `path` via a uniquely named sibling temp file and a
/// rename, so readers see either the old or the new content, never a partial
/// file, and concurrent writers never share a temp file. Missing parent
/// folders are created (0700 on Unix; existing folders are not touched); the
/// file is private (0600) on Unix. On Windows the rename replaces an existing
/// file, as `std::fs::rename` always has. The temp file is removed on error.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    create_private_dirs(dir)?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "write_atomic: path has no file name"))?;
    let (tmp, mut file) = create_temp(dir, name)?;
    let written = file.write_all(bytes);
    drop(file); // closed before the rename (required on Windows)
    let result = written.and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Like `create_dir_all`, but folders it creates are 0700 on Unix.
fn create_private_dirs(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// `.<name>.<pid>.<n>.tmp` in `dir`, opened with `create_new` (0600 on Unix).
fn create_temp(dir: &Path, name: &std::ffi::OsStr) -> std::io::Result<(PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut tmp_name = OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(format!(".{}.{n}.tmp", std::process::id()));
        let tmp = dir.join(tmp_name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            // A stale file from an earlier process with the same pid: next n.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_dir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh folder under the system temp dir, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("tidemark-core-test-{}-{tag}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn windows_uses_appdata_for_both() {
        let dirs = resolve(Platform::Windows, env(&[("APPDATA", r"C:\Users\u\AppData\Roaming")]));
        let expected = PathBuf::from(r"C:\Users\u\AppData\Roaming").join("win-island");
        assert_eq!(dirs, AppDirs { config: expected.clone(), data: expected });
    }

    #[test]
    fn windows_without_appdata_falls_back_to_cwd() {
        assert_eq!(resolve(Platform::Windows, env(&[])).config, PathBuf::from(".").join("win-island"));
    }

    #[test]
    fn linux_uses_xdg_dirs() {
        let dirs = resolve(
            Platform::Unix,
            env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/cfg"), ("XDG_DATA_HOME", "/data")]),
        );
        assert_eq!(dirs.config, PathBuf::from("/cfg/tidemark"));
        assert_eq!(dirs.data, PathBuf::from("/data/tidemark"));
    }

    #[test]
    fn linux_defaults_under_home() {
        let dirs = resolve(Platform::Unix, env(&[("HOME", "/home/u")]));
        assert_eq!(dirs.config, PathBuf::from("/home/u/.config/tidemark"));
        assert_eq!(dirs.data, PathBuf::from("/home/u/.local/share/tidemark"));
    }

    #[test]
    fn relative_xdg_is_ignored() {
        let dirs = resolve(
            Platform::Unix,
            env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "rel/cfg"), ("XDG_DATA_HOME", "")]),
        );
        assert_eq!(dirs.config, PathBuf::from("/home/u/.config/tidemark"));
        assert_eq!(dirs.data, PathBuf::from("/home/u/.local/share/tidemark"));
    }

    #[test]
    fn no_home_falls_back_to_cwd() {
        let dirs = resolve(Platform::Unix, env(&[]));
        assert_eq!(dirs.data, PathBuf::from(".").join(".local").join("share").join("tidemark"));
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn atomic_write_creates_parents_and_replaces_existing_file() {
        let tmp = test_dir::TempDir::new("atomic");
        let dir = tmp.path().join("a");
        let file = dir.join("b.txt");
        write_atomic(&file, b"one").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"one");
        write_atomic(&file, b"two, longer").unwrap();
        write_atomic(&file, b"3").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"3");
        assert_eq!(entries(&dir), ["b.txt"]);
    }

    #[test]
    fn concurrent_writes_are_never_partial_and_leave_no_temp_files() {
        const WRITERS: u8 = 8;
        const ROUNDS: usize = 25;
        const LEN: usize = 64 * 1024;
        let tmp = test_dir::TempDir::new("concurrent");
        let file = tmp.path().join("history.json");
        write_atomic(&file, &vec![b'0'; LEN]).unwrap();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (file, done) = (file.clone(), done.clone());
            std::thread::spawn(move || {
                let mut reads = 0;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    // Windows may refuse a read during a replace; that is fine,
                    // a short or mixed read is not.
                    if let Ok(data) = std::fs::read(&file) {
                        assert_eq!(data.len(), LEN, "partial file seen");
                        assert!(data.iter().all(|&b| b == data[0]), "mixed contents seen");
                        reads += 1;
                    }
                }
                reads
            })
        };
        let writers: Vec<_> = (1..=WRITERS)
            .map(|w| {
                let file = file.clone();
                std::thread::spawn(move || {
                    let payload = vec![b'0' + w; LEN];
                    // Windows can report a transient sharing error when two
                    // renames race onto the same target; count successes.
                    (0..ROUNDS).filter(|_| write_atomic(&file, &payload).is_ok()).count()
                })
            })
            .collect();
        let ok: usize = writers.into_iter().map(|t| t.join().unwrap()).sum();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        reader.join().unwrap();
        assert!(ok > 0);
        let data = std::fs::read(&file).unwrap();
        assert_eq!(data.len(), LEN);
        assert!((b'1'..=b'0' + WRITERS).contains(&data[0]) && data.iter().all(|&b| b == data[0]));
        assert_eq!(entries(tmp.path()), ["history.json"]);
    }

    #[test]
    fn failed_rename_removes_temp_file() {
        let tmp = test_dir::TempDir::new("rename-fail");
        // The target is a non-empty directory, so the rename must fail.
        let target = tmp.path().join("notes.json");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), b"x").unwrap();
        assert!(write_atomic(&target, b"data").is_err());
        assert_eq!(entries(tmp.path()), ["notes.json"]);
        assert_eq!(entries(&target), ["keep"]);
    }

    #[cfg(unix)]
    #[test]
    fn unix_files_are_private_and_new_dirs_are_0700() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let tmp = test_dir::TempDir::new("perms");
        let existing = tmp.path().join("existing");
        std::fs::DirBuilder::new().mode(0o755).create(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = existing.join("new").join("deeper").join("history.json");
        write_atomic(&file, b"{}").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&existing.join("new")), 0o700);
        assert_eq!(mode(&existing.join("new").join("deeper")), 0o700);
        assert_eq!(mode(&existing), 0o755, "existing parent must not be chmod-ed");
        // Replacing a world-readable file leaves a private one.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&file, b"[]").unwrap();
        assert_eq!(mode(&file), 0o600);
    }
}
