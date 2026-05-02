//! glassbar auto-updater.
//!
//! Hits the GitHub Releases API for the project's latest tag, compares it
//! to the version this binary was built against (CARGO_PKG_VERSION), and
//! if a newer release exists downloads the MSI and runs it through
//! msiexec. Console app on purpose — the user sees what's happening,
//! and a tail prompt keeps the window open after a one-shot run.
//!
//! Lives next to glassbar.exe in `Program Files\glassbar\` after MSI
//! install (bundled via tauri.conf.json bundle.resources). Standalone
//! download is also attached to GitHub Releases for portable users.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

const REPO: &str = "pikammmmm/custom-windows-taskbar";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn main() {
    println!("glassbar updater");
    println!("================");
    println!();
    println!("Currently installed: v{CURRENT}");

    let latest = match fetch_latest() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Could not contact GitHub: {e}");
            pause();
            return;
        }
    };
    let latest_tag = latest.tag_name.trim_start_matches('v');
    println!("Latest on GitHub:    v{latest_tag}");
    println!();

    if !is_newer(latest_tag, CURRENT) {
        println!("You're on the latest version. Nothing to do.");
        pause();
        return;
    }

    let Some(msi) = latest.assets.iter().find(|a| a.name.to_lowercase().ends_with(".msi")) else {
        eprintln!("Latest release has no MSI asset — can't auto-update.");
        eprintln!("Download manually from: https://github.com/{REPO}/releases/latest");
        pause();
        return;
    };

    println!("[*] Downloading {} ...", msi.name);
    let tmp = std::env::temp_dir().join(&msi.name);
    if let Err(e) = download(&msi.browser_download_url, &tmp) {
        eprintln!("    download failed: {e}");
        pause();
        return;
    }
    println!("    saved to {}", tmp.display());

    println!("[*] Stopping glassbar...");
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "glassbar.exe"])
        .output();
    std::thread::sleep(std::time::Duration::from_millis(600));

    println!("[*] Installing v{latest_tag}... (Windows Installer will pop a small progress bar)");
    // /qb = basic UI, no full UAC dialog spam, lets the user see something
    // is happening. Add /norestart so a Windows Installer hiccup doesn't
    // reboot the machine.
    let status = Command::new("msiexec")
        .args(["/i", tmp.to_str().unwrap_or(""), "/qb", "/norestart"])
        .status();
    let _ = std::fs::remove_file(&tmp);

    match status {
        Ok(s) if s.success() => {
            println!();
            println!("Updated to v{latest_tag}. Launching the new glassbar...");
            // Look for glassbar.exe next to us first (MSI install dir),
            // then fall back to whatever Windows can resolve via shell.
            let here = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("glassbar.exe")));
            if let Some(p) = here {
                if p.exists() {
                    let _ = Command::new(&p).spawn();
                }
            }
        }
        Ok(s) => eprintln!("Installer exited with status {s}"),
        Err(e) => eprintln!("Couldn't run msiexec: {e}"),
    }

    pause();
}

/// True when `a` is strictly newer than `b` (semver-ish comparison on
/// dot-separated numeric components). Falls back to lexicographic if
/// either side has non-numeric pieces.
fn is_newer(a: &str, b: &str) -> bool {
    let parts = |s: &str| s.split('.').filter_map(|x| x.parse::<u32>().ok()).collect::<Vec<_>>();
    let (av, bv) = (parts(a), parts(b));
    if av.is_empty() || bv.is_empty() { return a > b; }
    av > bv
}

fn fetch_latest() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(15))
        .build();
    let body: Release = agent
        .get(&url)
        .set("User-Agent", "glassbar-updater")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(body)
}

fn download(url: &str, target: &PathBuf) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(180))
        .build();
    let resp = agent
        .get(url)
        .set("User-Agent", "glassbar-updater")
        .call()
        .map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(target).map_err(|e| e.to_string())?;
    std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
    Ok(())
}

fn pause() {
    println!();
    println!("Press Enter to close.");
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
}
