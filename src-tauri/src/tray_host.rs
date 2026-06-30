//! System-tray host via `Shell_NotifyIcon` interception.
//!
//! glassbar replaces the Windows taskbar, so the apps that normally park an
//! icon in the notification area (Discord, Steam, Slack, …) lose their visible
//! home. On Windows 11 (build 26200) the notification area is pure XAML/WinUI —
//! there is no `ToolbarWindow32` to scrape any more (that was the dead v1
//! approach). Instead we *become* the notification area the same way
//! CairoShell / ManagedShell do: register a per-process window class named
//! `Shell_TrayWnd`, host a hidden top-level window, and broadcast
//! `TaskbarCreated` so every tray app re-registers its icon to us.
//!
//! ## How the interception works
//! 1. A dedicated thread registers class `Shell_TrayWnd` (per-process; this
//!    coexists with explorer's same-named class because window classes are
//!    scoped to the registering module), creates a hidden `WS_POPUP` window
//!    with `WS_EX_TOPMOST | WS_EX_TOOLWINDOW`, and `SetWindowPos(HWND_TOPMOST)`
//!    so the shell's `FindWindow("Shell_TrayWnd")` returns OUR window.
//! 2. We `SendNotifyMessage(HWND_BROADCAST, RegisterWindowMessage("TaskbarCreated"))`
//!    on startup; every tray app reacts by re-issuing `Shell_NotifyIcon(NIM_ADD)`
//!    against the current `Shell_TrayWnd` — which is now us.
//! 3. `Shell_NotifyIcon` is delivered as `WM_COPYDATA` with `dwData == 1` and a
//!    `SHELLTRAYDATA { DWORD dwUnknown; DWORD dwMessage; NOTIFYICONDATAW nid; }`
//!    payload. We parse the registrations into a live map and forward clicks
//!    back to the owning apps.
//!
//! ## Struct-offset caveat (verify, don't assume)
//! `SHELLTRAYDATA` and the 64-bit `NOTIFYICONDATAW` layout are read via
//! explicit, bounds-checked byte offsets (no blind `transmute`). The documented
//! 64-bit field order is encoded in the `NID_*` constants below;
//! `tray_diagnostics()` dumps the raw bytes + parsed fields + observed `cbData`
//! to `debug.log` so the offsets can be confirmed against the running shell.
//!
//! ## Safety
//! Nothing here panics across the FFI boundary: every `WM_COPYDATA` buffer is
//! bounds-checked before parse, every fallible Win32 call is checked, and the
//! poisoned-mutex paths recover the inner data instead of unwinding. The
//! `WndProc` does only cheap, non-blocking work (parse + `CopyIcon` + a PNG
//! encode of the supplied icon) and never messages the owning app while it is
//! blocked inside its own `Shell_NotifyIcon` call — the owner-exe icon fallback
//! (which *does* message the owner) is deferred to a background thread.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CopyIcon, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SendNotifyMessageW, SetWindowPos, TranslateMessage, UnregisterClassW,
    HICON, HWND_BROADCAST, HWND_TOPMOST, MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
    SWP_NOSIZE, WM_APP, WM_COPYDATA, WM_DESTROY, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

// ───────────────────────── Shell_NotifyIcon constants ─────────────────────
// NIM_* (dwMessage values) and NIF_* (uFlags bits) from shellapi.h.
const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETVERSION: u32 = 4;

const NIF_MESSAGE: u32 = 0x0001;
const NIF_ICON: u32 = 0x0002;
const NIF_TIP: u32 = 0x0004;

// Notify-icon callback events forwarded back to the owning app.
const NIN_SELECT: u32 = 0x0400; // WM_USER + 0 (NOTIFYICON_VERSION_4 left-select)
const WM_CONTEXTMENU: u32 = 0x007B;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_NULL: u32 = 0x0000;

/// Private message that asks the host thread to tear its window down from the
/// thread that owns it (`DestroyWindow` must run on the creating thread).
const WM_TRAYHOST_SHUTDOWN: u32 = WM_APP + 1;

// ───────────────────────── SHELLTRAYDATA byte offsets ─────────────────────
// SHELLTRAYDATA { DWORD dwUnknown; DWORD dwMessage; NOTIFYICONDATAW nid; }
//   dwMessage @ 4, nid @ 8.
// 64-bit NOTIFYICONDATAW field offsets (relative to the whole buffer, nid@8):
//   cbSize@8, hWnd@16, uID@24, uFlags@28, uCallbackMessage@32, hIcon@40,
//   szTip[128]@48, dwState@304, dwStateMask@308, szInfo[256]@312,
//   uVersion@824, szInfoTitle[64]@828, dwInfoFlags@956, guidItem@960,
//   hBalloonIcon@976  (struct size 984; observed cbData ~1484 incl. trailing).
const OFF_DWMESSAGE: usize = 4;
const NID_CBSIZE: usize = 8;
const NID_HWND: usize = 16;
const NID_UID: usize = 24;
const NID_UFLAGS: usize = 28;
const NID_UCALLBACK: usize = 32;
const NID_HICON: usize = 40;
const NID_SZTIP: usize = 48;
const NID_UVERSION: usize = 824;
const SZTIP_CHARS: usize = 128;

/// Minimum buffer to safely read the load-bearing scalar fields
/// (owner hWnd … uCallbackMessage). Everything past this is read defensively
/// via `Option`-returning helpers, so an unusually small legacy payload simply
/// yields fewer fields rather than reading out of bounds.
const MIN_TRAY_BUF: usize = NID_UCALLBACK + 4; // 36

/// ~150 ms debounce so a registration burst (the spike saw 31 messages in 8 s)
/// collapses to a single `tray:changed` emit.
const DEBOUNCE_MS: u64 = 150;

/// Neutral placeholder shown when neither the live tray HICON nor the owner
/// exe icon can be rendered. Percent-escaped SVG (`%23` = `#`).
const GENERIC_TRAY_ICON: &str = "data:image/svg+xml;utf8,\
<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'>\
<rect x='5' y='5' width='22' height='22' rx='6' fill='%233a4254'/>\
<circle cx='16' cy='16' r='5' fill='%23aeb6c7'/>\
</svg>";

// ───────────────────────────── global state ───────────────────────────────
static HOST_HWND: AtomicIsize = AtomicIsize::new(0);
static HOST_OK: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_DONE: AtomicBool = AtomicBool::new(false);
static CHANGE_SEQ: AtomicU64 = AtomicU64::new(0);
static LAST_CHANGE_MS: AtomicU64 = AtomicU64::new(0);
static LAST_CBDATA: AtomicU32 = AtomicU32::new(0);
static STATE: OnceLock<Mutex<Vec<TrayEntry>>> = OnceLock::new();
static APP: OnceLock<AppHandle> = OnceLock::new();
static EPOCH: OnceLock<Instant> = OnceLock::new();
static LAST_RAW: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();

fn state() -> &'static Mutex<Vec<TrayEntry>> {
    STATE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Lock the map, recovering the inner data on poisoning so a panic on one
/// thread can never cascade into a panic here (and thus across the FFI line).
fn st() -> MutexGuard<'static, Vec<TrayEntry>> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    EPOCH
        .get()
        .map(|e| e.elapsed().as_millis() as u64)
        .unwrap_or(0)
}

fn mark_changed() {
    CHANGE_SEQ.fetch_add(1, Ordering::SeqCst);
    LAST_CHANGE_MS.store(now_ms(), Ordering::SeqCst);
}

// ───────────────────────────── serialised model ───────────────────────────

/// One mirrored tray icon, serialised to the HUD frontend. Field names are
/// load-bearing — the v1 frontend (`ui/hud/app.js`) reads `owner_hwnd`,
/// `callback_msg`, etc. verbatim, so this shape must not change.
#[derive(Serialize, Clone)]
pub struct TrayIcon {
    /// Stable key for diffing/re-render: `"{owner_hwnd}:{uid}"`.
    pub id: String,
    /// HWND that owns the notify icon (target of forwarded clicks).
    pub owner_hwnd: isize,
    /// NOTIFYICONDATA uID.
    pub uid: u32,
    /// uCallbackMessage the owner registered for this icon.
    pub callback_msg: u32,
    /// NOTIFYICON version (4 = v4 packing, 3 = legacy, 0 = unknown).
    pub version: u32,
    /// Hover tooltip text.
    pub tooltip: String,
    /// Owner exe path (fallback icon + label).
    pub exe_path: String,
    /// Owner process id.
    pub pid: u32,
    /// `data:image/...` URL for the icon (never empty — falls back to a glyph).
    pub icon: String,
}

/// Internal live entry. Kept in a `Vec` (not a `HashMap`) so the HUD grid order
/// stays stable across modifies; the tray count is tiny so linear lookup is
/// fine.
#[derive(Clone)]
struct TrayEntry {
    owner_hwnd: isize,
    uid: u32,
    callback_msg: u32,
    version: u32,
    tip: String,
    exe_path: String,
    pid: u32,
    /// Rendered `data:` URL (empty until the icon pipeline fills it).
    icon_url: String,
    /// True when the supplied HICON was missing/unrenderable and we still owe
    /// an owner-exe icon render (deferred off the WndProc thread).
    needs_exe_icon: bool,
}

impl TrayEntry {
    fn matches(&self, owner: isize, uid: u32) -> bool {
        self.owner_hwnd == owner && self.uid == uid
    }

    fn to_icon(&self) -> TrayIcon {
        TrayIcon {
            id: make_id(self.owner_hwnd, self.uid),
            owner_hwnd: self.owner_hwnd,
            uid: self.uid,
            callback_msg: self.callback_msg,
            version: self.version,
            tooltip: self.tip.clone(),
            exe_path: self.exe_path.clone(),
            pid: self.pid,
            icon: if self.icon_url.is_empty() {
                GENERIC_TRAY_ICON.to_string()
            } else {
                self.icon_url.clone()
            },
        }
    }
}

fn make_id(owner: isize, uid: u32) -> String {
    format!("{owner}:{uid}")
}

// ───────────────────────────── byte readers ───────────────────────────────
// All bounds-checked: out-of-range reads yield None / "" rather than UB.

fn read_u32(buf: &[u8], off: usize) -> Option<u32> {
    buf.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_u64(buf: &[u8], off: usize) -> Option<u64> {
    buf.get(off..off + 8).map(|s| {
        u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
    })
}

/// Read a NUL-terminated UTF-16 string of at most `max_chars` from `off`.
fn read_wstr(buf: &[u8], off: usize, max_chars: usize) -> String {
    let mut out: Vec<u16> = Vec::new();
    let mut i = off;
    for _ in 0..max_chars {
        match buf.get(i..i + 2) {
            Some(s) => {
                let c = u16::from_le_bytes([s[0], s[1]]);
                if c == 0 {
                    break;
                }
                out.push(c);
                i += 2;
            }
            None => break,
        }
    }
    String::from_utf16_lossy(&out)
}

// ───────────────────────── click/menu encoders (pure) ─────────────────────

fn loword_hiword(lo: u16, hi: u16) -> u32 {
    (lo as u32) | ((hi as u32) << 16)
}

/// (wParam, lParam) messages to POST for a left-click "activate", per the
/// stored notify-icon version. v4 and legacy encodings are disjoint, so a
/// well-behaved app only reacts to the format it registered; unknown (0) sends
/// both, maximising "the click does something" without double-activation risk.
fn activate_messages(version: u32, x: i32, y: i32, uid: u32) -> Vec<(usize, u32)> {
    let mut msgs = Vec::new();
    if version == 4 || version == 0 {
        let w = loword_hiword(x as u16, y as u16) as usize;
        let l = loword_hiword(NIN_SELECT as u16, uid as u16);
        msgs.push((w, l));
    }
    if version != 4 {
        msgs.push((uid as usize, WM_LBUTTONDOWN));
        msgs.push((uid as usize, WM_LBUTTONUP));
    }
    msgs
}

/// (wParam, lParam) messages to POST for a right-click "open the app's own
/// menu", per the stored notify-icon version.
fn menu_messages(version: u32, x: i32, y: i32, uid: u32) -> Vec<(usize, u32)> {
    let mut msgs = Vec::new();
    if version == 4 || version == 0 {
        let w = loword_hiword(x as u16, y as u16) as usize;
        let l = loword_hiword(WM_CONTEXTMENU as u16, uid as u16);
        msgs.push((w, l));
    }
    if version != 4 {
        msgs.push((uid as usize, WM_RBUTTONDOWN));
        msgs.push((uid as usize, WM_RBUTTONUP));
    }
    msgs
}

// ───────────────────────────── icon pipeline ──────────────────────────────

/// Render the app-supplied HICON to a PNG data URL. `CopyIcon` duplicates the
/// foreign handle into our process (USER handles are valid session-wide), we
/// rasterise the copy, then destroy it. Sends NO message to the owner, so this
/// is safe to call from the WndProc while the owner is blocked in its
/// `Shell_NotifyIcon` call.
fn render_primary(hicon_val: isize) -> Option<String> {
    if hicon_val == 0 {
        return None;
    }
    unsafe {
        let copy = CopyIcon(HICON(hicon_val as *mut c_void)).ok()?;
        let url = crate::icons::icon_handle_to_data_url(copy).ok();
        let _ = DestroyIcon(copy);
        url
    }
}

/// Background pass that upgrades entries still lacking an icon to the owner-exe
/// icon (or the generic glyph). Runs only off the WndProc thread (emitter /
/// command threads) where the owner is NOT blocked, so the WM_GETICON probe
/// inside `get_icon_data_url` can't stall the message pump.
fn resolve_fallback_icons() {
    // 1. Snapshot the work list under a short lock.
    let work: Vec<(isize, u32, String)> = {
        let g = st();
        g.iter()
            .filter(|e| e.needs_exe_icon)
            .map(|e| (e.owner_hwnd, e.uid, e.exe_path.clone()))
            .collect()
    };
    if work.is_empty() {
        return;
    }
    // 2. Render outside the lock.
    for (owner, uid, exe) in work {
        let url = if !exe.is_empty() {
            crate::icons::get_icon_data_url(&exe, Some(owner))
                .unwrap_or_else(|_| GENERIC_TRAY_ICON.to_string())
        } else {
            GENERIC_TRAY_ICON.to_string()
        };
        // 3. Write back if the entry is still present and still wants it.
        let mut g = st();
        if let Some(e) = g.iter_mut().find(|e| e.matches(owner, uid)) {
            if e.needs_exe_icon {
                e.icon_url = url;
                e.needs_exe_icon = false;
            }
        }
    }
}

// ───────────────────────── WM_COPYDATA handling ───────────────────────────

unsafe fn handle_tray_copydata(buf: &[u8]) {
    LAST_CBDATA.store(buf.len() as u32, Ordering::SeqCst);
    store_last_raw(buf);

    let Some(msg) = read_u32(buf, OFF_DWMESSAGE) else {
        return;
    };
    let owner = match read_u64(buf, NID_HWND) {
        Some(v) => v as isize,
        None => return,
    };
    if owner == 0 {
        return;
    }
    let uid = read_u32(buf, NID_UID).unwrap_or(0);
    let flags = read_u32(buf, NID_UFLAGS).unwrap_or(0);

    match msg {
        NIM_ADD | NIM_MODIFY => upsert(owner, uid, flags, buf),
        NIM_DELETE => remove_entry(owner, uid),
        NIM_SETVERSION => {
            let ver = read_u32(buf, NID_UVERSION).unwrap_or(0);
            set_version(owner, uid, ver);
        }
        _ => {} // NIM_SETFOCUS and anything else: ignore.
    }
}

fn upsert(owner: isize, uid: u32, flags: u32, buf: &[u8]) {
    // Parse only the fields this message flags as valid (uFlags is per-message,
    // not cumulative — absence of NIF_ICON in a MODIFY does NOT mean "no icon").
    let cb = if flags & NIF_MESSAGE != 0 {
        read_u32(buf, NID_UCALLBACK)
    } else {
        None
    };
    let tip = if flags & NIF_TIP != 0 {
        Some(read_wstr(buf, NID_SZTIP, SZTIP_CHARS))
    } else {
        None
    };
    // Pre-render the supplied icon off-lock (cheap, no owner messaging).
    // None  => this message carried no icon update.
    // Some((url, needs_fallback)) => icon present (url possibly empty if the
    //          handle was missing/unrenderable, in which case we owe a fallback).
    let new_icon: Option<(String, bool)> = if flags & NIF_ICON != 0 {
        let hicon = read_u64(buf, NID_HICON).map(|v| v as isize).unwrap_or(0);
        match render_primary(hicon) {
            Some(url) => Some((url, false)),
            None => Some((String::new(), true)),
        }
    } else {
        None
    };

    // Resolve owner exe/pid only for a brand-new entry. pid_of / exe_of_pid are
    // fast and never message the owner, so doing them here is deadlock-free.
    let exists = {
        let g = st();
        g.iter().any(|e| e.matches(owner, uid))
    };
    let (pid, exe) = if exists {
        (0u32, String::new())
    } else {
        let pid = crate::win32::pid_of(owner);
        (pid, crate::win32::exe_of_pid(pid).unwrap_or_default())
    };

    {
        let mut g = st();
        if let Some(e) = g.iter_mut().find(|e| e.matches(owner, uid)) {
            if let Some(c) = cb {
                e.callback_msg = c;
            }
            if let Some(t) = tip {
                e.tip = t;
            }
            if let Some((url, needs)) = new_icon {
                if !url.is_empty() {
                    e.icon_url = url;
                    e.needs_exe_icon = false;
                } else {
                    e.needs_exe_icon = needs;
                }
            }
        } else {
            let (icon_url, needs_exe_icon) = match new_icon {
                Some((url, _)) if !url.is_empty() => (url, false),
                Some((_, needs)) => (String::new(), needs),
                None => (String::new(), true), // ADD without NIF_ICON: try exe icon
            };
            g.push(TrayEntry {
                owner_hwnd: owner,
                uid,
                callback_msg: cb.unwrap_or(0),
                version: 0,
                tip: tip.unwrap_or_default(),
                exe_path: exe,
                pid,
                icon_url,
                needs_exe_icon,
            });
        }
    }
    mark_changed();
}

fn remove_entry(owner: isize, uid: u32) {
    let changed = {
        let mut g = st();
        let before = g.len();
        g.retain(|e| !e.matches(owner, uid));
        g.len() != before
    };
    if changed {
        mark_changed();
    }
}

fn set_version(owner: isize, uid: u32, ver: u32) {
    let changed = {
        let mut g = st();
        if let Some(e) = g.iter_mut().find(|e| e.matches(owner, uid)) {
            e.version = ver;
            true
        } else {
            // SETVERSION before ADD (rare): ignore — version stays 0, which
            // makes activate() send both encodings (safe).
            false
        }
    };
    if changed {
        mark_changed();
    }
}

fn store_last_raw(buf: &[u8]) {
    let cap = buf.len().min(1024);
    let m = LAST_RAW.get_or_init(|| Mutex::new(Vec::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    g.clear();
    g.extend_from_slice(&buf[..cap]);
}

// ───────────────────────────── window + loop ──────────────────────────────

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            let cds = lparam.0 as *const COPYDATASTRUCT;
            if !cds.is_null() {
                let cds = &*cds;
                // dwData == 1 => Shell_NotifyIcon. dwData == 0 (APPBAR) and any
                // other value are simply acknowledged (passed through).
                if cds.dwData == 1
                    && !cds.lpData.is_null()
                    && cds.cbData as usize >= MIN_TRAY_BUF
                {
                    let len = cds.cbData as usize;
                    // Copy out so we never hold a pointer into the sender's
                    // memory beyond this call.
                    let buf = std::slice::from_raw_parts(cds.lpData as *const u8, len).to_vec();
                    handle_tray_copydata(&buf);
                }
            }
            LRESULT(1) // TRUE — tell the caller Shell_NotifyIcon succeeded.
        }
        WM_TRAYHOST_SHUTDOWN => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn broadcast_taskbar_created() {
    unsafe {
        let name = wide("TaskbarCreated");
        let msg = RegisterWindowMessageW(PCWSTR(name.as_ptr()));
        if msg != 0 {
            let _ = SendNotifyMessageW(HWND_BROADCAST, msg, WPARAM(0), LPARAM(0));
        }
    }
}

fn host_thread() {
    unsafe {
        let hmodule = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                crate::glog!("[tray_host] GetModuleHandleW failed: {e}");
                return;
            }
        };
        let hinstance: HINSTANCE = hmodule.into();

        let class_name = wide("Shell_TrayWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            crate::glog!("[tray_host] RegisterClassW(Shell_TrayWnd) failed — tray host disabled");
            return;
        }

        let win_name = wide("");
        let hwnd = match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(win_name.as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            hinstance,
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                crate::glog!("[tray_host] CreateWindowExW failed: {e}");
                let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hinstance);
                return;
            }
        };

        // Become the topmost Shell_TrayWnd so FindWindow("Shell_TrayWnd")
        // returns us, beating explorer's (the spike confirmed topmost wins).
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
        HOST_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        HOST_OK.store(true, Ordering::SeqCst);
        crate::glog!("[tray_host] hosting Shell_TrayWnd hwnd=0x{:x}", hwnd.0 as isize);

        // Make every tray app re-register to us.
        broadcast_taskbar_created();
        // Re-hide explorer's taskbar immediately — the broadcast can make the
        // real shell briefly re-show it; shell_taskbar now skips our own host.
        let _ = crate::shell_taskbar::hide_windows_taskbar();

        let mut msg = MSG::default();
        loop {
            let ret = GetMessageW(&mut msg, None, 0, 0);
            // 0 = WM_QUIT, -1 = error → exit the loop.
            if ret.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Shutdown: window already destroyed (WM_DESTROY → PostQuitMessage).
        HOST_HWND.store(0, Ordering::SeqCst);
        HOST_OK.store(false, Ordering::SeqCst);
        // Hand the icons back to explorer.
        broadcast_taskbar_created();
        let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hinstance);
        SHUTDOWN_DONE.store(true, Ordering::SeqCst);
        crate::glog!("[tray_host] shut down; rebroadcast TaskbarCreated for explorer");
    }
}

fn emitter_thread() {
    // last == initial seq (0) so we don't emit an empty list before the first
    // real registration arrives.
    let mut last = CHANGE_SEQ.load(Ordering::SeqCst);
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let seq = CHANGE_SEQ.load(Ordering::SeqCst);
        if seq == last {
            continue;
        }
        // Debounce: wait until changes have settled for DEBOUNCE_MS.
        if now_ms().saturating_sub(LAST_CHANGE_MS.load(Ordering::SeqCst)) < DEBOUNCE_MS {
            continue;
        }
        // Upgrade any pending exe-icon fallbacks before emitting.
        resolve_fallback_icons();
        last = seq;
        if let Some(app) = APP.get() {
            let _ = app.emit("tray:changed", snapshot());
        }
    }
}

// ───────────────────────────── public surface ─────────────────────────────

/// Spawn the tray host (Win32 message loop + Shell_TrayWnd window) and the
/// debounced emitter thread. Idempotent-ish: intended to be called exactly once
/// from `main.rs` setup (glassbar is singleton-guarded).
pub fn spawn(app: AppHandle) {
    EPOCH.get_or_init(Instant::now);
    let _ = APP.set(app);
    std::thread::spawn(host_thread);
    std::thread::spawn(emitter_thread);
}

/// HWND of our hosted `Shell_TrayWnd` (0 until the host thread creates it).
/// `shell_taskbar` uses this to avoid hiding our own window when it hides
/// explorer's taskbar.
pub fn host_hwnd() -> isize {
    HOST_HWND.load(Ordering::SeqCst)
}

/// True once the host window is up and intercepting registrations.
pub fn host_running() -> bool {
    HOST_OK.load(Ordering::SeqCst)
}

/// Current live tray icons (resolves any pending exe-icon fallbacks first so a
/// caller-driven read is complete).
pub fn snapshot() -> Vec<TrayIcon> {
    resolve_fallback_icons();
    let g = st();
    g.iter().map(|e| e.to_icon()).collect()
}

/// Look up the stored callback message + version for an icon, falling back to
/// the values the frontend passed if we have no live entry for it.
fn resolve(owner: isize, uid: u32, cb_arg: u32, ver_arg: u32) -> (u32, u32) {
    let g = st();
    if let Some(e) = g.iter().find(|e| e.matches(owner, uid)) {
        let cb = if e.callback_msg != 0 {
            e.callback_msg
        } else {
            cb_arg
        };
        (cb, e.version)
    } else {
        (cb_arg, ver_arg)
    }
}

fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

unsafe fn post(owner: HWND, msg: u32, wparam: usize, lparam: u32) -> bool {
    // MAKELPARAM semantics: pack as u32 then sign-extend into the signed LPARAM.
    PostMessageW(owner, msg, WPARAM(wparam), LPARAM(lparam as i32 as isize)).is_ok()
}

/// Forward a left-click to the owning app (activate / toggle its window). Uses
/// the per-icon `uVersion` we recorded via NIM_SETVERSION when available.
pub fn activate(owner_hwnd: isize, uid: u32, callback_msg: u32, version: u32) -> Result<()> {
    if owner_hwnd == 0 {
        return Err(anyhow!("tray_activate: null owner hwnd"));
    }
    let owner = HWND(owner_hwnd as *mut c_void);
    let (cb, ver) = resolve(owner_hwnd, uid, callback_msg, version);
    if cb == 0 {
        // No callback registered — best we can do is surface the window.
        let _ = crate::win32::focus(owner_hwnd);
        return Ok(());
    }
    let (x, y) = cursor_pos();
    let mut delivered = false;
    unsafe {
        for (w, l) in activate_messages(ver, x, y, uid) {
            delivered |= post(owner, cb, w, l);
        }
    }
    if !delivered {
        let _ = crate::win32::focus(owner_hwnd);
    }
    Ok(())
}

/// Forward a right-click so the owning app pops its OWN native context menu.
/// Applies the KB135788 foreground workaround (our HUD is `WS_EX_NOACTIVATE`,
/// so we push the owner foreground via `win32::force_foreground` first), then
/// forwards the contextmenu event and posts a benign `WM_NULL` so the menu's
/// modal loop tears down cleanly.
pub fn open_menu(owner_hwnd: isize, uid: u32, callback_msg: u32, version: u32) -> Result<()> {
    if owner_hwnd == 0 {
        return Err(anyhow!("tray_open_menu: null owner hwnd"));
    }
    let owner = HWND(owner_hwnd as *mut c_void);
    let (cb, ver) = resolve(owner_hwnd, uid, callback_msg, version);
    crate::win32::force_foreground(owner_hwnd);
    if cb == 0 {
        return Ok(());
    }
    let (x, y) = cursor_pos();
    unsafe {
        for (w, l) in menu_messages(ver, x, y, uid) {
            let _ = post(owner, cb, w, l);
        }
        let _ = PostMessageW(owner, WM_NULL, WPARAM(0), LPARAM(0));
    }
    Ok(())
}

/// Tear the host window down and hand the icons back to explorer. Called on
/// graceful exit. Best-effort + bounded wait so it works during process exit;
/// abnormal termination relies on the next glassbar launch re-broadcasting.
pub fn shutdown() {
    let h = HOST_HWND.load(Ordering::SeqCst);
    if h == 0 {
        return;
    }
    SHUTDOWN_DONE.store(false, Ordering::SeqCst);
    unsafe {
        let _ = PostMessageW(
            HWND(h as *mut c_void),
            WM_TRAYHOST_SHUTDOWN,
            WPARAM(0),
            LPARAM(0),
        );
    }
    // Give the host thread a moment to DestroyWindow + rebroadcast before the
    // process tears down underneath it.
    let start = Instant::now();
    while !SHUTDOWN_DONE.load(Ordering::SeqCst) && start.elapsed() < Duration::from_millis(400) {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Verbose dump to `debug.log`: host status, observed `cbData`, the raw
/// SHELLTRAYDATA header bytes, the fields parsed at the documented offsets, and
/// every live icon. The primary tool for confirming the struct offsets against
/// the running shell. (debug.log is local-only, so the PII it may contain —
/// tooltips / exe paths — never leaves the machine.)
pub fn diagnostics() -> String {
    crate::glog!("[tray_host] ===== tray_diagnostics begin =====");
    let running = HOST_OK.load(Ordering::SeqCst);
    let host = HOST_HWND.load(Ordering::SeqCst);
    let cbdata = LAST_CBDATA.load(Ordering::SeqCst);
    crate::glog!(
        "[tray_host] host_running={running} host_hwnd=0x{host:x} last_cbData={cbdata} (expect ~1484)"
    );

    if let Some(m) = LAST_RAW.get() {
        let raw = m.lock().unwrap_or_else(|e| e.into_inner());
        if !raw.is_empty() {
            // Header through hIcon (offsets 0..48) — all load-bearing scalars,
            // and deliberately stops before szTip so we don't dump tooltip text.
            let head = &raw[..raw.len().min(NID_SZTIP)];
            crate::glog!("[tray_host] raw[0..{}]=[{}]", head.len(), hex(head));
            crate::glog!(
                "[tray_host] parsed: dwMessage={} nid.cbSize={} hWnd=0x{:x} uID={} uFlags=0x{:x} cb=0x{:x} hIcon=0x{:x} uVersion={}",
                read_u32(&raw, OFF_DWMESSAGE).unwrap_or(0),
                read_u32(&raw, NID_CBSIZE).unwrap_or(0),
                read_u64(&raw, NID_HWND).unwrap_or(0),
                read_u32(&raw, NID_UID).unwrap_or(0),
                read_u32(&raw, NID_UFLAGS).unwrap_or(0),
                read_u32(&raw, NID_UCALLBACK).unwrap_or(0),
                read_u64(&raw, NID_HICON).unwrap_or(0),
                read_u32(&raw, NID_UVERSION).unwrap_or(0),
            );
        }
    }

    let icons = snapshot();
    for ic in &icons {
        crate::glog!(
            "[tray_host] icon id={} hwnd=0x{:x} uid={} cb=0x{:x} ver={} pid={} exe={} tooltip={:?} iconBytes={}",
            ic.id,
            ic.owner_hwnd,
            ic.uid,
            ic.callback_msg,
            ic.version,
            ic.pid,
            ic.exe_path,
            ic.tooltip,
            ic.icon.len()
        );
    }
    crate::glog!(
        "[tray_host] ===== tray_diagnostics end ({} icons) =====",
        icons.len()
    );

    if running {
        format!(
            "tray_host: {} icon(s) live; raw SHELLTRAYDATA + parsed offsets written to debug.log",
            icons.len()
        )
    } else {
        "tray host not running (Shell_TrayWnd registration failed) — see debug.log".to_string()
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 8 == 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ───────────────────────────────── tests ──────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic SHELLTRAYDATA buffer with known values at the
    /// documented 64-bit offsets. Tooltip is a non-PII placeholder.
    fn make_buf(
        msg: u32,
        hwnd: u64,
        uid: u32,
        flags: u32,
        cb: u32,
        hicon: u64,
        tip: &str,
        ver: u32,
    ) -> Vec<u8> {
        let mut b = vec![0u8; 1484];
        b[OFF_DWMESSAGE..OFF_DWMESSAGE + 4].copy_from_slice(&msg.to_le_bytes());
        b[NID_HWND..NID_HWND + 8].copy_from_slice(&hwnd.to_le_bytes());
        b[NID_UID..NID_UID + 4].copy_from_slice(&uid.to_le_bytes());
        b[NID_UFLAGS..NID_UFLAGS + 4].copy_from_slice(&flags.to_le_bytes());
        b[NID_UCALLBACK..NID_UCALLBACK + 4].copy_from_slice(&cb.to_le_bytes());
        b[NID_HICON..NID_HICON + 8].copy_from_slice(&hicon.to_le_bytes());
        for (i, c) in tip.encode_utf16().enumerate() {
            let o = NID_SZTIP + i * 2;
            b[o..o + 2].copy_from_slice(&c.to_le_bytes());
        }
        b[NID_UVERSION..NID_UVERSION + 4].copy_from_slice(&ver.to_le_bytes());
        b
    }

    #[test]
    fn parses_documented_offsets() {
        let buf = make_buf(
            NIM_ADD,
            0x0000_0001_2345_6789,
            7,
            NIF_MESSAGE | NIF_ICON | NIF_TIP,
            0x401,
            0xDEAD_BEEF,
            "TestTip",
            4,
        );
        assert_eq!(read_u32(&buf, OFF_DWMESSAGE), Some(NIM_ADD));
        assert_eq!(read_u64(&buf, NID_HWND), Some(0x0000_0001_2345_6789));
        assert_eq!(read_u32(&buf, NID_UID), Some(7));
        assert_eq!(
            read_u32(&buf, NID_UFLAGS),
            Some(NIF_MESSAGE | NIF_ICON | NIF_TIP)
        );
        assert_eq!(read_u32(&buf, NID_UCALLBACK), Some(0x401));
        assert_eq!(read_u64(&buf, NID_HICON), Some(0xDEAD_BEEF));
        assert_eq!(read_wstr(&buf, NID_SZTIP, SZTIP_CHARS), "TestTip");
        assert_eq!(read_u32(&buf, NID_UVERSION), Some(4));
    }

    #[test]
    fn readers_are_bounds_checked() {
        let small = vec![0u8; 10];
        assert_eq!(read_u32(&small, 8), None); // would need bytes 8..12
        assert_eq!(read_u64(&small, 4), None); // would need bytes 4..12
        assert_eq!(read_wstr(&small, 8, 128), ""); // truncates safely
    }

    #[test]
    fn id_formatting_is_stable() {
        assert_eq!(make_id(0x1234, 7), "4660:7");
        assert_eq!(make_id(0, 0), "0:0");
    }

    #[test]
    fn loword_hiword_packs_correctly() {
        assert_eq!(loword_hiword(0x0400, 7), 0x0007_0400);
        assert_eq!(loword_hiword(0, 0), 0);
        assert_eq!(loword_hiword(0xFFFF, 0xFFFF), 0xFFFF_FFFF);
    }

    #[test]
    fn activate_v4_sends_single_select() {
        let msgs = activate_messages(4, 100, 200, 7);
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0],
            (
                loword_hiword(100, 200) as usize,
                loword_hiword(NIN_SELECT as u16, 7)
            )
        );
    }

    #[test]
    fn activate_legacy_sends_down_up() {
        let msgs = activate_messages(3, 0, 0, 7);
        assert_eq!(
            msgs,
            vec![(7usize, WM_LBUTTONDOWN), (7usize, WM_LBUTTONUP)]
        );
    }

    #[test]
    fn activate_unknown_sends_both() {
        // version 0 (unknown) → v4 packing + legacy down/up = 3 messages.
        let msgs = activate_messages(0, 5, 6, 9);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].0, loword_hiword(5, 6) as usize);
        assert_eq!(msgs[1], (9usize, WM_LBUTTONDOWN));
        assert_eq!(msgs[2], (9usize, WM_LBUTTONUP));
    }

    #[test]
    fn menu_v4_uses_contextmenu() {
        let msgs = menu_messages(4, 10, 20, 3);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].1, loword_hiword(WM_CONTEXTMENU as u16, 3));
    }

    #[test]
    fn menu_legacy_uses_rbutton() {
        let msgs = menu_messages(3, 0, 0, 3);
        assert_eq!(
            msgs,
            vec![(3usize, WM_RBUTTONDOWN), (3usize, WM_RBUTTONUP)]
        );
    }
}
