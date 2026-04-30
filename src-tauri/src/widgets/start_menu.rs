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

/// Spawn a background thread that builds the index immediately and refreshes
/// it every 5 minutes — catches newly installed apps without requiring a
/// glassbar restart.
pub fn spawn() {
    std::thread::spawn(|| loop {
        if let Err(e) = build() {
            tracing::warn!("start_menu index build failed: {e}");
        }
        std::thread::sleep(std::time::Duration::from_secs(300));
    });
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

    // Supplement with UWP / Store apps — these don't have .lnk shortcuts so
    // the walk above misses them entirely. We dedupe by lowered name so a
    // Store app that ALSO has a .lnk (rare) doesn't appear twice.
    let known: std::collections::HashSet<String> = entries.iter()
        .map(|e| e.name.to_lowercase())
        .collect();
    for (name, app_id) in crate::widgets::uwp::enumerate() {
        if known.contains(&name.to_lowercase()) { continue; }
        // shell:AppsFolder\<AppID> is the canonical launch path; commands::launch
        // detects the `shell:` prefix and routes via explorer.exe.
        let path = format!("shell:AppsFolder\\{}", app_id);
        let name_lower = name.to_lowercase();
        entries.push(AppEntry { name, path, name_lower });
    }

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
///   5 = name starts with q                               (e.g. "chr"        → "chrome")
///   4 = q is exactly the acronym of name's words         (e.g. "vsc"        → "visual studio code")
///   3 = acronym prefix OR word-start match               (e.g. "vs"         → "visual studio code")
///   2 = q appears anywhere in name                       (e.g. "stud"       → "visual studio code")
///   1 = q is a subsequence of name (loose fallback)      (e.g. "vsco"       → "visual studio code")
///   0 = q matches name within edit-distance budget       (typo: "minceraft" → "minecraft")
/// Tier 1-5 first run on the original strings, then on space/punctuation-
/// collapsed copies — that's how "auto clicker" lands "AutoClicker.lnk".
pub fn score(q: &str, name: &str) -> Option<i32> {
    if q.is_empty() { return None; }

    if let Some(s) = score_strict(q, name) { return Some(s); }

    // Strip whitespace + separators on both sides and try again. Capped
    // at 4 so a real prefix on the original always beats a collapsed one.
    let q_collapsed = collapse_separators(q);
    let n_collapsed = collapse_separators(name);
    if (q_collapsed != q || n_collapsed != name) && !q_collapsed.is_empty() {
        if let Some(s) = score_strict(&q_collapsed, &n_collapsed) {
            return Some(s.min(4));
        }
        // Typo tolerance — single-edit forgiving for short queries, ~25%
        // for longer ones. Operates on the collapsed strings so "minecraft
        // launcher" with a missing letter still rescues.
        let max_edit = if q_collapsed.len() <= 4 { 1 } else { q_collapsed.len() / 4 + 1 };
        if fuzzy_match(&q_collapsed, &n_collapsed, max_edit) { return Some(0); }
    } else {
        let max_edit = if q.len() <= 4 { 1 } else { q.len() / 4 + 1 };
        if fuzzy_match(q, name, max_edit) { return Some(0); }
    }

    None
}

fn score_strict(q: &str, name: &str) -> Option<i32> {
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

/// Drop everything that isn't a letter or digit. "Auto Clicker" → "autoclicker",
/// "Visual Studio Code" → "visualstudiocode", "minecraft.launcher" → "minecraftlauncher".
fn collapse_separators(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect()
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

/// True iff `q` matches some substring of `name` within `max_edit`
/// Levenshtein distance. Uses a sliding window of size q.len()+max_edit
/// so edit-distance never has to consider segments that can't possibly
/// fit, keeping per-name cost ~O(window² · slides) which is small at the
/// query lengths spotlight sees.
fn fuzzy_match(q: &str, name: &str, max_edit: usize) -> bool {
    if q.len() <= 1 { return false; } // 1-char fuzzy is just noise
    let q_chars: Vec<char> = q.chars().collect();
    let n_chars: Vec<char> = name.chars().collect();
    if n_chars.len() + max_edit < q_chars.len() { return false; }

    let window = q_chars.len() + max_edit;
    if n_chars.len() <= window {
        return levenshtein(&q_chars, &n_chars) <= max_edit;
    }
    for start in 0..=(n_chars.len() - window) {
        let slice = &n_chars[start..start + window];
        if levenshtein(&q_chars, slice) <= max_edit {
            return true;
        }
    }
    false
}

/// Classic Levenshtein with a rolling two-row buffer. Returns the minimum
/// edit distance between `a` and `b`.
fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.is_empty() { return b.len(); }
    if b.is_empty() { return a.len(); }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = std::cmp::min(
                std::cmp::min(prev[j] + 1, curr[j - 1] + 1),
                prev[j - 1] + cost,
            );
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}
