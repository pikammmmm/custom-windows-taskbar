use std::path::PathBuf;
use anyhow::{Result, anyhow};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub fn data_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("com", "glassbar", "glassbar")
        .ok_or_else(|| anyhow!("could not resolve AppData directory"))?;
    let dir = dirs.data_dir().to_path_buf();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn pinned_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("pinned.json"))
}

pub fn icon_cache_dir() -> Result<PathBuf> {
    let dir = data_dir()?.join("icon-cache");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn settings_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("settings.json"))
}

/// Set of taskbar-pin paths we've already imported into the dock — used so
/// the live sync only adds *newly* pinned items. Without this, unpinning
/// from the dock gets reverted on the next sync because the entry still
/// exists in the Windows taskbar pin folder.
pub fn imported_taskbar_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("imported_taskbar.json"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub hud_position: Option<(f64, f64)>,
    #[serde(default)]
    pub auto_start: bool,
    /// Display name for the configured weather location. Shown next to the
    /// temperature in the HUD. None = first run, treat as "not set yet".
    #[serde(default)]
    pub weather_city: Option<String>,
    #[serde(default)]
    pub weather_lat: Option<f64>,
    #[serde(default)]
    pub weather_lon: Option<f64>,
    /// Most recent volume the user explicitly set via the HUD slider —
    /// persisted so that reopening the HUD seeds the slider with the
    /// last-known value instead of flashing 50% (the HTML default) until
    /// the next snapshot tick lands. Distinct from the OS endpoint volume
    /// only briefly during the user-intent window.
    #[serde(default)]
    pub volume_percent: Option<u8>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hud_position: None,
            auto_start: false,
            weather_city: None,
            weather_lat: None,
            weather_lon: None,
            volume_percent: None,
        }
    }
}

pub fn load_settings() -> Result<Settings> {
    let path = settings_path()?;
    if !path.exists() { return Ok(Settings::default()); }
    let s = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&s).unwrap_or_default())
}

pub fn save_settings(s: &Settings) -> Result<()> {
    let path = settings_path()?;
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    std::fs::write(&path, serde_json::to_string_pretty(s)?)?;
    Ok(())
}
