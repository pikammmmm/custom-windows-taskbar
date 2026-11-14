use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{HWND, BOOL, LPARAM, TRUE, CloseHandle};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowTextW, IsWindowVisible, GetWindowLongW, GWL_EXSTYLE,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, GW_OWNER, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow,
    SW_MINIMIZE, SW_RESTORE, IsIconic, GetForegroundWindow, PostMessageW, WM_CLOSE,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
    PROCESS_NAME_FORMAT, TerminateProcess,
};

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub exe_path: String,
}

pub fn enumerate_windows() -> Result<Vec<WindowInfo>> {
    let mut results: Vec<WindowInfo> = Vec::new();
    unsafe {
        EnumWindows(
            Some(enum_proc),
            LPARAM(&mut results as *mut _ as isize),
        )?;
    }
    Ok(results)
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let results = &mut *(lparam.0 as *mut Vec<WindowInfo>);

    if !IsWindowVisible(hwnd).as_bool() {
        return TRUE;
    }
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
    if (ex_style as u32) & WS_EX_TOOLWINDOW.0 != 0 {
        return TRUE;
    }

    // DWM cloaking — modern Win10/11 UWP apps suspend background
    // windows by cloaking them. They pass IsWindowVisible (the W flag
    // is still set) but the OS doesn't actually render them. Win+W
    // widget hosts, Xbox Game Bar pinned widgets, frozen Store apps
    // all show up here. The standard "is this window real" check is
    // DwmGetWindowAttribute(DWMWA_CLOAKED) — non-zero = cloaked.
    let mut cloaked: i32 = 0;
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut _ as *mut _,
        std::mem::size_of::<i32>() as u32,
    ).is_ok() && cloaked != 0
    {
        return TRUE;
    }

    // Owner check — popups, tooltips, and child-style helper windows
    // get owners. They're alt-tab-invisible too. Skip unless the window
    // explicitly opted into being treated as an app window
    // (WS_EX_APPWINDOW), which is what genuine secondary main windows
    // use.
    let owner = GetWindow(hwnd, GW_OWNER).unwrap_or_default();
    let app_window = (ex_style as u32) & WS_EX_APPWINDOW.0 != 0;
    if !owner.0.is_null() && !app_window {
        return TRUE;
    }

    let mut buf = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut buf);
    if len == 0 {
        return TRUE;
    }
    let title = String::from_utf16_lossy(&buf[..len as usize]);

    let exe_path = match exe_for_hwnd(hwnd) {
        Ok(p) => p,
        Err(_) => return TRUE,
    };

    // Skip our own windows by exe name match
    if exe_path.to_lowercase().contains("glassbar") {
        return TRUE;
    }

    results.push(WindowInfo { hwnd: hwnd.0 as isize, title, exe_path });
    TRUE
}

fn exe_for_hwnd(hwnd: HWND) -> Result<String> {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return Err(anyhow!("no pid"));
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(handle, PROCESS_NAME_FORMAT(0), windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        res?;
        Ok(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn focus(hwnd: isize) -> Result<()> {
    unsafe {
        let h = HWND(hwnd as *mut _);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let _ = SetForegroundWindow(h);
    }
    Ok(())
}

/// SetForegroundWindow with the AttachThreadInput trick — modern Windows
/// blocks raw SetForegroundWindow calls from a process that doesn't
/// already own the foreground (the "Set Foreground Window restrictions"
/// in the docs). Attaching our thread's input queue to the target's
/// thread bypasses that restriction. Used by the clipboard paste flow:
/// our panel just hid, focus needs to land on the user's working window
/// before SendInput Ctrl+V fires.
pub fn focus_aggressive(hwnd: isize) -> Result<()> {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};

    if hwnd == 0 { return Ok(()); }
    unsafe {
        let h = HWND(hwnd as *mut _);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let mut pid: u32 = 0;
        let target_thread = GetWindowThreadProcessId(h, Some(&mut pid));
        let our_thread = GetCurrentThreadId();
        if target_thread == 0 || target_thread == our_thread {
            let _ = SetForegroundWindow(h);
            return Ok(());
        }
        let _ = AttachThreadInput(our_thread, target_thread, true);
        let _ = SetForegroundWindow(h);
        // Detach immediately — leaving threads attached entangles their
        // input state and breaks normal focus handling for the rest of
        // the session.
        let _ = AttachThreadInput(our_thread, target_thread, false);
    }
    Ok(())
}

/// Force `hwnd` to the foreground from a background thread that does NOT own
/// the foreground (the switcher overlay is non-activating, so glassbar never
/// holds it). Win11 blocks a bare SetForegroundWindow in that case; the
/// documented workaround is to attach our input queue to the CURRENT foreground
/// thread, borrowing its right to set the foreground, then set + raise the
/// target. Differs from focus_aggressive (which attaches to the TARGET thread
/// and only works when glassbar already owns the foreground, e.g. clipboard).
pub fn force_foreground(hwnd: isize) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::BringWindowToTop;
    if hwnd == 0 {
        return;
    }
    unsafe {
        let target = HWND(hwnd as *mut _);
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }
        let fg = GetForegroundWindow();
        let our_thread = GetCurrentThreadId();
        let mut fg_pid = 0u32;
        let fg_thread = if fg.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(fg, Some(&mut fg_pid))
        };
        if fg_thread != 0 && fg_thread != our_thread {
            let _ = AttachThreadInput(our_thread, fg_thread, true);
            let _ = BringWindowToTop(target);
            let _ = SetForegroundWindow(target);
            let _ = AttachThreadInput(our_thread, fg_thread, false);
        } else {
            let _ = BringWindowToTop(target);
            let _ = SetForegroundWindow(target);
        }
    }
}

pub fn minimize(hwnd: isize) -> Result<()> {
    unsafe {
        let _ = ShowWindow(HWND(hwnd as *mut _), SW_MINIMIZE);
    }
    Ok(())
}

pub fn close(hwnd: isize) -> Result<()> {
    unsafe {
        PostMessageW(HWND(hwnd as *mut _), WM_CLOSE, windows::Win32::Foundation::WPARAM(0), LPARAM(0))?;
    }
    Ok(())
}

/// Force-kill the process owning `hwnd` (Task-Manager → End Task semantics).
/// No prompt, no graceful shutdown — used by the dock's "Close all" so the
/// behaviour matches what the user expects from End Task.
pub fn terminate_process_of(hwnd: isize) -> Result<()> {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid));
        if pid == 0 { return Err(anyhow!("no pid for hwnd")); }
        let handle = OpenProcess(PROCESS_TERMINATE, false, pid)?;
        let res = TerminateProcess(handle, 1);
        let _ = CloseHandle(handle);
        res?;
    }
    Ok(())
}

/// Resolve the OS process ID that owns `hwnd`. Returns 0 if it can't be
/// determined (caller can use that as a sentinel).
pub fn pid_of(hwnd: isize) -> u32 {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid));
        pid
    }
}

/// Look up the full executable path for a given PID. Used by close_hwnds to
/// figure out which image name to taskkill so Squirrel-style helper
/// processes (Discord/Slack GPU + network helpers) get cleaned up alongside
/// the windowed parent.
pub fn exe_of_pid(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf: [u16; 1024] = [0; 1024];
        let mut size: u32 = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if res.is_err() { return None; }
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

pub fn foreground_hwnd() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// True if the window is minimized.
pub fn is_iconic(hwnd: isize) -> bool {
    unsafe { IsIconic(HWND(hwnd as *mut _)).as_bool() }
}

/// True if either Alt key is physically down right now. The switcher watchdog
/// uses this to recover from a missed Alt-up (e.g. an elevated foreground
/// window swallowed the key-up before our non-elevated hook could see it).
pub fn is_alt_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU};
    unsafe { (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0 }
}

/// Full bounds (x, y, width, height) of the monitor containing `hwnd`.
/// Falls back to a 1080p origin if anything fails. Used to center the
/// switcher overlay on the monitor that holds the active window.
pub fn monitor_rect_for_hwnd(hwnd: isize) -> (i32, i32, i32, i32) {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let mon = MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            let r = mi.rcMonitor;
            (r.left, r.top, r.right - r.left, r.bottom - r.top)
        } else {
            (0, 0, 1920, 1080)
        }
    }
}

// ───────────────────── fullscreen-app detection ─────────────────────
// A taskbar must get out of the way of a real fullscreen app (F11 browser,
// borderless game, video player). Explorer's own taskbar does this with its
// "rude window" check; these helpers are glassbar's equivalent.

const WS_CAPTION_BITS: u32 = 0x00C0_0000;
const WS_MAXIMIZE_BIT: u32 = 0x0100_0000;

/// Pure fullscreen test: the window covers its whole monitor AND is neither a
/// normal captioned window nor a maximized one.
///
/// The maximized exclusion matters because glassbar releases the taskbar's
/// reserved strip, so the work area equals the monitor and every maximized
/// window covers it too. Real fullscreen modes drop the maximized state first
/// (Chromium restores before F11, games use a plain WS_POPUP).
///
/// Rects are `(left, top, right, bottom)`.
pub fn is_fullscreen_geometry(win: (i32, i32, i32, i32), mon: (i32, i32, i32, i32), style: u32) -> bool {
    let covers = win.0 <= mon.0 && win.1 <= mon.1 && win.2 >= mon.2 && win.3 >= mon.3;
    let captioned = style & WS_CAPTION_BITS == WS_CAPTION_BITS;
    let maximized = style & WS_MAXIMIZE_BIT != 0;
    covers && !captioned && !maximized
}

/// Monitor handle (as isize) of the fullscreen app in the foreground.
///   - `Some(0)`   — the foreground window is not fullscreen.
///   - `Some(mon)` — a fullscreen app owns the foreground on monitor `mon`.
///   - `None`      — the foreground is glassbar itself (spotlight, menu, ring
///                   switcher); callers keep their previous answer so opening
///                   one of our own panels over a game doesn't flip the state.
pub fn fullscreen_foreground_monitor() -> Option<isize> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowRect, GWL_STYLE};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return Some(0);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return None;
        }
        // The desktop and the shell's own surfaces cover the monitor without a
        // caption — they are not fullscreen apps.
        let mut buf = [0u16; 64];
        let len = GetClassNameW(fg, &mut buf);
        let class = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
        if matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return Some(0);
        }
        if !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() {
            return Some(0);
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        // Exclusive-mode Direct3D never shows up as a plain window rect.
        if matches!(SHQueryUserNotificationState(), Ok(s) if s == QUNS_RUNNING_D3D_FULL_SCREEN) {
            return Some(mon.0 as isize);
        }
        let mut wr = RECT::default();
        if GetWindowRect(fg, &mut wr).is_err() {
            return Some(0);
        }
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return Some(0);
        }
        let m = mi.rcMonitor;
        let style = GetWindowLongW(fg, GWL_STYLE) as u32;
        if is_fullscreen_geometry(
            (wr.left, wr.top, wr.right, wr.bottom),
            (m.left, m.top, m.right, m.bottom),
            style,
        ) {
            Some(mon.0 as isize)
        } else {
            Some(0)
        }
    }
}

/// Monitor handle (as isize) containing `hwnd`.
pub fn monitor_of_hwnd(hwnd: isize) -> isize {
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
    unsafe { MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST).0 as isize }
}

/// Monitor handle (as isize) containing the screen point.
pub fn monitor_at_point(x: i32, y: i32) -> isize {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
    unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST).0 as isize }
}

pub fn is_visible(hwnd: isize) -> bool {
    unsafe { IsWindowVisible(HWND(hwnd as *mut _)).as_bool() }
}

/// Show or hide a window without activating it or touching focus.
pub fn set_visible_noactivate(hwnd: isize, visible: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNOACTIVATE};
    unsafe {
        let _ = ShowWindow(
            HWND(hwnd as *mut _),
            if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
        );
    }
}

#[cfg(test)]
mod fullscreen_tests {
    use super::is_fullscreen_geometry;

    const MON: (i32, i32, i32, i32) = (0, 0, 1920, 1080);
    const POPUP: u32 = 0x8000_0000 | 0x1000_0000; // WS_POPUP | WS_VISIBLE
    const OVERLAPPED: u32 = 0x00CF_0000 | 0x1000_0000; // WS_OVERLAPPEDWINDOW | WS_VISIBLE

    #[test]
    fn borderless_window_covering_monitor_is_fullscreen() {
        assert!(is_fullscreen_geometry(MON, MON, POPUP));
    }

    #[test]
    fn maximized_captioned_window_is_not_fullscreen() {
        // Maximized windows overhang the monitor by the frame width.
        assert!(!is_fullscreen_geometry((-8, -8, 1928, 1088), MON, OVERLAPPED | 0x0100_0000));
    }

    #[test]
    fn maximized_borderless_window_is_not_fullscreen() {
        // Custom-titlebar apps (Steam, Spotify) maximized over the reclaimed work area.
        assert!(!is_fullscreen_geometry(MON, MON, POPUP | 0x0100_0000));
    }

    #[test]
    fn window_smaller_than_monitor_is_not_fullscreen() {
        assert!(!is_fullscreen_geometry((0, 0, 1920, 1040), MON, POPUP));
    }

    #[test]
    fn fullscreen_on_secondary_monitor_uses_that_monitors_rect() {
        let second = (1920, 0, 3840, 1080);
        assert!(is_fullscreen_geometry(second, second, POPUP));
        assert!(!is_fullscreen_geometry(second, MON, POPUP));
    }
}

/// Extension that hides the console window of a spawned child process —
/// `Command::new("powershell")` etc. flashes a black box on screen by
/// default on Windows because the child inherits a fresh console. Setting
/// CREATE_NO_WINDOW (0x08000000) suppresses it entirely.
pub trait CommandHidden {
    fn hidden(&mut self) -> &mut Self;
}

impl CommandHidden for std::process::Command {
    fn hidden(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: the child gets no console at all. Stdio handles
        // are still usable via .output() / .stdin(). The flag is harmless
        // for GUI subsystems (rundll32, explorer) — they ignore it.
        self.creation_flags(0x0800_0000)
    }
}
