//! Custom WindowProc that swallows non-client paint + activation messages.
//! The DWMWA_BORDER_COLOR = COLOR_NONE attribute is supposed to kill the
//! Win11 focus-border accent, but on some 24H2 builds DWM still paints a
//! white stroke when the host receives an activation. Subclassing the
//! window with a WndProc that returns 1 from WM_NCACTIVATE and 0 from
//! WM_NCPAINT is the canonical borderless-app trick — Win32 then never
//! gets the chance to redraw NC area at all.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, SetWindowLongPtrW,
    GWLP_WNDPROC, WNDPROC, WM_NCACTIVATE, WM_NCPAINT,
};

/// Stores the original WindowProc (as isize so it can live in a HashMap)
/// per HWND so a single hwnd can be subclassed exactly once and the chain
/// stays intact even if other code subclasses on top later.
fn originals() -> &'static Mutex<HashMap<isize, isize>> {
    static M: OnceLock<Mutex<HashMap<isize, isize>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe extern "system" fn nc_silencer(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        // No NC paint at all — the white bar / focus border is rendered here.
        WM_NCPAINT => return LRESULT(0),
        // Returning TRUE claims activation handled, keeping DWM from painting
        // the focus accent. Skipping default also suppresses the brief white
        // flash on click.
        WM_NCACTIVATE => return LRESULT(1),
        _ => {}
    }

    // Forward to whatever WindowProc was installed before we got here. Look
    // up by hwnd so multiple subclassed windows don't fight over a single
    // saved pointer.
    let orig_isize = {
        let map = originals().lock().unwrap();
        map.get(&(hwnd.0 as isize)).copied()
    };
    if let Some(orig) = orig_isize {
        let orig_fn: WNDPROC = std::mem::transmute(orig);
        return CallWindowProcW(orig_fn, hwnd, msg, w, l);
    }
    DefWindowProcW(hwnd, msg, w, l)
}

/// Subclass `hwnd` so its NC paints / activates are silenced. Idempotent —
/// calling twice on the same hwnd is harmless (the second call replaces
/// our own WndProc with itself and the original pointer is preserved).
pub fn silence_nc(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let new_proc = nc_silencer as *const () as isize;
        let orig = SetWindowLongPtrW(h, GWLP_WNDPROC, new_proc);
        if orig != 0 && orig != new_proc {
            originals().lock().unwrap().insert(hwnd, orig);
        }
    }
}
