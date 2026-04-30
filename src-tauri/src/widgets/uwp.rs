//! Enumerate UWP / Microsoft Store apps via Windows' Get-StartApps cmdlet.
//! Store apps don't have regular `.lnk` shortcuts so the start_menu walk
//! misses them; this fills the gap. Apps are launched via
//! `explorer.exe shell:AppsFolder\<AppID>` — that's how the OS itself
//! launches them from Start.

use serde::Deserialize;
use std::process::Command;

use crate::win32::CommandHidden;

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct StartApp {
    Name: String,
    AppID: String,
}

/// Run Get-StartApps and return (display name, AppID) pairs. Returns an
/// empty Vec on any failure — caller treats this as supplementary data,
/// so a missing PowerShell or odd shell isn't fatal.
pub fn enumerate() -> Vec<(String, String)> {
    // Hidden window + non-interactive so we don't flash a console on
    // startup. -ExecutionPolicy Bypass guards against locked-down hosts.
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-WindowStyle", "Hidden",
            "-NonInteractive",
            "-ExecutionPolicy", "Bypass",
            "-Command",
            "Get-StartApps | ConvertTo-Json -Compress",
        ])
        .hidden()
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() { return Vec::new(); }
    let stdout = String::from_utf8_lossy(&out.stdout);

    // PowerShell serialises a single-element list as a single object, not
    // a 1-element array — try array shape first, then fall back.
    if let Ok(items) = serde_json::from_str::<Vec<StartApp>>(&stdout) {
        return items.into_iter().map(|a| (a.Name, a.AppID)).collect();
    }
    if let Ok(item) = serde_json::from_str::<StartApp>(&stdout) {
        return vec![(item.Name, item.AppID)];
    }
    Vec::new()
}
