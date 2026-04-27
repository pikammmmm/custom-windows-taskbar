//! Cloudflare WARP status probe + control.
//!
//! Shells out to `warp-cli status` on a background thread every few seconds.
//! `connect` / `disconnect` go through the same CLI so the user can toggle
//! straight from the HUD button.

use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Hide the cmd window that would otherwise flash for every CLI call.
const CREATE_NO_WINDOW: u32 = 0x08000000;

const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Encoded as u8 because we need a lock-free shared variable. 0 = unknown
/// (probe hasn't run / CLI absent), 1 = disconnected, 2 = connected.
const ST_UNKNOWN: u8 = 0;
const ST_DISCONNECTED: u8 = 1;
const ST_CONNECTED: u8 = 2;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WarpState {
    /// True when warp-cli is on the machine and we got a parseable status.
    pub installed: bool,
    /// True only when the CLI most recently reported "Connected".
    pub connected: bool,
}

pub struct Probe {
    state: Arc<AtomicU8>,
}

impl Probe {
    pub fn spawn() -> Self {
        let state = Arc::new(AtomicU8::new(ST_UNKNOWN));
        let s = state.clone();
        std::thread::spawn(move || {
            // Run the first probe immediately so the HUD doesn't sit on
            // "unknown" for the full poll interval after launch.
            s.store(read_once(), Ordering::Relaxed);
            loop {
                std::thread::sleep(POLL_INTERVAL);
                s.store(read_once(), Ordering::Relaxed);
            }
        });
        Self { state }
    }

    pub fn current(&self) -> WarpState {
        match self.state.load(Ordering::Relaxed) {
            ST_CONNECTED => WarpState { installed: true,  connected: true  },
            ST_DISCONNECTED => WarpState { installed: true,  connected: false },
            _ => WarpState { installed: false, connected: false },
        }
    }
}

fn read_once() -> u8 {
    let Some(cli) = warp_cli_path() else { return ST_UNKNOWN };
    let out = match Command::new(&cli)
        .arg("status")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => o,
        Err(_) => return ST_UNKNOWN,
    };
    if !out.status.success() { return ST_UNKNOWN; }
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    // warp-cli prints e.g. "Status update: Connected" or
    // "Status update: Disconnected" / "Connecting" / "Disconnecting".
    if text.contains("disconnected") || text.contains("disconnecting") {
        ST_DISCONNECTED
    } else if text.contains("connected") || text.contains("connecting") {
        ST_CONNECTED
    } else {
        ST_DISCONNECTED
    }
}

fn warp_cli_path() -> Option<std::path::PathBuf> {
    // The standard installer lays the CLI down here on x64 Windows. Earlier
    // versions sat under "Program Files (x86)" which we also check as a
    // courtesy.
    let candidates = [
        r"C:\Program Files\Cloudflare\Cloudflare WARP\warp-cli.exe",
        r"C:\Program Files (x86)\Cloudflare\Cloudflare WARP\warp-cli.exe",
    ];
    candidates.iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.is_file())
}

/// Issue a connect / disconnect through the CLI. Best-effort: if the CLI
/// isn't installed we fail loudly so the frontend can surface it.
pub fn toggle(connect: bool) -> Result<(), String> {
    let cli = warp_cli_path().ok_or_else(|| "warp-cli not found".to_string())?;
    let cmd = if connect { "connect" } else { "disconnect" };
    let status = Command::new(&cli)
        .arg(cmd)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("warp-cli {cmd} spawn failed: {e}"))?;
    if !status.success() {
        return Err(format!("warp-cli {cmd} returned non-zero"));
    }
    Ok(())
}
