//! Events that background services and the shell backend deliver to the UI
//! thread on Linux (on Windows the same role is played by posted
//! `WM_APP + n` messages). Only platform-neutral, serializable payloads live
//! here; feature crates (e.g. MPRIS) define their own snapshot types and an
//! adapter converts them into UI state.

use crate::shell::ShellSignal;
use serde::{Deserialize, Serialize};

/// On-screen notices in the collapsed island.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "osd", rename_all = "snake_case")]
pub enum Osd {
    /// `level` 0.0..=1.0.
    Volume { level: f32, muted: bool },
    Brightness { level: f32 },
    Layout { code: String },
    CapsLock { on: bool },
}

/// Which service an error or status belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceId {
    Shell,
    Media,
    Lyrics,
    Weather,
    Audio,
    Privacy,
    Network,
    Bluetooth,
    Power,
    Proxy,
    Clipboard,
    Launcher,
    Capture,
    Ocr,
    Recording,
    Agent,
    Translate,
    YandexMusic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Shell { signal: ShellSignal },
    Osd { osd: Osd },
    /// A service could not do its job; `message` is user-facing Russian text.
    ServiceError { service: ServiceId, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::ShortcutId;
    use serde_json::{json, to_value};

    #[test]
    fn event_json_is_stable() {
        assert_eq!(
            to_value(Event::Osd { osd: Osd::Volume { level: 0.25, muted: false } }).unwrap(),
            json!({"type": "osd", "osd": {"osd": "volume", "level": 0.25, "muted": false}})
        );
        assert_eq!(
            to_value(Event::ServiceError { service: ServiceId::YandexMusic, message: "Вкладка не найдена".into() })
                .unwrap(),
            json!({"type": "service_error", "service": "yandex_music", "message": "Вкладка не найдена"})
        );
        assert_eq!(
            to_value(Event::Shell { signal: ShellSignal::ShortcutActivated { id: ShortcutId::Launcher } }).unwrap(),
            json!({"type": "shell", "signal": {"signal": "shortcut_activated", "id": "launcher"}})
        );
    }

    #[test]
    fn events_round_trip() {
        for e in [
            Event::Osd { osd: Osd::Brightness { level: 1.0 } },
            Event::Osd { osd: Osd::Layout { code: "EN".into() } },
            Event::Osd { osd: Osd::CapsLock { on: false } },
            Event::Shell { signal: ShellSignal::CapsLock { on: true } },
            Event::ServiceError { service: ServiceId::Network, message: "x".into() },
        ] {
            let text = serde_json::to_string(&e).unwrap();
            assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), e);
        }
    }
}
