//! Lightweight file logger that writes to `%APPDATA%\glassbar\debug.log`.
//! Used as the durable diagnostic channel for things that fire from
//! background threads where stderr would be invisible (clipboard polling,
//! key hook, audio endpoint reads). The on-disk file lets the user grab a
//! snapshot to share without having to attach a debugger or rebuild.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 1_000_000;

fn log_path() -> Option<PathBuf> {
    crate::config::data_dir().ok().map(|d| d.join("debug.log"))
}

/// One-shot init — writes a session-start banner so successive launches
/// are easy to tell apart in the log file.
pub fn init() {
    log("====================== glassbar session start ======================");
}

/// Append `msg` to the log file with a millisecond-precision timestamp.
/// Silent on any I/O failure — logging must never crash the host process.
pub fn log(msg: &str) {
    static MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = MUTEX.get_or_init(|| Mutex::new(())).lock().unwrap();
    let Some(path) = log_path() else { return };

    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = format!("[{now}] {msg}\n");

    // Truncate-rotate when the file grows past MAX_BYTES so a runaway
    // logger can't fill the disk. We just rewrite from scratch with the
    // new line — no rolling N-of-M files; the user only ever needs the
    // most recent activity.
    let truncate = std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false);
    let result = if truncate {
        std::fs::write(&path, line.as_bytes())
    } else {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(line.as_bytes()))
    };
    let _ = result;
}

#[macro_export]
macro_rules! glog {
    ($($arg:tt)*) => {
        $crate::logger::log(&format!($($arg)*))
    };
}
