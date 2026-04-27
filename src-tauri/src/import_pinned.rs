use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

use std::cell::Cell;
use windows::core::{Interface, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

use crate::icons;
use crate::pinned::PinnedApp;

/// Public accessor for the Windows-taskbar pinned-shortcut folder so callers
/// can `notify::watch` it without duplicating the path logic.
pub fn taskbar_pin_dir() -> Option<PathBuf> {
    quick_launch_taskbar_dir()
}

/// Read the per-user Windows-taskbar pinned-shortcut folder and resolve each
/// .lnk into a `PinnedApp`. Items whose target no longer exists are skipped.
pub fn read_taskbar_pins() -> Result<Vec<PinnedApp>> {
    let dir = quick_launch_taskbar_dir()
        .ok_or_else(|| anyhow!("could not resolve Quick Launch TaskBar dir"))?;
    if !dir.is_dir() {
        return Ok(vec![]);
    }

    // COM init for this thread. We deliberately do NOT uninit on exit:
    // icons::warm_cache_from caches an "is COM inited" flag in thread_local,
    // and uninitializing here would invalidate that without it knowing.
    ensure_com();

    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let Ok(entry) = entry else { continue };
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()).map(|s| s.eq_ignore_ascii_case("lnk")) != Some(true) {
            continue;
        }
        let display = p.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let target = match resolve_lnk(&p) {
            Ok(t) if Path::new(&t).is_file() => t,
            // Some pins (File Explorer, This PC, Recycle Bin) are shell-folder
            // shortcuts whose target is a CLSID PIDL, not a file path. We
            // can't `is_file()` those, so fall back to a small known-shell map
            // by display name.
            _ => match shell_target_for(&display) {
                Some(t) => t,
                None => continue,
            },
        };
        if let Some(lnk_str) = p.to_str() {
            // Pre-warm the icon cache from the .lnk itself — it carries the
            // exact icon Windows draws on the taskbar (custom IconLocation
            // included), which beats whatever the resolved exe ships.
            icons::warm_cache_from(&target, lnk_str);
        }
        out.push(PinnedApp { path: target, display_name: display, icon_path: None });
    }
    Ok(out)
}

fn quick_launch_taskbar_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata)
        .join("Microsoft")
        .join("Internet Explorer")
        .join("Quick Launch")
        .join("User Pinned")
        .join("TaskBar"))
}

/// Map a known shell-folder shortcut display name to a launchable exe path.
fn shell_target_for(display_name: &str) -> Option<String> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    match display_name.to_lowercase().as_str() {
        "file explorer" | "explorer" | "this pc" => Some(format!(r"{windir}\explorer.exe")),
        _ => None,
    }
}

/// One entry in the user's Recent folder, resolved to its target file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecentFile {
    pub name: String,
    pub path: String,
    pub modified_secs: u64,
}

/// Read the user's Recent folder (`%APPDATA%\Microsoft\Windows\Recent\`),
/// resolve each .lnk shortcut to its target file, and return the most
/// recently modified entries first. Caps `limit` to keep the menu tight.
pub fn recent_files(limit: usize) -> Result<Vec<RecentFile>> {
    let appdata = std::env::var_os("APPDATA")
        .ok_or_else(|| anyhow!("no %APPDATA%"))?;
    let dir = PathBuf::from(appdata)
        .join("Microsoft").join("Windows").join("Recent");
    if !dir.is_dir() { return Ok(vec![]); }
    ensure_com();

    // Collect (modified_time, path) so we can sort newest-first.
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)?
        .filter_map(|r| r.ok())
        .filter(|e| e.path().extension()
            .and_then(|x| x.to_str())
            .map(|s| s.eq_ignore_ascii_case("lnk"))
            == Some(true))
        .filter_map(|e| {
            let path = e.path();
            let mtime = e.metadata().ok().and_then(|m| m.modified().ok())?;
            Some((mtime, path))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    let mut out = Vec::with_capacity(limit);
    for (mtime, lnk) in entries {
        if out.len() >= limit { break; }
        let Ok(target) = resolve_lnk(&lnk) else { continue };
        if target.is_empty() || !Path::new(&target).exists() { continue; }
        let name = lnk.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let secs = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(RecentFile { name, path: target, modified_secs: secs });
    }
    Ok(out)
}

/// Resolve a dropped path into (path, display_name). Accepts:
///   - `.lnk` → resolves the shortcut's target so we pin the actual exe
///   - any other existing file → pinned as-is, opened with the default
///     handler when clicked (docs, scripts, archives, anything)
/// Returns None for directories or paths that don't exist.
pub fn resolve_drop(path: &Path) -> Option<(String, String)> {
    if !path.exists() { return None; }
    let display = path.file_stem().and_then(|s| s.to_str())?.to_string();
    let ext = path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase());
    if ext.as_deref() == Some("lnk") {
        ensure_com();
        let target = resolve_lnk(path).ok()?;
        if !Path::new(&target).is_file() { return None; }
        return Some((target, display));
    }
    if path.is_file() {
        Some((path.to_string_lossy().to_string(), display))
    } else {
        None
    }
}

fn resolve_lnk(lnk: &Path) -> Result<String> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let pfile: IPersistFile = link.cast()?;

        let wide: Vec<u16> = lnk.as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        pfile.Load(PCWSTR(wide.as_ptr()), STGM_READ)?;

        let mut buf = [0u16; 1024];
        // GetPath signature: pszFile (PWSTR), cchMaxPath, pfd (Option<*mut WIN32_FIND_DATAW>), fFlags
        link.GetPath(&mut buf, std::ptr::null_mut(), 0)?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(String::from_utf16_lossy(&buf[..len]))
    }
}

fn ensure_com() {
    thread_local! { static INITED: Cell<bool> = const { Cell::new(false) }; }
    INITED.with(|i| {
        if !i.get() {
            unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); }
            i.set(true);
        }
    });
}
