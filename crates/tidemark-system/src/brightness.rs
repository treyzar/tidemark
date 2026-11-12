//! Read-only backlight inventory; mutation is exclusively through logind.
use crate::{Error, Result};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brightness {
    pub name: String,
    pub current: u32,
    pub maximum: u32,
    class: PathBuf,
    root: PathBuf,
    canonical: PathBuf,
    device: u64,
    inode: u64,
}
impl Brightness {
    pub fn refresh(&self) -> Result<Self> {
        read_device(&self.class, &self.root, &self.name)
    }
    pub(crate) fn same_device(&self, other: &Self) -> bool {
        self.name == other.name
            && self.maximum == other.maximum
            && self.canonical == other.canonical
            && self.device == other.device
            && self.inode == other.inode
    }
}
pub fn inventory() -> Result<Vec<Brightness>> {
    inventory_at(Path::new("/sys/class/backlight"), Path::new("/sys/devices"))
}
fn inventory_at(class: &Path, root: &Path) -> Result<Vec<Brightness>> {
    let entries = std::fs::read_dir(class).map_err(io)?;
    let mut result = Vec::new();
    for entry in entries {
        if result.len() >= 32 {
            return Err(Error::Limit);
        }
        let entry = entry.map_err(io)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Error::InvalidResponse)?;
        result.push(read_device(class, root, &name)?);
    }
    if result.is_empty() {
        return Err(Error::Unsupported);
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}
fn read_device(class: &Path, root: &Path, name: &str) -> Result<Brightness> {
    if name.is_empty()
        || name.len() > 128
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Err(Error::InvalidInput);
    }
    let canonical = std::fs::canonicalize(class.join(name)).map_err(io)?;
    let root = std::fs::canonicalize(root).map_err(io)?;
    if !canonical.starts_with(&root) || canonical == root {
        return Err(Error::Denied);
    }
    let metadata = std::fs::metadata(&canonical).map_err(io)?;
    if !metadata.is_dir() {
        return Err(Error::InvalidResponse);
    }
    let maximum = read_number(&canonical.join("max_brightness"))?;
    if maximum == 0 {
        return Err(Error::Unsupported);
    }
    let current = match read_number(&canonical.join("actual_brightness")) {
        Ok(n) => n,
        Err(Error::Unsupported) => read_number(&canonical.join("brightness"))?,
        Err(e) => return Err(e),
    };
    if current > maximum {
        return Err(Error::InvalidResponse);
    }
    Ok(Brightness {
        name: name.into(),
        current,
        maximum,
        class: class.into(),
        root,
        canonical,
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}
fn read_number(path: &Path) -> Result<u32> {
    crate::process::read_text(path, 128)?
        .trim()
        .parse()
        .map_err(|_| Error::InvalidResponse)
}
fn io(e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::NotFound => Error::Unsupported,
        std::io::ErrorKind::PermissionDenied => Error::Denied,
        _ => Error::Transport,
    }
}
#[cfg(test)]
pub(crate) fn fixture() -> (tempfile::TempDir, Brightness) {
    let temp = tempfile::tempdir().unwrap();
    let class = temp.path().join("class");
    let root = temp.path().join("devices");
    let device = root.join("panel");
    std::fs::create_dir_all(&class).unwrap();
    std::fs::create_dir_all(&device).unwrap();
    std::os::unix::fs::symlink(&device, class.join("panel")).unwrap();
    std::fs::write(device.join("max_brightness"), "100\n").unwrap();
    std::fs::write(device.join("brightness"), "40\n").unwrap();
    std::fs::write(device.join("actual_brightness"), "40\n").unwrap();
    let target = inventory_at(&class, &root).unwrap().remove(0);
    (temp, target)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sysfs_fixture_is_bounded_and_identity_revalidation_detects_replacement() {
        let (_temp, target) = fixture();
        assert_eq!((target.current, target.maximum), (40, 100));
        std::fs::write(target.canonical.join("max_brightness"), "200\n").unwrap();
        assert!(!target.same_device(&target.refresh().unwrap()));
        std::fs::write(target.canonical.join("actual_brightness"), "999\n").unwrap();
        assert!(target.refresh().is_err());
        assert!(read_device(&target.class, &target.root, "../escape").is_err());
    }
    #[test]
    fn class_symlink_outside_devices_is_refused() {
        let (temp, target) = fixture();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(outside, target.class.join("bad")).unwrap();
        assert!(matches!(
            read_device(&target.class, &target.root, "bad"),
            Err(Error::Denied)
        ));
    }
}
