//! Claude usage probe — sums tokens used in the current 5-hour rate-limit
//! block by walking `~/.claude/projects/**/*.jsonl`. The block starts on
//! the earliest message in the last 5 hours and resets 5 hours after that
//! start. Same windowing as ccusage / claude.ai consumer rate-limit.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BLOCK_DURATION_SECS: u64 = 5 * 3600;
// Add 30 min slack on top of the block when picking which files to read,
// in case mtime drifts behind the block start by a minute or two.
const FILE_SCAN_LOOKBACK: Duration = Duration::from_secs(BLOCK_DURATION_SECS + 30 * 60);
const REFRESH: Duration = Duration::from_secs(45);
// Heuristic cap for the percentage bar — between Pro (~1M / 5h) and Max
// (~5M+). The user can re-skin the bar later if they want a different
// denominator; bar fill is just a hint, not a hard limit.
const ESTIMATED_CAP: u64 = 2_000_000;

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct ClaudeUsageState {
    /// Unix seconds — when the current 5-hour block started. None = no
    /// activity in the lookback window (idle / cap recently reset).
    pub block_start: Option<i64>,
    /// Unix seconds — when the block resets (block_start + 5h).
    pub block_reset: Option<i64>,
    /// Total tokens used in the current block: input + output + cache.
    pub tokens_used: u64,
    /// Number of assistant turns seen in the block. Useful as a secondary
    /// readout when token counts are vague.
    pub messages: u32,
    /// Heuristic denominator the frontend can use for a percent bar. The
    /// bar fill = tokens_used / estimated_cap, clamped to 100%.
    pub estimated_cap: u64,
    /// Best-effort display name of the account whose transcripts we read,
    /// pulled from the local Claude Code OAuth store. None when the
    /// credentials file doesn't exist or doesn't expose an email.
    pub account: Option<String>,
}

pub struct Probe {
    state: Arc<RwLock<ClaudeUsageState>>,
}

impl Probe {
    pub fn spawn() -> Self {
        let state = Arc::new(RwLock::new(ClaudeUsageState {
            estimated_cap: ESTIMATED_CAP,
            ..Default::default()
        }));
        let s = state.clone();
        std::thread::spawn(move || loop {
            match scan() {
                Ok(new_state) => *s.write().unwrap() = new_state,
                Err(e) => tracing::debug!("claude_usage scan failed: {e}"),
            }
            std::thread::sleep(REFRESH);
        });
        Self { state }
    }

    pub fn current(&self) -> ClaudeUsageState {
        self.state.read().unwrap().clone()
    }
}

fn scan() -> anyhow::Result<ClaudeUsageState> {
    let projects_dir = std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join(".claude").join("projects"))
        .ok_or_else(|| anyhow::anyhow!("USERPROFILE not set"))?;
    if !projects_dir.is_dir() {
        return Ok(ClaudeUsageState { estimated_cap: ESTIMATED_CAP, ..Default::default() });
    }

    let now = SystemTime::now();
    let cutoff = now - FILE_SCAN_LOOKBACK;

    let mut entries: Vec<(SystemTime, u64)> = Vec::new();
    walk_collect(&projects_dir, cutoff, &mut entries);

    if entries.is_empty() {
        return Ok(ClaudeUsageState { estimated_cap: ESTIMATED_CAP, ..Default::default() });
    }

    // Block starts at the earliest message we still have, capped at "now - 5h".
    // Anything older than now - 5h is in a previous (already-reset) block.
    let earliest_block_start = now - Duration::from_secs(BLOCK_DURATION_SECS);
    let block_start = entries.iter()
        .map(|(t, _)| *t)
        .filter(|t| *t >= earliest_block_start)
        .min();
    let Some(block_start) = block_start else {
        return Ok(ClaudeUsageState { estimated_cap: ESTIMATED_CAP, ..Default::default() });
    };
    let block_reset = block_start + Duration::from_secs(BLOCK_DURATION_SECS);

    let in_block: Vec<&(SystemTime, u64)> = entries.iter()
        .filter(|(t, _)| *t >= block_start && *t <= block_reset)
        .collect();
    let tokens_used: u64 = in_block.iter().map(|(_, n)| *n).sum();
    let messages = in_block.len() as u32;

    Ok(ClaudeUsageState {
        block_start: to_unix(block_start),
        block_reset: to_unix(block_reset),
        tokens_used,
        messages,
        estimated_cap: ESTIMATED_CAP,
        account: read_account_email(),
    })
}

/// Best-effort: read the email from Claude Code's local credentials store.
/// The file format isn't a stable public contract — we look for an "email"
/// or "account" / "user" string anywhere in the JSON and return None on
/// any miss so a schema change just gracefully hides the label.
fn read_account_email() -> Option<String> {
    let home = std::env::var_os("USERPROFILE")?;
    for fname in [".credentials.json", "credentials.json"] {
        let p = PathBuf::from(&home).join(".claude").join(fname);
        let Ok(raw) = std::fs::read_to_string(&p) else { continue };
        let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(&raw) else { continue };
        if let Some(s) = find_string_field(&v, &["email", "user_email", "account_email"]) {
            return Some(s);
        }
        if let Some(s) = find_string_field(&v, &["account", "user", "name"]) {
            return Some(s);
        }
    }
    None
}

fn find_string_field(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    match v {
        serde_json::Value::Object(m) => {
            for k in keys {
                if let Some(serde_json::Value::String(s)) = m.get(*k) { return Some(s.clone()); }
            }
            for (_, child) in m {
                if let Some(s) = find_string_field(child, keys) { return Some(s); }
            }
            None
        }
        serde_json::Value::Array(a) => {
            for child in a {
                if let Some(s) = find_string_field(child, keys) { return Some(s); }
            }
            None
        }
        _ => None,
    }
}

fn walk_collect(dir: &Path, cutoff: SystemTime, out: &mut Vec<(SystemTime, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_collect(&path, cutoff, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            // Skip files whose mtime predates the lookback window. Saves
            // 99% of the parsing on long-lived ~/.claude/projects trees.
            if let Ok(meta) = entry.metadata() {
                if let Ok(mtime) = meta.modified() {
                    if mtime < cutoff { continue; }
                }
            }
            parse_jsonl(&path, cutoff, out);
        }
    }
}

fn parse_jsonl(path: &Path, cutoff: SystemTime, out: &mut Vec<(SystemTime, u64)>) {
    use std::io::{BufRead, BufReader};
    let Ok(file) = std::fs::File::open(path) else { return };
    let reader = BufReader::new(file);
    for line_result in reader.lines() {
        let Ok(line) = line_result else { continue };
        if line.is_empty() { continue; }
        let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(&line) else { continue };
        let Some(ts_str) = v.get("timestamp").and_then(|t| t.as_str()) else { continue };
        let Some(ts) = parse_iso8601(ts_str) else { continue };
        if ts < cutoff { continue; }

        // Only assistant turns carry usage. Sum input + output + cache
        // CREATION but exclude cache READS — every turn re-reads the
        // cached prompt so including them inflates a 200-turn session into
        // hundreds of millions of phantom tokens. Cache reads also cost
        // ~1/10 the price of fresh input on Anthropic billing so they
        // aren't what users watch on a rate-limit dashboard anyway.
        let Some(usage) = v.get("message").and_then(|m| m.get("usage")) else { continue };
        let total =
            usage.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0)
            + usage.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0)
            + usage.get("cache_creation_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
        if total > 0 {
            out.push((ts, total));
        }
    }
}

fn parse_iso8601(s: &str) -> Option<SystemTime> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc).into())
}

fn to_unix(t: SystemTime) -> Option<i64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}
