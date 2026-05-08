//! Active keyboard-layout probe + layout switcher.
//!
//! Windows tracks keyboard layouts per-thread, so the "current layout"
//! is whatever the foreground window's GUI thread is currently using.
//! We resolve that on every snapshot tick — fast enough that pressing
//! Win+Space (the OS layout switcher) reflects in the HUD chip within
//! ~one tick.
//!
//! Switching layouts is done by posting WM_INPUTLANGCHANGEREQUEST to the
//! foreground window with the desired HKL — the OS routes that through
//! the Text Services Framework just like the Win+Space hotkey would.

use serde::Serialize;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Globalization::{GetLocaleInfoW, LOCALE_SABBREVLANGNAME, LOCALE_SLOCALIZEDDISPLAYNAME};
// LCTYPE is just a u32 alias in windows-rs 0.58 — the constants above
// have type u32 directly, so we type our wrapper's parameter the same.
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ActivateKeyboardLayout, GetKeyboardLayout, GetKeyboardLayoutList, HKL, KLF_SETFORPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, WM_INPUTLANGCHANGEREQUEST,
};

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct LayoutInfo {
    /// HKL encoded as a u32, sent back through Tauri commands so the
    /// frontend doesn't need to deal with raw pointers.
    pub hkl: u32,
    /// 2-3 letter abbreviated language code (ENG, DEU, FRA…).
    pub code: String,
    /// Human-readable language name ("English (United States)").
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct KeyboardState {
    pub current: Option<LayoutInfo>,
    /// Every installed layout, in the OS's listing order.
    pub installed: Vec<LayoutInfo>,
}

/// Pull the layout currently active in the foreground window's GUI
/// thread. Falls back to the calling thread's layout if there's no
/// foreground window (briefly the case during a window switch).
pub fn current() -> KeyboardState {
    unsafe {
        let hkl = active_hkl();
        let current = hkl.map(|h| layout_info(h));
        let installed = list_installed();
        KeyboardState { current, installed }
    }
}

unsafe fn active_hkl() -> Option<HKL> {
    let hwnd = GetForegroundWindow();
    if hwnd.0.is_null() {
        return Some(GetKeyboardLayout(0));
    }
    let mut pid: u32 = 0;
    let tid = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if tid == 0 {
        return Some(GetKeyboardLayout(0));
    }
    Some(GetKeyboardLayout(tid))
}

unsafe fn list_installed() -> Vec<LayoutInfo> {
    // First call with size=0 returns the count, then a sized buffer.
    let count = GetKeyboardLayoutList(None);
    if count <= 0 {
        return Vec::new();
    }
    let mut buf = vec![HKL(std::ptr::null_mut()); count as usize];
    let written = GetKeyboardLayoutList(Some(&mut buf));
    if written <= 0 {
        return Vec::new();
    }
    buf.truncate(written as usize);
    buf.into_iter().map(|h| layout_info(h)).collect()
}

unsafe fn layout_info(hkl: HKL) -> LayoutInfo {
    let raw = hkl.0 as usize;
    // Low word holds the input language identifier (locale ID). The high
    // word identifies the physical-layout DLL — irrelevant for naming.
    let lang_id = (raw & 0xFFFF) as u32;
    LayoutInfo {
        hkl: raw as u32,
        code: locale_string(lang_id, LOCALE_SABBREVLANGNAME).unwrap_or_else(|| "??".into()),
        name: locale_string(lang_id, LOCALE_SLOCALIZEDDISPLAYNAME)
            .unwrap_or_else(|| format!("0x{:04X}", lang_id)),
    }
}

unsafe fn locale_string(lcid: u32, lc_type: u32) -> Option<String> {
    // First call to size the buffer, second to fill it.
    let needed = GetLocaleInfoW(lcid, lc_type, None);
    if needed <= 0 {
        return None;
    }
    let mut buf = vec![0u16; needed as usize];
    let written = GetLocaleInfoW(lcid, lc_type, Some(&mut buf));
    if written <= 0 {
        return None;
    }
    let len = (written as usize).saturating_sub(1); // drop trailing NUL
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Switch the active keyboard layout to `hkl`. Posts the same message
/// the OS posts when the user hits Win+Space, so apps that listen for
/// language changes (Office, browsers) update their input handling
/// just like they would for the OS hotkey.
pub fn activate(hkl_raw: u32) -> Result<(), String> {
    unsafe {
        let hkl = HKL(hkl_raw as *mut _);
        // ActivateKeyboardLayout sets it for our process; the
        // WM_INPUTLANGCHANGEREQUEST we post next reaches the foreground
        // app's thread and asks it to switch too. Most apps honour the
        // request; the few that don't will still pick up our process-
        // wide change on next focus.
        let _ = ActivateKeyboardLayout(hkl, KLF_SETFORPROCESS);
        let fg = GetForegroundWindow();
        if !fg.0.is_null() {
            // wParam = 0 (hardware-independent), lParam = HKL.
            let _ = PostMessageW(
                fg,
                WM_INPUTLANGCHANGEREQUEST,
                WPARAM(0),
                LPARAM(hkl_raw as isize),
            );
        }
    }
    Ok(())
}
