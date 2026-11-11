//! Colors for the island, resolved from the config and the system theme.

use crate::config::{parse_hex, Config};
use windows::core::w;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

pub type Rgba = [f32; 4];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub light: bool,
    pub background: Rgba,
    pub tile: Rgba,
    pub tile_hover: Rgba,
    pub text: Rgba,
    pub subtext: Rgba,
    pub accent: Rgba,
}

impl Theme {
    pub fn resolve(cfg: &Config) -> Self {
        let light = match cfg.theme.as_str() {
            "light" => true,
            "dark" => false,
            _ => system_prefers_light(),
        };
        let mut t = if light { Self::light() } else { Self::dark() };
        if let Some(bg) = parse_hex(&cfg.background) {
            t.background = bg;
        }
        if let Some(acc) = parse_hex(&cfg.accent) {
            t.accent = acc;
        }
        t.background[3] = cfg.opacity.clamp(0.0, 1.0);
        t
    }

    fn dark() -> Self {
        Self {
            light: false,
            background: [0.0, 0.0, 0.0, 1.0],
            tile: [1.0, 1.0, 1.0, 0.07],
            tile_hover: [1.0, 1.0, 1.0, 0.13],
            text: [1.0, 1.0, 1.0, 0.95],
            subtext: [1.0, 1.0, 1.0, 0.55],
            accent: [0.486, 0.424, 0.949, 1.0],
        }
    }

    fn light() -> Self {
        Self {
            light: true,
            background: [0.961, 0.961, 0.973, 1.0],
            tile: [0.0, 0.0, 0.0, 0.05],
            tile_hover: [0.0, 0.0, 0.0, 0.10],
            text: [0.0, 0.0, 0.0, 0.9],
            subtext: [0.0, 0.0, 0.0, 0.5],
            accent: [0.416, 0.357, 0.886, 1.0],
        }
    }
}

/// Reads "Choose your app mode" from the Personalize key.
pub fn system_prefers_light() -> bool {
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let res = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    res.is_ok() && value != 0
}
