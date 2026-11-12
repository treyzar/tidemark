use crate::{transport::encode, Client, Error, Result};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetLanguage {
    English,
    Russian,
}
impl TargetLanguage {
    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Russian => "ru",
        }
    }
}
pub fn target_for(text: &str) -> TargetLanguage {
    let cyr = text
        .chars()
        .filter(|c| ('\u{0400}'..='\u{04ff}').contains(c))
        .count();
    let lat = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if cyr > lat {
        TargetLanguage::English
    } else {
        TargetLanguage::Russian
    }
}
pub fn parse_google(json: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| Error::InvalidResponse)?;
    let parts = value
        .get(0)
        .and_then(|v| v.as_array())
        .ok_or(Error::InvalidResponse)?;
    let mut out = String::new();
    for part in parts {
        out.push_str(
            part.get(0)
                .and_then(|v| v.as_str())
                .ok_or(Error::InvalidResponse)?,
        );
    }
    if out.trim().is_empty() {
        Err(Error::InvalidResponse)
    } else {
        Ok(out)
    }
}
pub fn parse_mymemory(json: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| Error::InvalidResponse)?;
    if value.get("responseStatus").and_then(|v| v.as_u64()) != Some(200) {
        return Err(Error::InvalidResponse);
    }
    let text = value
        .get("responseData")
        .and_then(|v| v.get("translatedText"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or(Error::InvalidResponse)?;
    Ok(text.to_owned())
}
impl Client {
    /// Explicit user action only. Google gtx is an unofficial compatibility API;
    /// on failure, MyMemory receives the same text if it fits its 500-byte limit.
    /// Text is never truncated. Whitespace input performs no network request.
    pub fn translate(&self, text: &str, target: TargetLanguage) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }
        if text.len() > 5000 {
            return Err(Error::TooLarge);
        }
        let to = target.code();
        let url = format!("https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={to}&dt=t&q={}",encode(text));
        match self.get(&url).and_then(|s| parse_google(&s)) {
            Ok(text) => Ok(text),
            Err(primary) => {
                if text.len() > 500 {
                    return Err(primary);
                }
                let from = if target == TargetLanguage::English {
                    "ru"
                } else {
                    "en"
                };
                let url = format!(
                    "https://api.mymemory.translated.net/get?q={}&langpair={from}%7C{to}",
                    encode(text)
                );
                self.get(&url).and_then(|s| parse_mymemory(&s))
            }
        }
    }
}
