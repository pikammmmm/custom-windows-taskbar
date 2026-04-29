//! Index of installed apps for the spotlight launcher. Built once at app
//! start by walking the per-user + all-users Start Menu folders, resolving
//! every .lnk shortcut to its target executable.

use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::import_pinned;

#[derive(Debug, Clone, Serialize)]
pub struct AppEntry {
    pub name: String,
    pub path: String,
    /// Lowercased name used for matching — pre-computed so search isn't
    /// allocating a new string per entry per keystroke.
    #[serde(skip)]
    pub name_lower: String,
}

fn index() -> &'static RwLock<Vec<AppEntry>> {
    static INDEX: OnceLock<RwLock<Vec<AppEntry>>> = OnceLock::new();
    INDEX.get_or_init(|| RwLock::new(Vec::new()))
}

/// Walk the Start Menu folders and populate the global index. Runs in a
/// background thread at app start; subsequent reads are lock-free fast.
pub fn build() -> Result<()> {
    let mut entries: Vec<AppEntry> = Vec::with_capacity(256);

    for root in start_menu_roots() {
        walk(&root, &mut entries);
    }

    // Dedupe by exe path: many apps install both a per-user and all-users
    // shortcut pointing at the same target. Keep the first display name.
    entries.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    entries.dedup_by(|a, b| a.path.eq_ignore_ascii_case(&b.path));
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    *index().write().unwrap() = entries;
    Ok(())
}

fn start_menu_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        out.push(PathBuf::from(appdata)
            .join("Microsoft").join("Windows").join("Start Menu").join("Programs"));
    }
    if let Some(programdata) = std::env::var_os("PROGRAMDATA") {
        out.push(PathBuf::from(programdata)
            .join("Microsoft").join("Windows").join("Start Menu").join("Programs"));
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<AppEntry>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if is_lnk(&path) {
            let Some((target, name)) = import_pinned::resolve_drop(&path) else { continue };
            // Filter out non-launchable .lnks (folder targets, web URLs, etc.).
            // resolve_drop already covers .exe; we accept the same.
            out.push(AppEntry {
                name: name.clone(),
                path: target,
                name_lower: name.to_lowercase(),
            });
        }
    }
}

fn is_lnk(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|s| s.eq_ignore_ascii_case("lnk")) == Some(true)
}

/// Search the index for entries matching `query`. Returns up to `limit`
/// results, ranked by a simple match score (prefix match > word-start
/// match > substring match). Empty query returns the alphabetical top.
pub fn search(query: &str, limit: usize) -> Vec<AppEntry> {
    let q = query.trim().to_lowercase();
    let guard = index().read().unwrap();
    if q.is_empty() {
        return guard.iter().take(limit).cloned().collect();
    }

    let mut scored: Vec<(i32, &AppEntry)> = guard.iter()
        .filter_map(|e| score(&q, &e.name_lower).map(|s| (s, e)))
        .collect();
    // Higher score first; stable name order on ties.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
    scored.into_iter().take(limit).map(|(_, e)| e.clone()).collect()
}

/// Score how well `name` matches `q`. None = no match. Higher = better.
///   5 = name starts with q                               (e.g. "chr" → "chrome")
///   4 = q is exactly the acronym of name's words         (e.g. "vsc" → "visual studio code")
///   3 = q is a prefix of the acronym, OR a word-start    (e.g. "vs"  → "visual studio code")
///   2 = q appears anywhere in name                       (e.g. "stud" → "visual studio code")
///   1 = q is a subsequence of name (fuzzy fallback)      (e.g. "vsco" → "visual studio code")
pub fn score(q: &str, name: &str) -> Option<i32> {
    if name.starts_with(q) { return Some(5); }
    let acronym: String = name
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| w.chars().next())
        .collect();
    if acronym == q { return Some(4); }
    if !q.is_empty() && acronym.starts_with(q) { return Some(3); }
    for word in name.split(|c: char| !c.is_alphanumeric()) {
        if word.starts_with(q) { return Some(3); }
    }
    if name.contains(q) { return Some(2); }
    if is_subsequence(q, name) { return Some(1); }
    None
}

/// True iff every char of `q` appears in `name` in order (not necessarily
/// contiguous). Cheap O(n) walk.
pub fn is_subsequence(q: &str, name: &str) -> bool {
    let mut q_chars = q.chars();
    let Some(mut target) = q_chars.next() else { return true };
    for c in name.chars() {
        if c == target {
            match q_chars.next() {
                Some(next) => target = next,
                None => return true,
            }
        }
    }
    false
}
