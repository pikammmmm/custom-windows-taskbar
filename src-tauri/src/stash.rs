//! User-curated file stash. Files dropped onto the HUD's Files panel land
//! here and persist in stash.json so they survive restarts. Each entry
//! stores just the absolute path; the display name is derived from the
//! filename.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StashEntry {
    pub path: String,
    /// Filename without directory; precomputed so the UI doesn't need to
    /// duplicate the parsing logic.
    pub name: String,
}

pub type StashHandle = Arc<Mutex<Vec<StashEntry>>>;

pub fn load() -> Result<Vec<StashEntry>> {
    let path = config::data_dir()?.join("stash.json");
    if !path.exists() { return Ok(vec![]); }
    let s = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&s).unwrap_or_default())
}

pub fn save(entries: &[StashEntry]) -> Result<()> {
    let path = config::data_dir()?.join("stash.json");
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    std::fs::write(&path, serde_json::to_string_pretty(entries)?)?;
    Ok(())
}

/// Build a StashEntry from a raw filesystem path. Returns None if the
/// path doesn't exist or has no filename component.
pub fn entry_for(p: &str) -> Option<StashEntry> {
    let path = Path::new(p);
    if !path.exists() { return None; }
    let name = path.file_name().and_then(|s| s.to_str())?.to_string();
    Some(StashEntry { path: p.to_string(), name })
}
