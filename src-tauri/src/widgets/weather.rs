//! Weather probe — Open-Meteo current-conditions endpoint, location driven
//! by the user's settings. Open-Meteo's geocoding API resolves any city
//! name into lat/lon (separate command in commands.rs); the user picks one
//! in the HUD's setup / Settings flow and we read the saved coords here.
//! No API key required.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Flipped to true by `request_refresh` to wake the probe out of its sleep.
/// Used so a city change in the HUD updates the weather card right away
/// instead of after the next 15-minute tick.
static REFRESH_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH_REQUESTED.store(true, Ordering::Release);
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct WeatherState {
    pub location: String,
    /// Air temperature in °C. `None` until the first probe lands.
    pub temp_c: Option<f32>,
    /// One-word condition derived from the WMO code.
    pub condition: Option<String>,
    /// Raw WMO weather interpretation code — used by the HUD to pick a glyph.
    pub code: Option<u8>,
}

// 15 min — Open-Meteo's free tier asks for moderate polling and the model
// data updates hourly anyway, so 15 minutes is comfortable headroom.
const REFRESH: Duration = Duration::from_secs(900);
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Deserialize)]
struct Resp {
    current: Current,
}
#[derive(Deserialize)]
struct Current {
    temperature_2m: f32,
    weather_code: u8,
}

pub struct Probe {
    state: Arc<Mutex<WeatherState>>,
}

impl Probe {
    /// Spawn the polling thread. The probe re-reads the user's settings
    /// every refresh so changing the city in the HUD flips weather without
    /// a glassbar restart.
    pub fn spawn() -> Self {
        let state = Arc::new(Mutex::new(WeatherState::default()));
        let s = state.clone();
        std::thread::spawn(move || loop {
            // Reload settings every tick so a city change picks up promptly.
            let settings = crate::config::load_settings().unwrap_or_default();
            match (settings.weather_lat, settings.weather_lon) {
                (Some(lat), Some(lon)) => {
                    let location = settings.weather_city.unwrap_or_default();
                    match fetch(lat, lon) {
                        Ok((temp, code)) => {
                            let mut g = s.lock().unwrap();
                            g.location = location;
                            g.temp_c = Some(temp);
                            g.condition = Some(condition_for(code).into());
                            g.code = Some(code);
                        }
                        Err(e) => tracing::debug!("weather fetch failed: {e}"),
                    }
                }
                _ => {
                    // No city configured yet — clear so the HUD shows the
                    // "set a city" prompt instead of stale data.
                    *s.lock().unwrap() = WeatherState::default();
                }
            }
            // Sleep in 1-second slices so a request_refresh() call breaks
            // out of the wait promptly. Once the flag clears we either
            // refetch (because the user changed a city) or fall through
            // to the next scheduled tick.
            let started = std::time::Instant::now();
            while started.elapsed() < REFRESH {
                std::thread::sleep(Duration::from_secs(1));
                if REFRESH_REQUESTED.swap(false, Ordering::AcqRel) {
                    break;
                }
            }
        });
        Self { state }
    }

    pub fn current(&self) -> WeatherState {
        self.state.lock().unwrap().clone()
    }
}

fn fetch(lat: f64, lon: f64) -> anyhow::Result<(f32, u8)> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
         &current=temperature_2m,weather_code&timezone=auto"
    );
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let body: Resp = agent
        .get(&url)
        .set("User-Agent", "glassbar/0.1")
        .call()?
        .into_json()?;
    Ok((body.current.temperature_2m, body.current.weather_code))
}

/// Map WMO weather codes to a one-word human label.
fn condition_for(code: u8) -> &'static str {
    match code {
        0 => "Clear",
        1 | 2 => "Partly cloudy",
        3 => "Cloudy",
        45 | 48 => "Foggy",
        51..=57 => "Drizzle",
        61..=67 | 80..=82 => "Rainy",
        71..=77 | 85..=86 => "Snowy",
        95..=99 => "Stormy",
        _ => "Unknown",
    }
}
