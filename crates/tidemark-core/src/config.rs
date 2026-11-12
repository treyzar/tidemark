//! User configuration: `config.toml` in the config directory
//! (`%APPDATA%\win-island` on Windows, `$XDG_CONFIG_HOME/tidemark` on Linux;
//! see `paths`).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Collapsed pill size, in DIPs.
    pub collapsed_width: f32,
    pub collapsed_height: f32,
    /// Collapsed width while music plays (cover + track + clock), in DIPs.
    pub collapsed_media_width: f32,
    /// Expanded panel width, in DIPs. The height follows from the tiles.
    pub expanded_width: f32,
    /// Corner radius of the expanded panel, in DIPs.
    pub expanded_radius: f32,
    /// Gap between the top of the screen and the island, in DIPs.
    pub top_offset: f32,
    /// "system", "dark" or "light". Ctrl+Alt+T switches between dark and light.
    pub theme: String,
    /// Accent color, "#RRGGBB".
    pub accent: String,
    /// Island background override, "#RRGGBB"; empty = the theme's own color.
    pub background: String,
    /// 0.0..=1.0
    pub opacity: f32,
    /// Tiles on the "Home" view, in order. Remove an id to hide that tile;
    /// the island re-packs the rest. Known ids: media, clipboard, tools,
    /// weather, apps.
    pub home_tiles: Vec<String>,
    /// City for the weather, e.g. "Москва". Empty = weather off (nothing is
    /// sent anywhere). Looked up via Open-Meteo geocoding.
    pub weather_city: String,
    /// The island's edge bounces to the music's bass while something plays.
    /// Listens to the PC's output (not the microphone) only during playback.
    pub beat_edge: bool,
    /// Keep a history of copied text, images and files (stored locally in
    /// %APPDATA%\win-island\clipboard). Items marked private by password
    /// managers are never recorded.
    pub clipboard_history: bool,
    /// Slide away while a full-screen app (game, video, slideshow) covers
    /// the screen; the cursor at the top edge brings the island back.
    pub hide_in_fullscreen: bool,
    /// Over a maximized (or top-snapped) window, shrink the collapsed
    /// island to a thin handle so the window's tabs stay clickable.
    pub shrink_under_windows: bool,
    /// How long the cursor must rest on the island before it expands, in
    /// ms (0 = immediately). A click always opens it at once.
    pub hover_delay_ms: u32,
    /// Briefly show the app you switch to (icon and name).
    pub show_active_app: bool,
    /// Notices for volume, brightness, keyboard layout and Caps Lock.
    pub osd: bool,
    /// Dots and a notice while an app uses the camera or microphone.
    pub privacy_indicator: bool,
    /// Alt+Space opens the island's search (apps, files, calculator).
    pub launcher: bool,
    /// Web search for the last search row; {q} is the query.
    pub search_url: String,
    /// Programs and shortcuts in the Home dock (%VARS% allowed). Absent on
    /// first run, when a few Windows apps are filled in.
    pub dock_apps: Option<Vec<String>>,
    /// Use the hardware GPU instead of the WARP software renderer.
    /// Costs ~50 MB more memory; only worth it for heavy effects.
    pub gpu: bool,
}

const DEFAULT_TILES: [&str; 5] = ["media", "clipboard", "window", "tools", "apps"];
/// Defaults of earlier versions, upgraded on load.
const OLD_DEFAULT_TILES: [[&str; 6]; 1] = [["media", "clipboard", "window", "weather", "tools", "apps"]];
const OLDEST_DEFAULT_TILES: [&str; 5] = ["media", "clipboard", "tools", "weather", "apps"];

impl Default for Config {
    fn default() -> Self {
        Self {
            collapsed_width: 180.0,
            collapsed_height: 36.0,
            collapsed_media_width: 320.0,
            expanded_width: 560.0,
            expanded_radius: 28.0,
            top_offset: 8.0,
            theme: "system".into(),
            accent: "#A54831".into(),
            background: String::new(),
            opacity: 1.0,
            home_tiles: DEFAULT_TILES.map(String::from).to_vec(),
            weather_city: String::new(),
            beat_edge: true,
            clipboard_history: true,
            hide_in_fullscreen: true,
            shrink_under_windows: true,
            hover_delay_ms: 300,
            show_active_app: true,
            osd: true,
            privacy_indicator: true,
            launcher: true,
            search_url: "https://www.google.com/search?q={q}".into(),
            dock_apps: None,
            gpu: false,
        }
    }
}

impl Config {
    /// Folder holding `config.toml`. On Windows this is also the data folder.
    pub fn dir() -> PathBuf {
        crate::paths::current().config
    }

    /// Folder for notes, clipboard history, lyrics cache and the shelf.
    pub fn data_dir() -> PathBuf {
        crate::paths::current().data
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    /// Brings configs written by older versions up to date.
    fn upgrade(mut self) -> Self {
        if self.search_url=="https://yandex.ru/search/?text={q}" {
            self.search_url=Self::default().search_url;
        }
        if self.home_tiles == OLDEST_DEFAULT_TILES.map(String::from) || OLD_DEFAULT_TILES.iter().any(|d| self.home_tiles == d.map(String::from)) {
            self.home_tiles = DEFAULT_TILES.map(String::from).to_vec();
        }
        self
    }

    /// Loads the config, writing the defaults on first run. Never fails:
    /// a broken file falls back to defaults so the island always starts.
    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    /// `load` for an explicit file. A missing file is created with the
    /// defaults; an unreadable or broken one is left untouched.
    pub fn load_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Config>(&text).unwrap_or_default().upgrade(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Self::default();
                let _ = cfg.save_to(path);
                cfg
            }
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let _ = self.save_to(&Self::path());
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        crate::paths::write_atomic(path, text.as_bytes())
    }
}

/// Parses "#RRGGBB" into RGBA with alpha 1.0.
pub fn parse_hex(s: &str) -> Option<[f32; 4]> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    let c = |shift: u32| ((v >> shift) & 0xFF) as f32 / 255.0;
    Some([c(16), c(8), c(0), 1.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_is_used_for_new_and_legacy_default_search() {
        assert_eq!(Config::default().search_url, "https://www.google.com/search?q={q}");
        let legacy: Config=toml::from_str("search_url = 'https://yandex.ru/search/?text={q}'").unwrap();
        assert_eq!(legacy.upgrade().search_url,Config::default().search_url);
        let custom: Config=toml::from_str("search_url = 'https://example.com/find?q={q}'").unwrap();
        assert_eq!(custom.upgrade().search_url,"https://example.com/find?q={q}");
    }

    #[test]
    fn missing_file_is_created_with_defaults() {
        let tmp = crate::paths::test_dir::TempDir::new("cfg-missing");
        let file = tmp.path().join("sub").join("config.toml");
        let cfg = Config::load_from(&file);
        assert_eq!(cfg.search_url, Config::default().search_url);
        let written: Config = toml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written.home_tiles, Config::default().home_tiles);
    }

    #[test]
    fn save_and_load_round_trip() {
        let tmp = crate::paths::test_dir::TempDir::new("cfg-roundtrip");
        let file = tmp.path().join("config.toml");
        let cfg = Config {
            weather_city: "Москва".into(),
            opacity: 0.5,
            home_tiles: vec!["media".into(), "apps".into()],
            ..Config::default()
        };
        cfg.save_to(&file).unwrap();
        let back = Config::load_from(&file);
        assert_eq!(back.weather_city, "Москва");
        assert_eq!(back.opacity, 0.5);
        assert_eq!(back.home_tiles, ["media", "apps"]);
    }

    #[test]
    fn broken_file_loads_defaults_without_overwrite() {
        let tmp = crate::paths::test_dir::TempDir::new("cfg-broken");
        let file = tmp.path().join("config.toml");
        std::fs::write(&file, "theme = [broken").unwrap();
        assert_eq!(Config::load_from(&file).theme, Config::default().theme);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "theme = [broken");
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#FF0000"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_hex("00ff00"), Some([0.0, 1.0, 0.0, 1.0]));
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("#12"), None);
    }

    #[test]
    fn old_config_files_still_load() {
        // Fields from earlier versions (color, expanded_height) are ignored.
        let cfg: Config = toml::from_str("color = \"#000000\"\nexpanded_height = 220.0\ntheme = \"dark\"").unwrap();
        assert_eq!(cfg.theme, "dark");
        assert_eq!(cfg.home_tiles.len(), 5);
        let v2: Config = toml::from_str("home_tiles = [\"media\", \"clipboard\", \"window\", \"weather\", \"tools\", \"apps\"]").unwrap();
        assert_eq!(v2.upgrade().home_tiles, DEFAULT_TILES.map(String::from));
        // A config saved with the old default tile list gains the new tile.
        let old: Config = toml::from_str("home_tiles = [\"media\", \"clipboard\", \"tools\", \"weather\", \"apps\"]").unwrap();
        assert!(old.upgrade().home_tiles.contains(&"window".to_string()));
    }
}
