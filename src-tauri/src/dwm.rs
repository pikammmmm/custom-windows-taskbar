use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWM_SYSTEMBACKDROP_TYPE,
};
use windows::Win32::UI::Controls::MARGINS;

pub const BACKDROP_AUTO: i32 = 0;
pub const BACKDROP_NONE: i32 = 1;
pub const BACKDROP_MICA: i32 = 2;
pub const BACKDROP_ACRYLIC: i32 = 3; // DWMSBT_TRANSIENTWINDOW — true see-through glass on Win11
pub const BACKDROP_TABBED: i32 = 4;

pub fn round_corners(hwnd: isize) {
    let pref = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

/// Extend the DWM frame into the entire client area so the backdrop has
/// somewhere to render. Required for DWMWA_SYSTEMBACKDROP_TYPE to take
/// visible effect on a borderless transparent window.
pub fn extend_frame_into_client(hwnd: isize) {
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(HWND(hwnd as *mut _), &margins);
    }
}

/// Set the Win11 system backdrop type. Returns true on success. Requires
/// Windows 11 build 22000 or newer; older builds will return false and the
/// caller should fall back to legacy window-vibrancy effects.
pub fn set_backdrop(hwnd: isize, kind: i32) -> bool {
    let value = DWM_SYSTEMBACKDROP_TYPE(kind);
    unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_SYSTEMBACKDROP_TYPE,
            &value as *const _ as *const _,
            std::mem::size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        )
        .is_ok()
    }
}
