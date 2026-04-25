use std::path::PathBuf;
use anyhow::{Result, anyhow};
use directories::ProjectDirs;

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
