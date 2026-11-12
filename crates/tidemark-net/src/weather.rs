use crate::{transport::encode, Client, Error, Result};
use serde::Deserialize;
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
        let t = self.temperature.round();
        if t < 0.0 {
            format!("\u{2212}{:.0}°", t.abs())
        } else {
            format!("{:.0}°", t.abs())
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

fn geocoding(json: &str) -> Option<(f64, f64)> {
    let r: GeoResponse = serde_json::from_str(json).ok()?;
    let first = r.results?.into_iter().next()?;
    Some((first.latitude, first.longitude))
}

fn forecast(json: &str) -> Option<Weather> {
    let r: ForecastResponse = serde_json::from_str(json).ok()?;
    if r.current.is_day > 1 {
        return None;
    }
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
    if !(1..=12).contains(&h) || m >= 60 {
        return None;
    }
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
fn wttr(json: &str, now_minutes: u32) -> Option<Weather> {
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

pub fn parse_geocoding(json: &str) -> Result<(f64, f64)> {
    let (lat, lon) = geocoding(json).ok_or(Error::InvalidResponse)?;
    if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 || lon.abs() > 180.0 {
        return Err(Error::InvalidResponse);
    }
    Ok((lat, lon))
}
pub fn parse_forecast(json: &str) -> Result<Weather> {
    forecast(json)
        .filter(|w| w.temperature.is_finite())
        .ok_or(Error::InvalidResponse)
}
pub fn parse_wttr(json: &str, now_minutes: u32) -> Result<Weather> {
    if now_minutes >= 1440 {
        return Err(Error::InvalidInput);
    }
    wttr(json, now_minutes)
        .filter(|w| w.temperature.is_finite())
        .ok_or(Error::InvalidResponse)
}
impl Client {
    /// Empty city is an opt-out and performs no request. Fallback time is
    /// caller's local clock (same as Windows); use weather_at for deterministic time.
    pub fn weather(&self, city: &str) -> Result<Option<Weather>> {
        use chrono::Timelike;
        let now = chrono::Local::now();
        let mins = now.hour() * 60 + now.minute();
        self.weather_at(city, mins)
    }
    pub fn weather_at(&self, city: &str, now_minutes: u32) -> Result<Option<Weather>> {
        let city = city.trim();
        if city.is_empty() {
            return Ok(None);
        }
        if city.len() > 512 || now_minutes >= 1440 {
            return Err(Error::InvalidInput);
        }
        let primary = (|| {
            let geo = self.get(&format!("https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=ru&format=json", encode(city)))?;
            let (lat, lon) = parse_geocoding(&geo)?;
            let raw = self.get(&format!("https://api.open-meteo.com/v1/forecast?latitude={lat:.4}&longitude={lon:.4}&current=temperature_2m,weather_code,is_day&timezone=auto"))?;
            parse_forecast(&raw)
        })();
        primary
            .or_else(|_| {
                let raw = self.get(&format!(
                    "https://wttr.in/{}?format=j1&lang=ru",
                    encode(city)
                ))?;
                parse_wttr(&raw, now_minutes)
            })
            .map(Some)
    }
}
