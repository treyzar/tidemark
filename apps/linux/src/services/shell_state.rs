//! Pure owner/generation and clipboard-offer guards, no desktop access.
use tidemark_core::{clip_history::MAX_TEXT_CHARS, shell::ClipboardOffer};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub owner: String,
    pub generation: u64,
}
impl Peer {
    pub fn matches(&self, owner: &str, generation: u64) -> bool {
        self.owner == owner && self.generation == generation
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadKey {
    pub peer: Peer,
    pub offer: u64,
    pub mime: String,
}
#[derive(Default)]
pub struct ClipboardGuard {
    enabled: bool,
    serial: u64,
    current: Option<ReadKey>,
}
impl ClipboardGuard {
    pub fn enable(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.invalidate();
            self.enabled = enabled;
        }
    }
    pub fn invalidate(&mut self) {
        self.serial = self.serial.wrapping_add(1);
        self.current = None;
    }
    pub fn offer(&mut self, peer: &Peer, offer: &ClipboardOffer) -> Option<ReadKey> {
        self.invalidate();
        if self.enabled
            && !offer.sensitive
            && !offer.mimes.iter().any(|m| m == "x-kde-passwordManagerHint")
        {
            let mime = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"]
                .into_iter()
                .find(|m| offer.mimes.iter().any(|offered| offered == m));
            self.current = mime.map(|mime| ReadKey {
                peer: peer.clone(),
                offer: self.serial,
                mime: mime.into(),
            });
        }
        self.current.clone()
    }
    pub fn current(&self) -> Option<ReadKey> {
        self.current.clone()
    }
    pub fn accepts(&self, key: &ReadKey) -> bool {
        self.enabled && self.current.as_ref() == Some(key)
    }
}
pub const MAX_TEXT_BYTES: usize = MAX_TEXT_CHARS * 4;
pub fn text(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_TEXT_BYTES {
        return Err("Текст в буфере превышает допустимый размер".into());
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| "Буфер содержит текст не в UTF-8".to_owned())?;
    if text.trim().is_empty() || text.chars().count() > MAX_TEXT_CHARS {
        return Err("Текст в буфере пуст или слишком длинный".into());
    }
    Ok(text.to_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn peer(n: u64) -> Peer {
        Peer {
            owner: ":1.2".into(),
            generation: n,
        }
    }
    fn offer() -> ClipboardOffer {
        ClipboardOffer {
            mimes: vec!["text/plain".into()],
            sensitive: false,
        }
    }
    #[test]
    fn same_unique_owner_new_generation_never_accepts_old_mutation() {
        assert!(peer(1).matches(":1.2", 1));
        assert!(!peer(1).matches(":1.2", 2));
        assert!(!peer(1).matches(":1.3", 1));
    }
    #[test]
    fn newer_offer_secret_and_setting_disable_invalidate_reads() {
        let mut guard = ClipboardGuard::default();
        guard.enable(true);
        let first = guard.offer(&peer(1), &offer()).unwrap();
        assert!(guard.accepts(&first));
        let next = guard.offer(&peer(1), &offer()).unwrap();
        assert!(!guard.accepts(&first));
        assert!(guard.accepts(&next));
        let mut secret = offer();
        secret.sensitive = true;
        assert!(guard.offer(&peer(1), &secret).is_none());
        assert!(!guard.accepts(&next));
        secret.sensitive = false;
        secret.mimes.push("x-kde-passwordManagerHint".into());
        assert!(guard.offer(&peer(1), &secret).is_none());
        let current = guard.offer(&peer(1), &offer()).unwrap();
        guard.enable(false);
        assert!(!guard.accepts(&current));
    }
    #[test]
    fn text_bounds_count_unicode_characters_and_reject_invalid_utf8() {
        assert_eq!(text("Пример".as_bytes()).unwrap(), "Пример");
        assert!(text(&[255]).is_err());
        assert!(text("я".repeat(MAX_TEXT_CHARS).as_bytes()).is_ok());
        assert!(text("я".repeat(MAX_TEXT_CHARS + 1).as_bytes()).is_err());
    }
}
