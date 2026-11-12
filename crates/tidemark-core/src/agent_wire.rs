//! Wire format between `island-hook` (Claude Code hooks, the opencode plugin)
//! and the island. One JSON line each way, the same on Windows (named pipe)
//! and Linux (Unix socket):
//!
//!   → {"kind":"notice"|"ask","source":"Claude","title":"…","text":"…","options":["…"]}
//!   ← {"choice":1} or {"choice":null}   (only for "ask")

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Message {
    pub kind: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct Reply {
    pub choice: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_line_parses() {
        let m: Message = serde_json::from_str(r#"{"kind":"notice"}"#).unwrap();
        assert_eq!(m, Message { kind: "notice".into(), source: "".into(), title: "".into(), text: "".into(), options: vec![] });
        assert!(serde_json::from_str::<Message>(r#"{"title":"x"}"#).is_err());
    }

    #[test]
    fn full_ask_round_trips() {
        let line = r#"{"kind":"ask","source":"Claude","title":"Bash","text":"ls","options":["Разрешить","Нет"]}"#;
        let m: Message = serde_json::from_str(line).unwrap();
        assert_eq!(m.options, ["Разрешить", "Нет"]);
        assert_eq!(serde_json::from_str::<Message>(&serde_json::to_string(&m).unwrap()).unwrap(), m);
    }

    #[test]
    fn reply_null_and_index() {
        assert_eq!(serde_json::to_string(&Reply { choice: Some(1) }).unwrap(), r#"{"choice":1}"#);
        assert_eq!(serde_json::to_string(&Reply { choice: None }).unwrap(), r#"{"choice":null}"#);
        assert_eq!(serde_json::from_str::<Reply>(r#"{"choice":null}"#).unwrap(), Reply { choice: None });
    }
}
