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
