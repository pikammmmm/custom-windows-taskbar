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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub hud_position: Option<(f64, f64)>,
    #[serde(default)]
    pub auto_start: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { hud_position: None, auto_start: false }
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
