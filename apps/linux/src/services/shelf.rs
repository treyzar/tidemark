//! A shelf stores references only. Removing an entry never removes its file.
use std::{
    io::{self, Read},
    path::Path,
};
const MAX_ITEMS: usize = 20;
const MAX_BYTES: u64 = 1 << 20;
#[derive(Clone, Default)]
pub struct Shelf {
    pub files: Vec<String>,
}
impl Shelf {
    pub fn add(&mut self, uris: &[String]) {
        for uri in uris.iter().rev() {
            if !uri.starts_with("file:///")
                || uri.len() > 32768
                || uri.chars().any(char::is_control)
            {
                continue;
            }
            self.files.retain(|old| old != uri);
            self.files.insert(0, uri.clone());
        }
        self.files.truncate(MAX_ITEMS);
    }
    pub fn load(path: &Path) -> io::Result<Self> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shelf is too large",
            ));
        }
        let uris: Vec<String> = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut shelf = Self::default();
        shelf.add(&uris);
        Ok(shelf)
    }
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(&self.files).map_err(io::Error::other)?;
        tidemark_core::paths::write_atomic(path, &bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_references_preserve_case_order_and_survive_duplicates() {
        let mut shelf = Shelf::default();
        shelf.add(&[
            "file:///tmp/A".into(),
            "file:///tmp/a".into(),
            "https://example.invalid/file".into(),
        ]);
        assert_eq!(shelf.files, ["file:///tmp/A", "file:///tmp/a"]);
        shelf.add(&["file:///tmp/a".into()]);
        assert_eq!(shelf.files, ["file:///tmp/a", "file:///tmp/A"]);
        shelf.add(
            &(0..40)
                .map(|n| format!("file:///tmp/{n}"))
                .collect::<Vec<_>>(),
        );
        assert_eq!(shelf.files.len(), 20);
        assert_eq!(shelf.files[0], "file:///tmp/0");
    }
    #[test]
    fn persistence_keeps_references_and_rejects_corrupt_data_without_overwriting() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tidemark-shelf-test-{}-{stamp}.json",
            std::process::id()
        ));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let mut shelf = Shelf::default();
        shelf.add(&["file:///tmp/unchanged".into()]);
        shelf.save(&path).unwrap();
        assert_eq!(Shelf::load(&path).unwrap().files, shelf.files);
        std::fs::write(&path, b"broken shelf").unwrap();
        assert!(Shelf::load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken shelf");
        std::fs::remove_file(path).unwrap();
    }
}
