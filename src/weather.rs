//! Current weather from Open-Meteo, falling back to wttr.in where
//! api.open-meteo.com is unreachable (both free, no API key). A background thread
//! fetches every 30 minutes, sleeping in between; the UI thread is told
//! about new data with a posted message.

use crate::http;
use serde::Deserialize;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_WEATHER: u32 = WM_APP + 2;
const REFRESH: Duration = Duration::from_secs(30 * 60);
const RETRY: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    Snow,
    Thunder,
}

impl Icon {
    /// Maps a WMO weather interpretation code.
    pub fn from_wmo(code: u32) -> Self {
        match code {
            0 => Self::Clear,
            1 | 2 => Self::PartlyCloudy,
            3 => Self::Cloudy,
            45 | 48 => Self::Fog,
            51..=57 => Self::Drizzle,
            61..=67 | 80..=82 => Self::Rain,
            71..=77 | 85 | 86 => Self::Snow,
            95..=99 => Self::Thunder,
            _ => Self::Cloudy,
        }
    }

    /// Maps a WorldWeatherOnline condition code (used by wttr.in).
    pub fn from_wwo(code: u32) -> Self {
        match code {
            113 => Self::Clear,
            116 => Self::PartlyCloudy,
            119 | 122 => Self::Cloudy,
            143 | 248 | 260 => Self::Fog,
            176 | 263 | 266 | 281 | 284 | 293 | 296 | 353 => Self::Drizzle,
            299 | 302 | 305 | 308 | 311 | 314 | 356 | 359 => Self::Rain,
            179 | 182 | 185 | 227 | 230 | 317..=338 | 350 | 362..=377 => Self::Snow,
            200 | 386..=395 => Self::Thunder,
            _ => Self::Cloudy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weather {
    pub temperature: f32,
    pub icon: Icon,
    pub is_day: bool,
}

impl Weather {
    /// "18°", "−5°" (with a real minus sign), "0°".
    pub fn temperature_label(&self) -> String {
        let t = self.temperature.round() as i32;
        if t < 0 {
            format!("\u{2212}{}°", -t)
        } else {
            format!("{t}°")
        }
    }
}

#[derive(Deserialize)]
struct GeoResponse {
    results: Option<Vec<GeoResult>>,
}

#[derive(Deserialize)]
struct GeoResult {
    latitude: f64,
    longitude: f64,
}

#[derive(Deserialize)]
struct ForecastResponse {
    current: Current,
}

#[derive(Deserialize)]
struct Current {
    temperature_2m: f32,
    weather_code: u32,
    is_day: u8,
}

pub fn parse_geocoding(json: &str) -> Option<(f64, f64)> {
    let r: GeoResponse = serde_json::from_str(json).ok()?;
    let first = r.results?.into_iter().next()?;
    Some((first.latitude, first.longitude))
}

pub fn parse_forecast(json: &str) -> Option<Weather> {
    let r: ForecastResponse = serde_json::from_str(json).ok()?;
    Some(Weather {
        temperature: r.current.temperature_2m,
        icon: Icon::from_wmo(r.current.weather_code),
        is_day: r.current.is_day != 0,
    })
}

#[derive(Deserialize)]
struct WttrResponse {
    current_condition: Vec<WttrCurrent>,
    weather: Vec<WttrDay>,
}

#[derive(Deserialize)]
struct WttrCurrent {
    #[serde(rename = "temp_C")]
    temp_c: String,
    #[serde(rename = "weatherCode")]
    weather_code: String,
}

#[derive(Deserialize)]
struct WttrDay {
    astronomy: Vec<WttrAstronomy>,
}

#[derive(Deserialize)]
struct WttrAstronomy {
    sunrise: String,
    sunset: String,
}

/// "07:56 AM" -> minutes since midnight.
fn parse_12h(s: &str) -> Option<u32> {
    let (time, ampm) = s.trim().split_once(' ')?;
    let (h, m) = time.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    let h = match (ampm, h) {
        ("AM", 12) => 0,
        ("AM", h) => h,
        ("PM", 12) => 12,
        ("PM", h) => h + 12,
        _ => return None,
    };
    Some(h * 60 + m)
}

/// Parses wttr.in `format=j1`. wttr.in has no day/night flag, so it is
/// derived from today's sunrise/sunset and `now_minutes` (local time).
pub fn parse_wttr(json: &str, now_minutes: u32) -> Option<Weather> {
    let r: WttrResponse = serde_json::from_str(json).ok()?;
    let cur = r.current_condition.first()?;
    let is_day = r
        .weather
        .first()
        .and_then(|d| d.astronomy.first())
        .and_then(|a| Some((parse_12h(&a.sunrise)?, parse_12h(&a.sunset)?)))
        .is_none_or(|(rise, set)| now_minutes >= rise && now_minutes < set);
    Some(Weather {
        temperature: cur.temp_c.trim().parse().ok()?,
        icon: Icon::from_wwo(cur.weather_code.trim().parse().ok()?),
        is_day,
    })
}

fn local_minutes() -> u32 {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    t.wHour as u32 * 60 + t.wMinute as u32
}

fn fetch_wttr(city: &str) -> Option<Weather> {
    let path = format!("/{}?format=j1&lang=ru", http::encode(city));
    parse_wttr(&http::get("wttr.in", &path)?, local_minutes())
}

#[derive(Clone, Copy, PartialEq)]
enum Source {
    OpenMeteo,
    Wttr,
}

fn geocode(city: &str) -> Option<(f64, f64)> {
    let path = format!("/v1/search?name={}&count=1&language=ru&format=json", http::encode(city));
    parse_geocoding(&http::get("geocoding-api.open-meteo.com", &path)?)
}

fn fetch(lat: f64, lon: f64) -> Option<Weather> {
    let path = format!(
        "/v1/forecast?latitude={lat:.4}&longitude={lon:.4}&current=temperature_2m,weather_code,is_day&timezone=auto"
    );
    parse_forecast(&http::get("api.open-meteo.com", &path)?)
}

/// Handle to the weather thread. Dropping it stops the thread.
pub struct Service {
    pub latest: Arc<Mutex<Option<Weather>>>,
    wake: Sender<()>,
}

impl Service {
    /// Starts the thread; returns None when no city is configured.
    pub fn start(hwnd: HWND, city: &str) -> Option<Self> {
        let city = city.trim().to_string();
        if city.is_empty() {
            return None;
        }
        let latest = Arc::new(Mutex::new(None));
        let (wake, rx) = mpsc::channel::<()>();
        let shared = latest.clone();
        let hwnd_raw = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("weather".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let hwnd = HWND(hwnd_raw as *mut _);
                let mut coords = None;
                let mut source = Source::OpenMeteo;
                loop {
                    let mut got = None;
                    if source == Source::OpenMeteo {
                        if coords.is_none() {
                            coords = geocode(&city);
                        }
                        got = coords.and_then(|(lat, lon)| fetch(lat, lon));
                    }
                    if got.is_none() {
                        // Open-Meteo blocked or down: stay on wttr.in for
                        // this session instead of waiting on timeouts.
                        got = fetch_wttr(&city);
                        if got.is_some() {
                            source = Source::Wttr;
                        }
                    }
                    let wait = if got.is_some() { REFRESH } else { RETRY };
                    if let Some(w) = got {
                        *shared.lock().unwrap() = Some(w);
                        unsafe {
                            let _ = PostMessageW(Some(hwnd), WM_WEATHER, WPARAM(0), LPARAM(0));
                        }
                    }
                    match rx.recv_timeout(wait) {
                        Ok(()) | Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .ok()?;
        Some(Self { latest, wake })
    }

    /// Fetch now (e.g. after the PC wakes from sleep).
    pub fn refresh(&self) {
        let _ = self.wake.send(());
    }

    pub fn get(&self) -> Option<Weather> {
        *self.latest.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmo_codes() {
        assert_eq!(Icon::from_wmo(0), Icon::Clear);
        assert_eq!(Icon::from_wmo(2), Icon::PartlyCloudy);
        assert_eq!(Icon::from_wmo(48), Icon::Fog);
        assert_eq!(Icon::from_wmo(55), Icon::Drizzle);
        assert_eq!(Icon::from_wmo(81), Icon::Rain);
        assert_eq!(Icon::from_wmo(86), Icon::Snow);
        assert_eq!(Icon::from_wmo(96), Icon::Thunder);
    }

    #[test]
    fn parses_open_meteo_responses() {
        let geo = r#"{"results":[{"id":1,"name":"Москва","latitude":55.75222,"longitude":37.61556}],"generationtime_ms":0.5}"#;
        assert_eq!(parse_geocoding(geo), Some((55.75222, 37.61556)));
        assert_eq!(parse_geocoding(r#"{"generationtime_ms":0.5}"#), None);

        let fc = r#"{"latitude":55.75,"current_units":{"temperature_2m":"°C"},"current":{"time":"2026-11-11T14:45","interval":900,"temperature_2m":-4.6,"weather_code":71,"is_day":1}}"#;
        let w = parse_forecast(fc).unwrap();
        assert_eq!(w.icon, Icon::Snow);
        assert!(w.is_day);
        assert_eq!(w.temperature_label(), "\u{2212}5°");
    }

    #[test]
    fn wwo_codes() {
        assert_eq!(Icon::from_wwo(113), Icon::Clear);
        assert_eq!(Icon::from_wwo(116), Icon::PartlyCloudy);
        assert_eq!(Icon::from_wwo(248), Icon::Fog);
        assert_eq!(Icon::from_wwo(308), Icon::Rain);
        assert_eq!(Icon::from_wwo(338), Icon::Snow);
        assert_eq!(Icon::from_wwo(389), Icon::Thunder);
    }

    #[test]
    fn parses_wttr_and_derives_day_night() {
        let json = r#"{"current_condition":[{"temp_C":"-3","weatherCode":"116","humidity":"80"}],
            "weather":[{"date":"2026-11-11","astronomy":[{"sunrise":"07:56 AM","sunset":"04:30 PM"}]}]}"#;
        let noon = parse_wttr(json, 12 * 60).unwrap();
        assert_eq!(noon.icon, Icon::PartlyCloudy);
        assert_eq!(noon.temperature_label(), "\u{2212}3°");
        assert!(noon.is_day);
        assert!(!parse_wttr(json, 2 * 60).unwrap().is_day);
        assert!(!parse_wttr(json, 17 * 60).unwrap().is_day);
    }

    #[test]
    fn twelve_hour_times() {
        assert_eq!(parse_12h("12:05 AM"), Some(5));
        assert_eq!(parse_12h("07:56 AM"), Some(7 * 60 + 56));
        assert_eq!(parse_12h("12:00 PM"), Some(12 * 60));
        assert_eq!(parse_12h("04:30 PM"), Some(16 * 60 + 30));
        assert_eq!(parse_12h("nonsense"), None);
    }

    #[test]
    fn temperature_labels() {
        let w = |t| Weather { temperature: t, icon: Icon::Clear, is_day: true };
        assert_eq!(w(18.4).temperature_label(), "18°");
        assert_eq!(w(-0.3).temperature_label(), "0°");
        assert_eq!(w(-12.6).temperature_label(), "\u{2212}13°");
    }
}

/// Live network checks: `cargo test -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore]
    fn providers_respond() {
        let t = std::time::Instant::now();
        let raw = http::get("geocoding-api.open-meteo.com", "/v1/search?name=%D0%9C%D0%BE%D1%81%D0%BA%D0%B2%D0%B0&count=1&language=ru&format=json");
        println!("geocode raw: {:?} ({:?})", raw.as_ref().map(|s| &s[..s.len().min(80)]), t.elapsed());
        let t = std::time::Instant::now();
        println!("open-meteo: {:?} ({:?})", fetch(55.75, 37.62), t.elapsed());
        let t = std::time::Instant::now();
        let raw = http::get("wttr.in", "/%D0%9C%D0%BE%D1%81%D0%BA%D0%B2%D0%B0?format=j1&lang=ru");
        println!("wttr raw: {:?} ({:?})", raw.as_ref().map(|s| &s[..s.len().min(80)]), t.elapsed());
        println!("wttr: {:?}", fetch_wttr("Москва"));
    }
}
