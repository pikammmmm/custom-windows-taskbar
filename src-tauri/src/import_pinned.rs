use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

use windows::core::{Interface, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
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

    // COM init for this thread; release on drop.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let _guard = ComGuard;

    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let Ok(entry) = entry else { continue };
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()).map(|s| s.eq_ignore_ascii_case("lnk")) != Some(true) {
            continue;
        }
        match resolve_lnk(&p) {
            Ok(target) if Path::new(&target).is_file() => {
                let display = p.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                // Pre-warm the icon cache from the .lnk itself: it carries the
                // exact icon Windows displays on the taskbar (including any
                // custom IconLocation), which is closer to what users expect
                // than whatever the resolved exe happens to ship.
                if let Some(lnk_str) = p.to_str() {
                    icons::warm_cache_from(&target, lnk_str);
                }
                out.push(PinnedApp { path: target, display_name: display, icon_path: None });
            }
            Ok(_) => continue,
            Err(e) => tracing::warn!("skip lnk {}: {e}", p.display()),
        }
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

struct ComGuard;
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
