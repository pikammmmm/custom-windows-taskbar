use serde::{Deserialize, Serialize};
use std::path::Path;
use anyhow::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PinnedApp {
    pub path: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_path: Option<String>,
}

pub fn load_from(path: &Path) -> Result<Vec<PinnedApp>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let contents = std::fs::read_to_string(path)?;
    let apps = serde_json::from_str(&contents)?;
    Ok(apps)
}

pub fn save_to(path: &Path, apps: &[PinnedApp]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(apps)?;
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tmp() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pinned.json");
        (dir, path)
    }

    #[test]
    fn load_returns_empty_when_file_missing() {
        let (_d, p) = tmp();
        let apps = load_from(&p).unwrap();
        assert_eq!(apps, vec![]);
    }

    #[test]
    fn save_then_load_round_trip() {
        let (_d, p) = tmp();
        let apps = vec![
            PinnedApp { path: "C:\\a.exe".into(), display_name: "A".into(), icon_path: None },
            PinnedApp { path: "C:\\b.exe".into(), display_name: "B".into(), icon_path: Some("C:\\b.ico".into()) },
        ];
        save_to(&p, &apps).unwrap();
        assert_eq!(load_from(&p).unwrap(), apps);
    }

    #[test]
    fn load_returns_empty_when_file_is_empty_array() {
        let (_d, p) = tmp();
        std::fs::write(&p, "[]").unwrap();
        assert_eq!(load_from(&p).unwrap(), vec![]);
    }

    #[test]
    fn load_errors_on_malformed_json() {
        let (_d, p) = tmp();
        std::fs::write(&p, "{not json").unwrap();
        assert!(load_from(&p).is_err());
    }

    #[test]
    fn save_creates_parent_directory_if_missing() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("nested").join("pinned.json");
        save_to(&p, &[]).unwrap();
        assert!(p.exists());
    }
}
