//! User configuration stored in %APPDATA%\win-island\config.toml.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// Programs and shortcuts in the Home dock (%VARS% allowed). Absent on
    /// first run, when a few Windows apps are filled in.
    pub dock_apps: Option<Vec<String>>,
    /// Use the hardware GPU instead of the WARP software renderer.
    /// Costs ~50 MB more memory; only worth it for heavy effects.
    pub gpu: bool,
}

const DEFAULT_TILES: [&str; 6] = ["media", "clipboard", "window", "weather", "tools", "apps"];
const OLD_DEFAULT_TILES: [&str; 5] = ["media", "clipboard", "tools", "weather", "apps"];

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
            accent: "#7C6CF2".into(),
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
            dock_apps: None,
            gpu: false,
        }
    }
}

impl Config {
    pub fn dir() -> PathBuf {
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into());
        base.join("win-island")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    /// Brings configs written by older versions up to date.
    fn upgrade(mut self) -> Self {
        if self.home_tiles == OLD_DEFAULT_TILES.map(String::from) {
            self.home_tiles = DEFAULT_TILES.map(String::from).to_vec();
        }
        self
    }

    /// Loads the config, writing the defaults on first run. Never fails:
    /// a broken file falls back to defaults so the island always starts.
    pub fn load() -> Self {
        match std::fs::read_to_string(Self::path()) {
            Ok(text) => toml::from_str::<Config>(&text).unwrap_or_default().upgrade(),
            Err(_) => {
                let cfg = Self::default();
                cfg.save();
                cfg
            }
        }
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(Self::dir());
        if let Ok(text) = toml::to_string(self) {
            let _ = std::fs::write(Self::path(), text);
        }
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
        assert_eq!(cfg.home_tiles.len(), 6);
        // A config saved with the old default tile list gains the new tile.
        let old: Config = toml::from_str("home_tiles = [\"media\", \"clipboard\", \"tools\", \"weather\", \"apps\"]").unwrap();
        assert!(old.upgrade().home_tiles.contains(&"window".to_string()));
    }
}
