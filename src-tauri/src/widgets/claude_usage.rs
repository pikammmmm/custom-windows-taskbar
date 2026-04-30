//! Claude usage probe — sums tokens used in the current 5-hour rate-limit
//! block by walking `~/.claude/projects/**/*.jsonl`. The block starts on
//! the earliest message in the last 5 hours and resets 5 hours after that
//! start. Same windowing as ccusage / claude.ai consumer rate-limit.

use chrono::{DateTime, Timelike, Utc};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BLOCK_DURATION_SECS: u64 = 5 * 3600;
// Lookback wide enough to walk a full day of blocks — the current block's
// boundary is determined by where the *previous* block expired, so missing
// older messages would push the reported start time forward incorrectly.
// 24h covers any realistic continuous session; the mtime filter still
// keeps the parse cost low.
const FILE_SCAN_LOOKBACK: Duration = Duration::from_secs(24 * 3600);
const REFRESH: Duration = Duration::from_secs(45);
// Heuristic cap for the percentage bar. Pro users land around 1-2M / 5h,
// Max users are routinely 5-10M. 8M is a Max-leaning middle so the bar
// stays informative for either tier without saturating instantly.
const ESTIMATED_CAP: u64 = 8_000_000;

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

    let idle_state = ClaudeUsageState {
        estimated_cap: ESTIMATED_CAP,
        account: read_account_email(),
        ..Default::default()
    };

    if entries.is_empty() {
        return Ok(idle_state);
    }

    // Anchored 5-hour blocks: a block begins when the user sends a message
    // after >=5h of silence, and lasts exactly 5h from that timestamp —
    // matching how Claude.ai's rate limit actually works. Walk the sorted
    // entries forward, opening a new block each time a message lands at or
    // after the current block's end.
    entries.sort_by_key(|(t, _)| *t);
    struct Block { start: SystemTime, end: SystemTime, tokens: u64, messages: u32 }
    let mut blocks: Vec<Block> = Vec::new();
    for (ts, tokens) in &entries {
        let extends_current = blocks.last().is_some_and(|b| *ts < b.end);
        if extends_current {
            let b = blocks.last_mut().unwrap();
            b.tokens += tokens;
            b.messages += 1;
        } else {
            // Anthropic anchors a new rate-limit window to the start of the
            // hour of the first message — that's what Claude.ai's "resets
            // at HH:00" message uses. Floor to the hour so our reset time
            // matches what the user sees in the official UI.
            let start = floor_to_hour(*ts);
            blocks.push(Block {
                start,
                end: start + Duration::from_secs(BLOCK_DURATION_SECS),
                tokens: *tokens,
                messages: 1,
            });
        }
    }

    // Most recent block is the one we care about. If it's already past its
    // reset window, the user is between blocks — show idle until the next
    // message kicks off a fresh one.
    let Some(last) = blocks.last() else { return Ok(idle_state); };
    if now >= last.end { return Ok(idle_state); }

    Ok(ClaudeUsageState {
        block_start: to_unix(last.start),
        block_reset: to_unix(last.end),
        tokens_used: last.tokens,
        messages: last.messages,
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

/// Floor a `SystemTime` to the start of its UTC hour. Round-trips through
/// chrono so we don't have to do the leap-aware arithmetic ourselves.
fn floor_to_hour(t: SystemTime) -> SystemTime {
    let dt: DateTime<Utc> = t.into();
    let floored = dt
        .with_minute(0).unwrap_or(dt)
        .with_second(0).unwrap_or(dt)
        .with_nanosecond(0).unwrap_or(dt);
    floored.into()
}

fn to_unix(t: SystemTime) -> Option<i64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}
