use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct WeatherState {
    pub location: String,
    /// Air temperature in °C. `None` until the first probe lands.
    pub temp_c: Option<f32>,
    /// One-word condition derived from the weather icon (Clear, Cloudy, Rainy, …).
    pub condition: Option<String>,
    /// WMO-style weather code, mapped from ARSO's icon string. Kept on the
    /// WMO scale because the HUD's emoji picker is keyed off WMO codes.
    pub code: Option<u8>,
}

const ARSO_URL: &str = "https://meteo.arso.gov.si/uploads/probase/www/observ/surface/text/sl/observation_MARIBOR_TABOR_latest.xml";
// ARSO publishes new observations ~25 min after each hour, hourly. 15 min
// keeps us close to fresh without pounding their server.
const REFRESH: Duration = Duration::from_secs(900);
const TIMEOUT: Duration = Duration::from_secs(8);

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
                Err(e) => tracing::debug!("ARSO weather fetch failed: {e}"),
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
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let xml = agent
        .get(ARSO_URL)
        .set("User-Agent", "glassbar/0.1")
        .call()?
        .into_string()?;
    let temp_str = extract_tag(&xml, "t")
        .ok_or_else(|| anyhow::anyhow!("no <t> in ARSO response"))?;
    let temp: f32 = temp_str.parse()?;
    let icon = extract_tag(&xml, "nn_icon-wwsyn_icon").unwrap_or_default();
    Ok((temp, arso_icon_to_wmo(&icon)))
}

/// Tiny depth-1 tag extractor — ARSO's surface observation XML is flat enough
/// that a real parser is overkill. Returns the inner text of the first
/// occurrence of `<tag>...</tag>`, trimmed.
fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end_rel = xml[start..].find(&close)?;
    let inner = xml[start..start + end_rel].trim();
    if inner.is_empty() { None } else { Some(inner.to_string()) }
}

/// Map ARSO's `nn_icon-wwsyn_icon` string back to WMO codes so the HUD's
/// existing icon mapping keeps working. ARSO encodes weather as
/// `<cloud-cover>[_<intensity><weather>][_<dayNight>]`, e.g. `clear_day`,
/// `overcast_lightRA_day`, `prevCloudy_modTS_night`. Precipitation /
/// thunderstorm / fog dominates the icon when present, otherwise we fall
/// back to cloud-cover bucketing.
fn arso_icon_to_wmo(icon: &str) -> u8 {
    let i = icon.to_ascii_lowercase();

    // Precipitation & special phenomena (precedence matters: TS over RA, SH over plain).
    if i.contains("ts") { return 95; }                      // thunderstorm
    if i.contains("shsn") { return 86; }                    // snow shower
    if i.contains("sn") { return 71; }                      // snow
    if i.contains("shra") { return 80; }                    // rain shower
    if i.contains("ra") { return 61; }                      // rain
    if i.contains("dz") { return 51; }                      // drizzle
    if i.contains("fg") { return 45; }                      // fog

    // No precipitation — bucket by cloud cover. Match the prefix so the
    // optional `_day` / `_night` suffix doesn't trip us.
    if i.starts_with("clear") { return 0; }
    if i.starts_with("mostclear") { return 1; }
    if i.starts_with("partcloudy") { return 2; }
    if i.starts_with("modcloudy") { return 2; }
    if i.starts_with("prevcloudy") { return 3; }
    if i.starts_with("overcast") { return 3; }

    255 // unknown → frontend renders the neutral fallback glyph
}

/// Map WMO codes to a one-word human label. Kept here (not in the frontend)
/// so the HUD just shows whatever string we hand it.
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
