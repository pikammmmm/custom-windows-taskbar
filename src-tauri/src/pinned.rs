use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
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

/// Reorder `list` in place to match `new_order_paths` (compared
/// case-insensitively). Anything in `list` not mentioned in the new order
/// is preserved at the tail in original relative order — the user only ever
/// hands us a complete pin list from the DOM so this is just a safety net.
pub fn reorder(list: &mut Vec<PinnedApp>, new_order_paths: &[String]) {
    let mut remaining: Vec<PinnedApp> = std::mem::take(list);
    for path in new_order_paths {
        if let Some(pos) = remaining.iter()
            .position(|p| p.path.eq_ignore_ascii_case(path))
        {
            list.push(remaining.remove(pos));
        }
    }
    // Anything the caller forgot keeps its old place at the end.
    list.extend(remaining);
}

pub type PinnedHandle = Arc<Mutex<Vec<PinnedApp>>>;

pub fn watch(
    path: PathBuf,
    on_change: impl Fn(Vec<PinnedApp>) + Send + 'static,
) -> Result<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};

    let path_for_callback = path.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
            return;
        }
        // Debounce: editors often write in two passes (truncate + write).
        std::thread::sleep(Duration::from_millis(150));
        match load_from(&path_for_callback) {
            Ok(apps) => on_change(apps),
            Err(e) => tracing::warn!("pinned reload failed: {e}"),
        }
    })?;

    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("pinned path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    watcher.watch(parent, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
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
