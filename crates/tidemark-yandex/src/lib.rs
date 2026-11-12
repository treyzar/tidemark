//! Verified Yandex Music favorites through accessibility; no credentials or input simulation.
use thiserror::Error;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("accessibility transport unavailable")]
    Transport,
    #[error("accessibility owner changed or disappeared")]
    OwnerChanged,
    #[error("accessibility limits exceeded")]
    Limit,
    #[error("a unique trusted Yandex Music document was not verified")]
    Origin,
    #[error("the exact track was not verified in a unique player")]
    Track,
    #[error("a unique Like button with a definite state was not verified")]
    Button,
    #[error("the track, origin, button or favorite state changed")]
    Changed,
    #[error("the accessibility action failed")]
    Action,
    #[error("the requested favorite state was not confirmed")]
    NotConfirmed,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Object {
    pub owner: String,
    pub path: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub document: Object,
    pub player: Object,
    pub button: Object,
    pub url: String,
    pub liked: bool,
}
pub fn normalized(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
pub fn artist_name(s: &str) -> String {
    normalized(s.trim_end().trim_end_matches(['—', '–', '-']).trim_end())
}
pub fn metadata_matches(names: &[String], title: &str, artist: &str) -> bool {
    let (title, artist) = (normalized(title), artist_name(artist));
    !title.is_empty()
        && !artist.is_empty()
        && names.iter().any(|s| normalized(s) == title)
        && names.iter().any(|s| artist_name(s) == artist)
}
pub fn trusted_music_url(url: &str) -> bool {
    if url
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    [
        "music.yandex.ru",
        "music.yandex.com",
        "music.yandex.by",
        "music.yandex.kz",
    ]
    .iter()
    .any(|host| authority.eq_ignore_ascii_case(host))
}
/// GetState is two packed 32-bit words, not a list of state IDs.
pub fn has_state(words: &[u32], bit: usize) -> bool {
    words
        .get(bit / 32)
        .is_some_and(|w| w & (1 << (bit % 32)) != 0)
}
pub fn like_state(role: u32, words: &[u32]) -> Result<bool> {
    if words.len() != 2
        || has_state(words, 6)
        || has_state(words, 27)
        || has_state(words, 32)
        || !has_state(words, 8)
        || !has_state(words, 24)
        || !has_state(words, 25)
        || !has_state(words, 30)
    {
        return Err(Error::Button);
    }
    if role == 7 && has_state(words, 41) {
        // A checkbox can be pressed while still unchecked (pointer down).
        // Only CHECKED confirms its persistent boolean value.
        Ok(has_state(words, 4))
    } else if role == 62 || (role == 43 && has_state(words, 41)) {
        Ok(has_state(words, 20) || has_state(words, 4))
    } else {
        Err(Error::Button)
    }
}
#[cfg(target_os = "linux")]
pub mod client;
#[cfg(target_os = "linux")]
pub use client::Client;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_requires_exact_https_authority() {
        for u in [
            "https://music.yandex.ru",
            "HTTPS://MUSIC.YANDEX.COM/album/1",
            "https://music.yandex.by/",
            "https://music.yandex.kz/",
        ] {
            assert!(trusted_music_url(u), "{u}");
        }
        for u in [
            "http://music.yandex.ru",
            "https://music.yandex.ru.evil/",
            "https://music.yandex.ru@evil/",
            "https://user@music.yandex.ru/",
            "https://music.yandex.ru:443/",
            "https://music.yandex.ru./",
            "https://music.yandex.ru%2eevil/",
            "https://musіc.yandex.ru/",
            "https://music.yandex.ru\\@evil/",
            "https://music.yandex.ru\n",
            "music.yandex.ru",
        ] {
            assert!(!trusted_music_url(u), "{u}");
        }
    }
    #[test]
    fn exact_metadata_requires_both_fields() {
        let n = vec!["Song".into(), "ARTIST –".into()];
        assert!(metadata_matches(&n, " song ", "artist"));
        assert!(!metadata_matches(&n, "Song remix", "artist"));
        assert!(!metadata_matches(&n, "Song", "other"));
        assert!(!metadata_matches(&n, "Song", ""));
    }
    #[test]
    fn state_requires_toggle_semantics_and_valid_visible_enabled_state() {
        let mut w = vec![(1 << 8) | (1 << 24) | (1 << 25) | (1 << 30), 0];
        assert_eq!(like_state(62, &w), Ok(false));
        w[0] |= 1 << 20;
        assert_eq!(like_state(62, &w), Ok(true));
        assert_eq!(like_state(43, &w), Err(Error::Button));
        w[1] |= 1 << 9;
        w[0] &= !(1 << 20);
        assert_eq!(like_state(43, &w), Ok(false));
        w[1] |= 1;
        assert_eq!(like_state(43, &w), Err(Error::Button));
    }
}

#[cfg(test)]
mod checkbox_regression {
    use super::*;
    #[test]
    fn pressing_unchecked_checkbox_does_not_confirm_favorite() {
        let enabled_visible_pressed = (1 << 8) | (1 << 24) | (1 << 25) | (1 << 30) | (1 << 20);
        let mut words = [enabled_visible_pressed, 1 << (41 - 32)];
        assert_eq!(like_state(7, &words), Ok(false));
        words[0] |= 1 << 4;
        assert_eq!(like_state(7, &words), Ok(true));
        words[0] &= !(1 << 20);
        assert_eq!(like_state(7, &words), Ok(true));
        // Toggle and ARIA-pressed push buttons retain their pressed convention.
        words[0] &= !(1 << 4);
        words[0] |= 1 << 20;
        assert_eq!(like_state(62, &words), Ok(true));
        assert_eq!(like_state(43, &words), Ok(true));
    }
}
