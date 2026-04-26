use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct WeatherState {
    pub location: String,
    /// Air temperature in °C. `None` until the first probe lands.
    pub temp_c: Option<f32>,
    /// One-word condition derived from WMO code (Sunny, Cloudy, Rainy, …).
    pub condition: Option<String>,
    /// WMO weather interpretation code — useful for the frontend to pick
    /// an icon. See https://open-meteo.com/en/docs (WMO Weather codes).
    pub code: Option<u8>,
}

const MARIBOR_LAT: f64 = 46.5547;
const MARIBOR_LON: f64 = 15.6459;
const REFRESH: Duration = Duration::from_secs(900); // 15 min — Open-Meteo says don't pound it
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
    pub fn spawn() -> Self {
        let state = Arc::new(Mutex::new(WeatherState {
            location: "Maribor".into(),
            ..Default::default()
        }));
        let s = state.clone();
        std::thread::spawn(move || loop {
            match fetch() {
                Ok((temp, code)) => {
                    let mut g = s.lock().unwrap();
                    g.temp_c = Some(temp);
                    g.condition = Some(condition_for(code).into());
                    g.code = Some(code);
                }
                Err(e) => tracing::debug!("weather fetch failed: {e}"),
            }
            std::thread::sleep(REFRESH);
        });
        Self { state }
    }

    pub fn current(&self) -> WeatherState {
        self.state.lock().unwrap().clone()
    }
}

fn fetch() -> anyhow::Result<(f32, u8)> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,weather_code",
        MARIBOR_LAT, MARIBOR_LON
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .build();
    let body: Resp = agent.get(&url).call()?.into_json()?;
    Ok((body.current.temperature_2m, body.current.weather_code))
}

/// Map WMO weather codes to a one-word human label. We only collapse to the
/// broad category — the icon picker on the frontend can read the raw code
/// for finer distinctions (e.g. drizzle vs rain).
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
