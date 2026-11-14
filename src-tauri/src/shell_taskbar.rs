use anyhow::Result;
use windows::Win32::Foundation::{HWND, LPARAM, BOOL, TRUE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    VK_LWIN, VK_M,
};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABM_GETSTATE, ABM_SETSTATE, ABS_AUTOHIDE, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, ShowWindow, SHOW_WINDOW_CMD, SW_HIDE, SW_SHOW,
};

/// Hide explorer's `Shell_TrayWnd` and every `Shell_SecondaryTrayWnd`
/// (multi-monitor secondary trays). Returns the count of windows hidden.
pub fn hide_windows_taskbar() -> Result<usize> {
    apply_to_taskbars(SW_HIDE)
}

pub fn show_windows_taskbar() -> Result<usize> {
    apply_to_taskbars(SW_SHOW)
}

/// Context handed to the EnumWindows callback so it can skip glassbar's own
/// tray-host window and tally how many taskbars it touched.
struct ApplyCtx {
    cmd: SHOW_WINDOW_CMD,
    host: isize,
    count: usize,
}

/// CRITICAL coexistence rule: glassbar's tray host registers its OWN
/// `Shell_TrayWnd` class + window (see `tray_host`) and makes it topmost, so a
/// naive `FindWindow("Shell_TrayWnd")` now returns OUR window, not explorer's.
/// We therefore enumerate every top-level window and apply the show/hide to all
/// `Shell_TrayWnd` / `Shell_SecondaryTrayWnd` windows EXCEPT our host — keeping
/// us hiding explorer's taskbar instead of fighting ourselves.
fn apply_to_taskbars(cmd: SHOW_WINDOW_CMD) -> Result<usize> {
    let mut ctx = ApplyCtx {
        cmd,
        host: crate::tray_host::host_hwnd(),
        count: 0,
    };
    unsafe {
        let _ = EnumWindows(Some(apply_proc), LPARAM(&mut ctx as *mut _ as isize));
    }
    Ok(ctx.count)
}

unsafe extern "system" fn apply_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut ApplyCtx);
    let mut buf = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut buf);
    if len > 0 {
        let class = String::from_utf16_lossy(&buf[..len as usize]);
        let is_taskbar = class == "Shell_TrayWnd" || class == "Shell_SecondaryTrayWnd";
        // Skip glassbar's own tray-host Shell_TrayWnd.
        if is_taskbar && hwnd.0 as isize != ctx.host {
            let _ = ShowWindow(hwnd, ctx.cmd);
            ctx.count += 1;
        }
    }
    TRUE
}

/// HWND of EXPLORER's primary taskbar (0 if explorer isn't running). Skips
/// glassbar's own tray-host window, which carries the same class name.
pub fn explorer_tray_hwnd() -> isize {
    struct Find {
        host: isize,
        found: isize,
    }
    unsafe extern "system" fn find_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let f = &mut *(lparam.0 as *mut Find);
        let mut buf = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut buf);
        if len > 0
            && hwnd.0 as isize != f.host
            && String::from_utf16_lossy(&buf[..len as usize]) == "Shell_TrayWnd"
        {
            f.found = hwnd.0 as isize;
            return BOOL(0); // stop enumerating
        }
        TRUE
    }
    let mut f = Find { host: crate::tray_host::host_hwnd(), found: 0 };
    unsafe {
        let _ = EnumWindows(Some(find_proc), LPARAM(&mut f as *mut _ as isize));
    }
    f.found
}

fn appbar_message(msg: u32, tray: isize, lparam: isize) -> usize {
    let mut abd = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: HWND(tray as *mut _),
        lParam: LPARAM(lparam),
        ..Default::default()
    };
    unsafe { SHAppBarMessage(msg, &mut abd) }
}

/// Give the taskbar's reserved screen strip back to applications.
///
/// `ShowWindow(SW_HIDE)` only makes Explorer's taskbar invisible — it stays a
/// registered appbar, so the work area still stops short of the screen edge
/// and maximized windows leave a dead band where the taskbar used to be.
/// Switching the taskbar to auto-hide is the supported way to release that
/// reservation. The previous state is written to a marker file so it survives
/// a crash or a `taskkill` (updater / uninstaller) and can be put back later.
///
/// Call BEFORE the tray host starts and before hiding the taskbar.
pub fn reclaim_work_area() {
    let tray = explorer_tray_hwnd();
    if tray == 0 {
        return;
    }
    let prev = appbar_message(ABM_GETSTATE, tray, 0) as u32;
    if prev & ABS_AUTOHIDE != 0 {
        // Already auto-hidden — the user's own setting, or ours from a run
        // that never exited cleanly (the marker, if any, still has the original).
        return;
    }
    if let Ok(marker) = crate::config::taskbar_restore_path() {
        if !marker.exists() {
            let _ = std::fs::write(&marker, prev.to_string());
        }
    }
    appbar_message(ABM_SETSTATE, tray, (prev | ABS_AUTOHIDE) as isize);
    crate::glog!("[shell_taskbar] taskbar set to auto-hide to free the work area (was state {prev})");
}

/// Undo `reclaim_work_area` if (and only if) glassbar was the one that turned
/// auto-hide on. Idempotent — the marker is removed after the first restore.
pub fn restore_work_area() {
    let Ok(marker) = crate::config::taskbar_restore_path() else { return };
    let Ok(text) = std::fs::read_to_string(&marker) else { return };
    let tray = explorer_tray_hwnd();
    if tray == 0 {
        return; // keep the marker; the next run / the uninstaller can restore
    }
    let prev = text.trim().parse::<u32>().unwrap_or(0);
    appbar_message(ABM_SETSTATE, tray, prev as isize);
    let _ = std::fs::remove_file(&marker);
}

/// Send a single tap of the Windows key — opens / toggles the Start menu.
pub fn tap_start_key() -> Result<()> {
    unsafe {
        let inputs = [
            kb(VK_LWIN, false),
            kb(VK_LWIN, true),
        ];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

/// Send Win+M — minimizes every minimizable top-level window. Unlike Win+D
/// this is not a toggle; restoring is up to the user.
pub fn minimize_all_windows() -> Result<()> {
    unsafe {
        let inputs = [
            kb(VK_LWIN, false),
            kb(VK_M, false),
            kb(VK_M, true),
            kb(VK_LWIN, true),
        ];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

fn kb(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { Default::default() },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
