//! Free translation without a key: Google's public "gtx" endpoint, with
//! MyMemory as a fallback. Russian text goes to English, anything else to
//! Russian. Runs on a short-lived thread; the result comes back as a
//! posted message.

use crate::http;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_TRANSLATED: u32 = WM_APP + 14;

/// Target language: "en" for mostly-Cyrillic text, otherwise "ru".
pub fn target_for(text: &str) -> &'static str {
    let (mut cyr, mut lat) = (0usize, 0usize);
    for c in text.chars() {
        if ('\u{0400}'..='\u{04FF}').contains(&c) {
            cyr += 1;
        } else if c.is_ascii_alphabetic() {
            lat += 1;
        }
    }
    if cyr > lat { "en" } else { "ru" }
}

/// Google returns [[["перевод","source",...], ...], ...].
pub fn parse_google(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let parts = v.get(0)?.as_array()?;
    let out: String = parts.iter().filter_map(|p| p.get(0)?.as_str()).collect();
    (!out.is_empty()).then_some(out)
}

pub fn parse_mymemory(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let t = v.get("responseData")?.get("translatedText")?.as_str()?;
    (!t.is_empty()).then(|| t.to_string())
}

fn google(text: &str, to: &str) -> Option<String> {
    let path = format!("/translate_a/single?client=gtx&sl=auto&tl={to}&dt=t&q={}", http::encode(text));
    parse_google(&http::get("translate.googleapis.com", &path)?)
}

fn mymemory(text: &str, to: &str) -> Option<String> {
    let from = if to == "en" { "ru" } else { "en" };
    // MyMemory caps queries at 500 bytes.
    let short: String = text.chars().take(450).collect();
    let path = format!("/get?q={}&langpair={from}|{to}", http::encode(&short));
    parse_mymemory(&http::get("api.mymemory.translated.net", &path)?)
}

pub fn translate(text: &str) -> Option<String> {
    let to = target_for(text);
    google(text, to).or_else(|| mymemory(text, to))
}

/// Translates on a background thread; posts WM_TRANSLATED when done (the
/// result, or None on failure, lands in `slot`).
pub fn spawn(hwnd: HWND, text: String, slot: Arc<Mutex<Option<Option<String>>>>) {
    let hwnd_raw = hwnd.0 as isize;
    let _ = std::thread::Builder::new().name("translate".into()).spawn(move || {
        let result = translate(&text);
        *slot.lock().unwrap() = Some(result);
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd_raw as *mut _)), WM_TRANSLATED, WPARAM(0), LPARAM(0));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_direction() {
        assert_eq!(target_for("Встреча перенесена"), "en");
        assert_eq!(target_for("The meeting has moved"), "ru");
        assert_eq!(target_for("12345"), "ru");
    }

    #[test]
    fn parses_responses() {
        let g = r#"[[["Встреча перенесена ","The meeting has moved ",null],["на пятницу","to Friday",null]],null,"en"]"#;
        assert_eq!(parse_google(g).as_deref(), Some("Встреча перенесена на пятницу"));
        assert_eq!(parse_google("[]"), None);
        let m = r#"{"responseData":{"translatedText":"Привет","match":1}}"#;
        assert_eq!(parse_mymemory(m).as_deref(), Some("Привет"));
    }
}
