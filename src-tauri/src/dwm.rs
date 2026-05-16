use windows::Win32::Foundation::{BOOL, COLORREF, HWND};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWINDOWATTRIBUTE,
    DWMWA_NCRENDERING_POLICY, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMNCRENDERINGPOLICY, DWMWCP_DONOTROUND,
    DWM_SYSTEMBACKDROP_TYPE,
};

/// Win11-only: the magic value that tells DWM to suppress the focus-border
/// accent stroke entirely. The native enum constant isn't exported by the
/// version of `windows` we use, so we pass the raw u32.
const DWMWA_BORDER_COLOR: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(34);
const DWMWA_COLOR_NONE: u32 = 0xFFFFFFFE;
use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
    GWL_EXSTYLE, GWL_STYLE, HWND_TOP, HWND_TOPMOST, LWA_ALPHA, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_BORDER, WS_CAPTION,
    WS_DLGFRAME, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
    WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};

pub const BACKDROP_MICA: i32 = 2;
pub const BACKDROP_ACRYLIC: i32 = 3; // DWMSBT_TRANSIENTWINDOW — true see-through glass on Win11

/// Tell DWM **not** to round our corners — we own the rounding via
/// `apply_rounded_region` at the exact 22px CSS radius. Without this, DWM
/// applies its own ~8px round to the acrylic backdrop *underneath* our
/// 22px region clip, producing two visible curves at every corner: a
/// tight inner one from DWM's small round, a wider outer one from the
/// region. Setting DONOTROUND collapses the two into a single edge.
pub fn round_corners(hwnd: isize) {
    let pref = DWMWCP_DONOTROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

/// Hard-clip the window to a rounded-rectangle region so the layered-alpha
/// surface no longer paints in the corners. Without this the WS_EX_LAYERED
/// window stays a perfect rectangle even with rounded CSS, leaving four
/// faintly-tinted square corners around the glass pill.
///
/// `width` and `height` are physical pixels. Call AFTER the window is built
/// and re-call if you ever resize it (we don't, so once is enough).
pub fn apply_rounded_region(hwnd: isize, width: i32, height: i32, radius: i32) {
    unsafe {
        // GDI region right/bottom are exclusive — passing (width, height) gives
        // a region that covers exactly `width × height` pixels. The previous
        // `width + 1` extended the right/bottom-corner curves 1px past the
        // window edge, making those corners visibly flatter than the
        // (correctly-sized) left ones.
        let rgn = CreateRoundRectRgn(0, 0, width, height, radius * 2, radius * 2);
        if !rgn.0.is_null() {
            // SetWindowRgn takes ownership of the region — don't free it.
            let _ = SetWindowRgn(HWND(hwnd as *mut _), rgn, windows::Win32::Foundation::TRUE);
        }
    }
}

/// Add WS_EX_LAYERED and apply uniform alpha to the whole window. Used as
/// a last-resort glass effect when Tauri's transparent(true) doesn't make
/// the host window per-pixel-alpha capable. The whole window — including
/// content — will be alpha-blended with whatever's behind it on screen.
pub fn make_layered_with_alpha(hwnd: isize, alpha: u8) {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let style = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA);
    }
}

/// Re-assert HWND_TOPMOST without moving/resizing/activating. Fullscreen
/// (especially borderless-windowed) apps occasionally clobber z-order — a
/// cheap periodic re-assert keeps the dock above F11 / game windows.
pub fn assert_topmost(hwnd: isize) {
    unsafe {
        let _ = SetWindowPos(
            HWND(hwnd as *mut _),
            HWND_TOPMOST,
            0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Move the window AND keep it pinned at HWND_TOPMOST in a single
/// SetWindowPos call. Tauri's `WebviewWindow::set_position` calls
/// SetWindowPos with HWND_TOP (regular top, not topmost) which silently
/// drops us out of the topmost band — that's why other apps could peek
/// through the dock during the slide-in animation. Always use this
/// helper from animation paths so each frame re-asserts topmost too.
pub fn set_position_topmost(hwnd: isize, x: i32, y: i32) {
    unsafe {
        let _ = SetWindowPos(
            HWND(hwnd as *mut _),
            HWND_TOPMOST,
            x, y, 0, 0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Strip WS_CAPTION / WS_SYSMENU / WS_MINIMIZEBOX / WS_MAXIMIZEBOX /
/// WS_THICKFRAME / WS_BORDER from the window's GWL_STYLE. Tauri's
/// `.decorations(false)` should already do this, but on some Win11 builds the
/// title bar still gets drawn after we change region/transparency — this is
/// the belt-and-braces removal of the OS-drawn frame.
pub fn strip_decorations(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let strip: u32 = WS_CAPTION.0
            | WS_SYSMENU.0
            | WS_MINIMIZEBOX.0
            | WS_MAXIMIZEBOX.0
            | WS_THICKFRAME.0
            | WS_BORDER.0
            | WS_DLGFRAME.0;
        let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
        // Add WS_POPUP — without it, a top-level window defaults to WS_OVERLAPPED
        // which implies a caption no matter what other bits we clear.
        let new_style = (style & !strip) | WS_POPUP.0;
        // Bail when nothing changed — SetWindowPos+SWP_FRAMECHANGED triggers a
        // non-client redraw that flickers the corners, so we only want to pay
        // that cost when WS_CAPTION has actually crept back in.
        if new_style == style {
            return;
        }
        SetWindowLongPtrW(h, GWL_STYLE, new_style as isize);
        let _ = SetWindowPos(
            h,
            HWND_TOP,
            0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Add WS_EX_NOACTIVATE so clicking the dock doesn't yank focus away from
/// whatever the user was working with (game, IDE, etc).
pub fn make_noactivate(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let style = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as isize);
    }
}

/// Tell DWM to skip drawing the non-client area entirely. Belt-and-braces
/// alongside `strip_decorations` — even if WS_CAPTION sneaks back in (Tauri's
/// internal SetWindowPos calls re-add it on some Win11 builds), DWM won't
/// render a title bar. Also forces dark-mode caption colors so any momentary
/// flash before this attribute applies appears dark rather than white.
pub fn suppress_nc_rendering(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let policy = DWMNCRP_DISABLED;
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_NCRENDERING_POLICY,
            &policy as *const DWMNCRENDERINGPOLICY as *const _,
            std::mem::size_of::<DWMNCRENDERINGPOLICY>() as u32,
        );
        let dark: BOOL = BOOL(1);
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );
        // Kill the Win11 focus-border accent — that's the white stroke the
        // user sees flash when the window receives a click. COLOR_NONE is
        // the documented sentinel that tells DWM "draw no border at all".
        // No-op on Win10 (silently ignored).
        let none = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_BORDER_COLOR,
            &none as *const u32 as *const _,
            std::mem::size_of::<u32>() as u32,
        );
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
