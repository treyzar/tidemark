//! Platform-neutral UI decisions for the Linux island. Everything here is
//! pure so it is covered by `cargo test` on Windows and Linux; GTK widgets
//! and D-Bus workers live in `ui` and `services`.

use std::time::Duration;

use tidemark_core::config::{parse_hex, Config};
use tidemark_core::shell::Capability;

/// Tab id, Russian label and icon name. The icon names come from the
/// standard Adwaita icon theme shipped with Ubuntu 24.04.
pub const TABS: [(&str, &str, &str); 5] = [
    ("home", "Главная", "go-home-symbolic"),
    ("clipboard", "Буфер", "edit-paste-symbolic"),
    ("notes", "Заметки", "appointment-new-symbolic"),
    ("text", "Текст", "text-x-generic-symbolic"),
    ("control", "Управление", "preferences-system-symbolic"),
];

/// Tiles that can be toggled on the Home page, matching the shared config.
pub const HOME_TILES: [(&str, &str, &str); 6] = [
    (
        "media",
        "Музыка",
        "Трек, обложка и управление воспроизведением",
    ),
    ("clipboard", "Буфер обмена", "Недавно скопированные тексты"),
    ("window", "Окно", "Быстрые действия с активным окном"),
    ("tools", "Инструменты", "Захват экрана, OCR, QR и запись"),
    ("weather", "Погода", "Погода для выбранного города"),
    ("apps", "Приложения", "Закреплённые приложения в доке"),
];

pub const THEME_LABELS: [&str; 3] = ["Как в системе", "Тёмная", "Светлая"];
pub const THEME_VALUES: [&str; 3] = ["system", "dark", "light"];

pub const REMINDER_HINT: &str =
    "Например: через 15 минут позвонить маме, завтра в 9:00 встреча, в 18:30 выключить плиту";
pub const REMINDER_EXAMPLES: &str =
    "Не понял время. Примеры: «через 15 минут позвонить», «в 18:30 выключить плиту», «завтра в 9:00 встреча».";

/// Hover delay, clamped to the range the Windows settings window accepts.
pub fn hover_delay(text: &str, fallback: u32) -> u32 {
    text.trim().parse::<u32>().unwrap_or(fallback).min(5000)
}

/// `#RRGGBB` normalized the same way the Windows settings write it.
pub fn normalize_hex(text: &str) -> Option<String> {
    parse_hex(text)?;
    Some(format!(
        "#{}",
        text.trim().trim_start_matches('#').to_uppercase()
    ))
}

/// Fraction of the media progress bar under a click, clamped to 0..=1.
pub fn fraction_for_x(x: f64, width: f64) -> f64 {
    if width <= 0.0 {
        0.0
    } else {
        (x / width).clamp(0.0, 1.0)
    }
}

/// "1:23 / 3:45", with the MPRIS duration when the player publishes it.
pub fn progress_label(position: Duration, duration: Option<Duration>) -> String {
    let left = tidemark_core::timer::clock(position);
    match duration {
        Some(d) if d > Duration::ZERO => format!("{left} / {}", tidemark_core::timer::clock(d)),
        _ => left,
    }
}

/// Home tile order: configured ids that still exist, then the remaining
/// known tiles only if the config is empty/corrupt.
pub fn visible_home_tiles(cfg: &Config) -> Vec<String> {
    let known: Vec<&str> = HOME_TILES.iter().map(|(id, _, _)| *id).collect();
    let mut out: Vec<String> = cfg
        .home_tiles
        .iter()
        .filter(|id| known.contains(&id.as_str()))
        .cloned()
        .collect();
    if out.is_empty() {
        out.push("media".into());
    }
    out
}

pub fn tile_enabled(cfg: &Config, id: &str) -> bool {
    visible_home_tiles(cfg).iter().any(|tile| tile == id)
}

/// Russian name and explanation for every backend capability. The System
/// page lists all of them; features whose capability is missing are disabled
/// with the same explanation instead of pretending to work.
pub fn capability_text(capability: Capability) -> (&'static str, &'static str) {
    match capability {
        Capability::PlaceWindows => (
            "Размещение окна",
            "Остров поверх окон, без рамки и вне списка задач",
        ),
        Capability::InputRegion => ("Клики вне формы", "Прозрачные для мыши поля вокруг острова"),
        Capability::FocusTracking => (
            "Активное окно",
            "Плитка окна и уведомления о переключении приложений",
        ),
        Capability::WindowActions => (
            "Действия с окном",
            "Поверх всех, прозрачность, свернуть, половина экрана",
        ),
        Capability::FullscreenTracking => (
            "Полноэкранный режим",
            "Прятать остров в играх и при просмотре видео",
        ),
        Capability::MaximizedTracking => {
            ("Развёрнутые окна", "Сжимать остров над развёрнутым окном")
        }
        Capability::ClipboardRead => ("Чтение буфера обмена", "История копирования в фоне"),
        Capability::ClipboardWrite => ("Запись буфера обмена", "Возврат записи из истории в буфер"),
        Capability::Screenshot => ("Снимок экрана", "Тихий снимок области без острова"),
        Capability::CaptureHiding => ("Скрытие из записи", "Не показывать остров на записи экрана"),
        Capability::GlobalShortcuts => (
            "Горячие клавиши",
            "Поиск, тема и лайк на глобальных сочетаниях",
        ),
        Capability::KeyboardLayout => ("Раскладка клавиатуры", "Индикатор RU/EN"),
        Capability::CapsLock => ("Caps Lock", "Индикатор Caps Lock"),
        Capability::Brightness => ("Яркость", "Индикатор яркости экрана"),
    }
}

pub fn missing_capabilities(caps: &[Capability]) -> Vec<Capability> {
    Capability::ALL
        .iter()
        .copied()
        .filter(|capability| !caps.contains(capability))
        .collect()
}

/// The honest status line used by Home and Settings while the GNOME Shell
/// adapter is not running.
pub fn shell_status_text(caps: &[Capability]) -> String {
    if caps.is_empty() {
        "Расширение GNOME Shell не подключено. Остров работает как обычное окно; \
         активное окно, фоновая история буфера, горячие клавиши и захват экрана недоступны."
            .into()
    } else {
        format!("Доступно возможностей рабочего стола: {}.", caps.len())
    }
}

/// Percent-encode a local path as a `file://` URI for FileManager1 and
/// `GtkFileLauncher`.
pub fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for byte in path.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Decode a local `file://` artwork URI from MPRIS. Returns None for remote
/// URLs and malformed escapes.
pub fn art_file_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = rest.get(i + 1..i + 3)?;
            let value = u8::from_str_radix(hex, 16).ok()?;
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn timer_presets() -> [(u32, &'static str); 3] {
    [(5, "5 мин"), (15, "15 мин"), (25, "25 мин")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_delay_is_bounded_and_keeps_fallback() {
        assert_eq!(hover_delay("300", 100), 300);
        assert_eq!(hover_delay("99999", 100), 5000);
        assert_eq!(hover_delay("abc", 100), 100);
        assert_eq!(hover_delay("  ", 100), 100);
    }

    #[test]
    fn hex_is_normalized_only_when_valid() {
        assert_eq!(normalize_hex("#a54831"), Some("#A54831".into()));
        assert_eq!(normalize_hex("00FF00"), Some("#00FF00".into()));
        assert_eq!(normalize_hex("#12"), None);
        assert_eq!(normalize_hex(""), None);
    }

    #[test]
    fn progress_fraction_and_label() {
        assert_eq!(fraction_for_x(50.0, 200.0), 0.25);
        assert_eq!(fraction_for_x(-10.0, 200.0), 0.0);
        assert_eq!(fraction_for_x(300.0, 200.0), 1.0);
        assert_eq!(fraction_for_x(20.0, 0.0), 0.0);
        assert_eq!(
            progress_label(Duration::from_secs(83), Some(Duration::from_secs(225))),
            "1:23 / 3:45"
        );
        assert_eq!(progress_label(Duration::from_secs(8), None), "0:08");
    }

    #[test]
    fn home_tiles_follow_config_and_have_a_fallback() {
        let mut cfg = Config::default();
        cfg.home_tiles = vec!["notes".into(), "clipboard".into(), "media".into()];
        assert_eq!(visible_home_tiles(&cfg), ["clipboard", "media"]);
        cfg.home_tiles.clear();
        assert_eq!(visible_home_tiles(&cfg), ["media"]);
        cfg.home_tiles = vec!["window".into(), "media".into()];
        assert!(tile_enabled(&cfg, "window"));
        assert!(!tile_enabled(&cfg, "tools"));
    }

    #[test]
    fn file_uris_round_trip_local_paths() {
        let uri = file_uri("/home/Иван/Музыка/a b.mp3");
        assert!(uri.starts_with("file:///home/"));
        assert!(uri.contains("%20"));
        let path = art_file_path(&uri).unwrap();
        assert_eq!(path, "/home/Иван/Музыка/a b.mp3");
        assert_eq!(art_file_path("https://example.test/cover.png"), None);
        assert_eq!(art_file_path("file:///bad%2"), None);
    }

    #[test]
    fn capability_report_lists_every_missing_feature() {
        let missing = missing_capabilities(&[]);
        assert_eq!(missing.len(), Capability::ALL.len());
        assert!(shell_status_text(&[]).contains("GNOME Shell"));
        assert_eq!(
            capability_text(Capability::FocusTracking).0,
            "Активное окно"
        );
    }
}
