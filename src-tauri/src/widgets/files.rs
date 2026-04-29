//! Filesystem index for the spotlight launcher. Walks the user's common
//! folders (Desktop, Documents, Downloads, Pictures, Videos, Music) at
//! startup and on a slow refresh; the spotlight searches names against
//! this in-memory list and launches matches via the OS file association.

use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

const MAX_DEPTH: usize = 4;
const MAX_ENTRIES: usize = 50_000;
// 10 minutes — files churn more than installed apps but indexing isn't free,
// and the user can always type the exact name to launch via the OS even if
// the index hasn't picked it up yet.
const REFRESH_SECS: u64 = 600;

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    /// Pre-computed lowercase name for matching — keeps search allocation-free.
    #[serde(skip)]
    pub name_lower: String,
}

fn index() -> &'static RwLock<Vec<FileEntry>> {
    static INDEX: OnceLock<RwLock<Vec<FileEntry>>> = OnceLock::new();
    INDEX.get_or_init(|| RwLock::new(Vec::new()))
}

/// Spawn a background thread that builds the index immediately and then
/// refreshes it every REFRESH_SECS. Cheap to call from setup; never blocks.
pub fn spawn() {
    std::thread::spawn(|| loop {
        if let Err(e) = build() {
            tracing::warn!("file index build failed: {e}");
        }
        std::thread::sleep(std::time::Duration::from_secs(REFRESH_SECS));
    });
}

fn build() -> Result<()> {
    let mut entries: Vec<FileEntry> = Vec::with_capacity(2048);
    for root in roots() {
        walk(&root, 0, &mut entries);
        if entries.len() >= MAX_ENTRIES { break; }
    }
    entries.sort_by(|a, b| a.name_lower.cmp(&b.name_lower));
    *index().write().unwrap() = entries;
    Ok(())
}

fn roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let home = PathBuf::from(home);
        for sub in ["Desktop", "Documents", "Downloads", "Pictures", "Videos", "Music"] {
            out.push(home.join(sub));
        }
    }
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<FileEntry>) {
    if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        if out.len() >= MAX_ENTRIES { break; }
        let path = entry.path();
        // Skip dot-directories and Windows system markers ($RECYCLE.BIN, etc).
        let Some(name_os) = path.file_name() else { continue };
        let Some(name) = name_os.to_str() else { continue };
        if name.starts_with('.') || name.starts_with('$') { continue; }
        if path.is_dir() {
            walk(&path, depth + 1, out);
        } else {
            out.push(FileEntry {
                name: name.to_string(),
                path: path.to_string_lossy().to_string(),
                name_lower: name.to_lowercase(),
            });
        }
    }
}

/// Search the index for entries matching `query`. Empty query returns
/// nothing — files are noisier than apps, so we don't surface them by
/// default the way the start-menu list does.
pub fn search(query: &str, limit: usize) -> Vec<FileEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() { return Vec::new(); }
    let guard = index().read().unwrap();
    let mut scored: Vec<(i32, &FileEntry)> = guard.iter()
        .filter_map(|e| score(&q, &e.name_lower).map(|s| (s, e)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
    scored.into_iter().take(limit).map(|(_, e)| e.clone()).collect()
}

/// Same scoring rubric as start_menu::score so apps and files rank
/// consistently when interleaved in spotlight results.
fn score(q: &str, name: &str) -> Option<i32> {
    crate::widgets::start_menu::score(q, name)
}
