use anyhow::{Result, anyhow};
use windows::Win32::Foundation::{HWND, BOOL, LPARAM, TRUE, CloseHandle};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, IsWindowVisible, GetWindowLongW, GWL_EXSTYLE,
    WS_EX_TOOLWINDOW, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow,
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

pub fn foreground_hwnd() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
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
