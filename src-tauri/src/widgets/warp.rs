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

/// Process-wide singleton probe — created by widget_state on startup
/// and looked up by warp_toggle so refresh_now() can fire without
/// needing to thread the Probe handle through Tauri's State manager.
fn singleton() -> &'static std::sync::OnceLock<Probe> {
    static SLOT: std::sync::OnceLock<Probe> = std::sync::OnceLock::new();
    &SLOT
}

pub fn install_singleton(p: Probe) {
    let _ = singleton().set(p);
}

pub fn refresh_global() {
    if let Some(p) = singleton().get() {
        p.refresh_now();
    }
}

#[derive(Clone)]
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

    /// Force an immediate status re-read and update the cached value.
    /// Called from the toggle path so the HUD sees the new state right
    /// away instead of waiting up to 5s for the next scheduled poll —
    /// without this the user clicked, snapshot still reported the OLD
    /// state, the next click sent the same command again, and it
    /// looked like nothing was happening.
    pub fn refresh_now(&self) {
        self.state.store(read_once(), Ordering::Relaxed);
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
    // Cloudflare ships the CLI under several layouts depending on
    // installer vintage and MSI vs Store-app provenance. Previously we
    // only checked the two stable Program Files paths and quietly bailed
    // on every other layout — manifesting to users as "the WARP button
    // does nothing." Now we walk a wider list and also honour the
    // PROGRAMFILES env var so non-default install drives work too.
    let mut candidates: Vec<std::path::PathBuf> = vec![
        r"C:\Program Files\Cloudflare\Cloudflare WARP\warp-cli.exe".into(),
        r"C:\Program Files (x86)\Cloudflare\Cloudflare WARP\warp-cli.exe".into(),
        r"C:\Program Files\Cloudflare Inc\Cloudflare WARP\warp-cli.exe".into(),
    ];
    if let Ok(pf) = std::env::var("PROGRAMFILES") {
        candidates.push(format!(r"{pf}\Cloudflare\Cloudflare WARP\warp-cli.exe").into());
        candidates.push(format!(r"{pf}\Cloudflare Inc\Cloudflare WARP\warp-cli.exe").into());
    }
    if let Ok(pf) = std::env::var("PROGRAMFILES(X86)") {
        candidates.push(format!(r"{pf}\Cloudflare\Cloudflare WARP\warp-cli.exe").into());
    }
    let found = candidates.iter().find(|p| p.is_file()).cloned();
    if found.is_none() {
        crate::glog!("warp: warp-cli.exe not found in any candidate path");
    }
    found
}

/// Resolve the path to the GUI Cloudflare WARP app. Used as a fallback
/// when the CLI is missing — clicking the HUD button still opens the app
/// so the user can act, instead of failing silently.
fn warp_app_path() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = vec![
        r"C:\Program Files\Cloudflare\Cloudflare WARP\Cloudflare WARP.exe".into(),
        r"C:\Program Files (x86)\Cloudflare\Cloudflare WARP\Cloudflare WARP.exe".into(),
    ];
    if let Ok(pf) = std::env::var("PROGRAMFILES") {
        candidates.push(format!(r"{pf}\Cloudflare\Cloudflare WARP\Cloudflare WARP.exe").into());
    }
    candidates.iter().find(|p| p.is_file()).cloned()
}

/// Issue a connect / disconnect through the CLI. If the CLI isn't
/// installed but the GUI app is, fall back to launching the app so the
/// user has somewhere to land instead of a silent dead-button. Logs the
/// path it tried so debug.log captures every failed click.
pub fn toggle(connect: bool) -> Result<(), String> {
    crate::glog!("warp: toggle(connect={connect}) called");
    let cli = match warp_cli_path() {
        Some(c) => c,
        None => {
            // Fallback: open the GUI app. The user can finish the
            // connect/disconnect there.
            if let Some(app) = warp_app_path() {
                crate::glog!("warp: CLI missing, launching GUI: {}", app.display());
                let _ = Command::new(&app).spawn();
                return Err("warp-cli not found — launched WARP app instead".into());
            }
            crate::glog!("warp: neither CLI nor GUI app found");
            return Err("Cloudflare WARP isn't installed".into());
        }
    };
    crate::glog!("warp: using CLI at {}", cli.display());
    let cmd = if connect { "connect" } else { "disconnect" };
    let output = Command::new(&cli)
        .arg(cmd)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| {
            crate::glog!("warp: spawn failed: {e}");
            format!("warp-cli {cmd} spawn failed: {e}")
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        crate::glog!("warp: {cmd} non-zero exit. stdout={stdout:?} stderr={stderr:?}");
        return Err(format!("warp-cli {cmd}: {stderr}"));
    }
    crate::glog!("warp: {cmd} succeeded");
    Ok(())
}
