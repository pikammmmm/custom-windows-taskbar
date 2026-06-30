//! Cross-process notification-area (system tray) mirror.
//!
//! glassbar replaces the Windows taskbar, so the apps that normally park an
//! icon in the notification area (Discord, Steam, Slack, …) lose their visible
//! home. This module reads the *visible* tray icons straight out of explorer's
//! tray toolbar and re-surfaces them in the HUD, plus forwards clicks back to
//! the owning apps so they behave exactly as if the user clicked the real tray.
//!
//! ## How the read works
//! The shell stores its visible tray buttons in a `ToolbarWindow32` living at
//! `Shell_TrayWnd → TrayNotifyWnd → SysPager → ToolbarWindow32` (all inside
//! explorer.exe). Each toolbar button's `dwData` points at an (undocumented)
//! `TRAYDATA` struct that carries the owner HWND, the notify-icon uID, the
//! callback message and the `HICON`. We:
//!   1. `OpenProcess` explorer with VM_READ | VM_OPERATION | QUERY_INFORMATION,
//!   2. `VirtualAllocEx` a small scratch buffer *inside explorer*,
//!   3. ask the toolbar (`TB_GETBUTTON` / `TB_GETBUTTONTEXTW`) to fill that
//!      buffer, then `ReadProcessMemory` it back into our address space.
//!
//! We only ever WRITE to our own scratch allocation; every byte we read out of
//! explorer is read-only. The scratch buffer and process handle are released on
//! every path. Nothing here panics — every fallible Win32 call is checked and
//! the whole thing degrades to an empty list + a log line on failure.
//!
//! ## Struct-offset caveat (verify, don't assume)
//! `TRAYDATA` is undocumented and its layout has drifted across Windows
//! versions. The offsets below match 64-bit Windows 10/11 explorer (HWND@0,
//! uID@8, uCallbackMessage@12, hIcon@24). `diagnostics()` dumps the raw bytes
//! of each `TRAYDATA` to `debug.log` so the offsets can be confirmed against
//! the running shell rather than trusted blindly.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CopyIcon, DestroyIcon, FindWindowExW, FindWindowW, GetCursorPos, GetWindowThreadProcessId,
    PostMessageW, SendMessageW, HICON,
};

// Toolbar control messages (commctrl.h). WM_USER = 0x0400.
const TB_GETBUTTON: u32 = 0x0417; // WM_USER + 23
const TB_BUTTONCOUNT: u32 = 0x0418; // WM_USER + 24
const TB_GETBUTTONTEXTW: u32 = 0x044B; // WM_USER + 75
const TBSTATE_HIDDEN_BIT: u8 = 0x08;

// Notify-icon callback events.
const NIN_SELECT: u32 = 0x0400; // WM_USER + 0 (NOTIFYICON_VERSION_4 left-select)
const WM_CONTEXTMENU: u32 = 0x007B;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_NULL: u32 = 0x0000;

// Scratch buffer carved out of explorer: a TBBUTTON (32 B on x64) at the base,
// then a UTF-16 text region for TB_GETBUTTONTEXTW.
const SCRATCH_SIZE: usize = 1024;
const TEXT_OFFSET: usize = 64;
const MAX_TEXT_CHARS: usize = (SCRATCH_SIZE - TEXT_OFFSET) / 2;

/// Neutral placeholder shown when neither the live tray HICON nor the owner
/// exe icon can be rendered. Percent-escaped SVG (`%23` = `#`), matching the
/// data-URL convention the icon overrides in `icons.rs` use.
const GENERIC_TRAY_ICON: &str = "data:image/svg+xml;utf8,\
<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'>\
<rect x='5' y='5' width='22' height='22' rx='6' fill='%233a4254'/>\
<circle cx='16' cy='16' r='5' fill='%23aeb6c7'/>\
</svg>";

/// One mirrored tray icon, serialised to the HUD frontend.
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
    /// Best-effort NOTIFYICON version (4 = v4 packing, 3 = legacy, 0 = unknown).
    pub version: u32,
    /// Hover tooltip text.
    pub tooltip: String,
    /// Owner exe path (fallback icon + label).
    pub exe_path: String,
    /// Owner process id.
    pub pid: u32,
    /// `data:image/...` URL for the icon.
    pub icon: String,
}

/// 64-bit `TBBUTTON` (commctrl.h). Declared locally so the read is independent
/// of crate struct churn; `#[repr(C)]` matches the OS layout exactly (32 B).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TbButton {
    i_bitmap: i32,
    id_command: i32,
    fs_state: u8,
    fs_style: u8,
    b_reserved: [u8; 6], // x64 padding before the pointer-sized dwData
    dw_data: usize,
    i_string: isize,
}

/// Undocumented shell `TRAYDATA` (64-bit explorer layout, 32 B). Only the
/// owner HWND, uID, callback message and HICON are load-bearing; the two
/// reserved DWORDs are kept so the trailing HICON lands at offset 24.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TrayData {
    hwnd: isize,       // off 0  : owner HWND
    uid: u32,          // off 8  : uID
    callback_msg: u32, // off 12 : uCallbackMessage
    dw_state: u32,     // off 16 : (reserved / state)
    u_version: u32,    // off 20 : (reserved / version)
    hicon: isize,      // off 24 : hIcon
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Walk to explorer's visible-tray `ToolbarWindow32`. Tolerates the `SysPager`
/// intermediate being absent on some shell variants.
unsafe fn find_tray_toolbar() -> Result<HWND> {
    let shell = FindWindowW(PCWSTR(wide("Shell_TrayWnd").as_ptr()), PCWSTR::null())
        .map_err(|e| anyhow!("Shell_TrayWnd not found: {e}"))?;
    let notify = FindWindowExW(
        shell,
        HWND::default(),
        PCWSTR(wide("TrayNotifyWnd").as_ptr()),
        PCWSTR::null(),
    )
    .map_err(|e| anyhow!("TrayNotifyWnd not found: {e}"))?;

    // Preferred path: TrayNotifyWnd → SysPager → ToolbarWindow32.
    if let Ok(pager) = FindWindowExW(
        notify,
        HWND::default(),
        PCWSTR(wide("SysPager").as_ptr()),
        PCWSTR::null(),
    ) {
        if let Ok(tb) = FindWindowExW(
            pager,
            HWND::default(),
            PCWSTR(wide("ToolbarWindow32").as_ptr()),
            PCWSTR::null(),
        ) {
            return Ok(tb);
        }
    }

    // Fallback: toolbar directly under TrayNotifyWnd.
    FindWindowExW(
        notify,
        HWND::default(),
        PCWSTR(wide("ToolbarWindow32").as_ptr()),
        PCWSTR::null(),
    )
    .map_err(|e| anyhow!("tray ToolbarWindow32 not found: {e}"))
}

/// Read the current visible tray icons. `Err` means the tray was unreadable
/// (toolbar not found / OpenProcess denied) → HUD shows "Tray unavailable".
/// `Ok(empty)` means readable but no icons → HUD shows "No tray icons".
pub fn read_tray_icons() -> Result<Vec<TrayIcon>> {
    read_tray_icons_inner(false)
}

fn read_tray_icons_inner(diag: bool) -> Result<Vec<TrayIcon>> {
    unsafe {
        let toolbar = find_tray_toolbar()?;
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(toolbar, Some(&mut pid));
        if pid == 0 {
            return Err(anyhow!("could not resolve explorer pid for tray toolbar"));
        }
        let process = OpenProcess(
            PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_QUERY_INFORMATION,
            false,
            pid,
        )
        .map_err(|e| anyhow!("OpenProcess(explorer pid {pid}) failed: {e}"))?;

        // Everything after the OpenProcess is funnelled through here so the
        // process handle is always closed, even on an early return.
        let outcome = read_with_process(toolbar, process, diag);
        let _ = CloseHandle(process);
        outcome
    }
}

unsafe fn read_with_process(toolbar: HWND, process: HANDLE, diag: bool) -> Result<Vec<TrayIcon>> {
    let count = SendMessageW(toolbar, TB_BUTTONCOUNT, WPARAM(0), LPARAM(0)).0;
    if diag {
        crate::glog!("[tray] TB_BUTTONCOUNT = {count}");
    }
    if count <= 0 {
        return Ok(Vec::new());
    }

    let scratch = VirtualAllocEx(
        process,
        None,
        SCRATCH_SIZE,
        MEM_COMMIT | MEM_RESERVE,
        PAGE_READWRITE,
    );
    if scratch.is_null() {
        return Err(anyhow!("VirtualAllocEx in explorer failed"));
    }

    // No early returns past this point — the scratch buffer must be freed.
    let mut icons = Vec::new();
    for i in 0..count as usize {
        if let Some(icon) = read_one_button(toolbar, process, scratch, i, diag) {
            icons.push(icon);
        }
    }

    let _ = VirtualFreeEx(process, scratch, 0, MEM_RELEASE);
    Ok(icons)
}

unsafe fn read_one_button(
    toolbar: HWND,
    process: HANDLE,
    scratch: *mut c_void,
    index: usize,
    diag: bool,
) -> Option<TrayIcon> {
    // 1. Have explorer write the TBBUTTON for `index` into our scratch buffer,
    //    then copy it back into our address space.
    let r = SendMessageW(
        toolbar,
        TB_GETBUTTON,
        WPARAM(index),
        LPARAM(scratch as isize),
    );
    if r.0 == 0 {
        return None;
    }
    let mut tb = TbButton::default();
    if ReadProcessMemory(
        process,
        scratch as *const c_void,
        &mut tb as *mut TbButton as *mut c_void,
        std::mem::size_of::<TbButton>(),
        None,
    )
    .is_err()
    {
        return None;
    }
    let _ = tb.i_bitmap;
    let _ = tb.fs_style;
    let _ = tb.b_reserved;
    let _ = tb.i_string;

    // Hidden / overflow icons are out of scope for now.
    if tb.fs_state & TBSTATE_HIDDEN_BIT != 0 {
        return None;
    }
    if tb.dw_data == 0 {
        return None;
    }

    // 2. dwData → TRAYDATA inside explorer.
    let mut td = TrayData::default();
    if ReadProcessMemory(
        process,
        tb.dw_data as *const c_void,
        &mut td as *mut TrayData as *mut c_void,
        std::mem::size_of::<TrayData>(),
        None,
    )
    .is_err()
    {
        return None;
    }

    if diag {
        // Raw byte dump so TRAYDATA offsets can be eyeballed against the live
        // 64-bit explorer — this is the one thing we deliberately don't assume.
        let mut raw = [0u8; 48];
        let _ = ReadProcessMemory(
            process,
            tb.dw_data as *const c_void,
            raw.as_mut_ptr() as *mut c_void,
            raw.len(),
            None,
        );
        crate::glog!(
            "[tray] #{index} idCommand={} fsState=0x{:02x} dwData=0x{:x} TRAYDATA raw=[{}]",
            tb.id_command,
            tb.fs_state,
            tb.dw_data,
            hex(&raw)
        );
        crate::glog!(
            "[tray] #{index} parsed hwnd=0x{:x} uid={} cb=0x{:x} state=0x{:x} ver={} hicon=0x{:x}",
            td.hwnd,
            td.uid,
            td.callback_msg,
            td.dw_state,
            td.u_version,
            td.hicon
        );
    }

    if td.hwnd == 0 {
        return None;
    }

    // 3. Tooltip text via TB_GETBUTTONTEXTW into the scratch text region.
    let tooltip = read_button_text(toolbar, process, scratch, tb.id_command);

    // 4. Owner exe + pid (reuses the battle-tested win32 helpers).
    let pid = crate::win32::pid_of(td.hwnd);
    let exe_path = crate::win32::exe_of_pid(pid).unwrap_or_default();

    // 5. Icon with the CopyIcon → exe-icon → generic fallback chain.
    let icon = icon_data_url(td.hicon, &exe_path, td.hwnd);

    // Best-effort version: only trust an exact v3/v4 readout, otherwise treat
    // as unknown (which makes the action path send both encodings — safe).
    let version = match td.u_version {
        3 | 4 => td.u_version,
        _ => 0,
    };

    Some(TrayIcon {
        id: format!("{}:{}", td.hwnd, td.uid),
        owner_hwnd: td.hwnd,
        uid: td.uid,
        callback_msg: td.callback_msg,
        version,
        tooltip,
        exe_path,
        pid,
        icon,
    })
}

unsafe fn read_button_text(
    toolbar: HWND,
    process: HANDLE,
    scratch: *mut c_void,
    id_command: i32,
) -> String {
    let text_ptr = (scratch as usize + TEXT_OFFSET) as *mut c_void;
    let n = SendMessageW(
        toolbar,
        TB_GETBUTTONTEXTW,
        WPARAM(id_command as usize),
        LPARAM(text_ptr as isize),
    )
    .0;
    if n <= 0 {
        return String::new();
    }
    let chars = (n as usize).min(MAX_TEXT_CHARS);
    let mut buf = vec![0u16; chars];
    if ReadProcessMemory(
        process,
        text_ptr as *const c_void,
        buf.as_mut_ptr() as *mut c_void,
        chars * 2,
        None,
    )
    .is_err()
    {
        return String::new();
    }
    String::from_utf16_lossy(&buf)
}

/// Icon fallback chain: live tray HICON → owner exe/window icon → placeholder.
fn icon_data_url(hicon_val: isize, exe_path: &str, owner_hwnd: isize) -> String {
    // 1. CopyIcon the explorer-owned HICON into our process (USER handles are
    //    valid session-wide), render to PNG, then destroy our copy.
    if hicon_val != 0 {
        unsafe {
            if let Ok(copy) = CopyIcon(HICON(hicon_val as *mut c_void)) {
                let url = crate::icons::icon_handle_to_data_url(copy).ok();
                let _ = DestroyIcon(copy);
                if let Some(u) = url {
                    return u;
                }
            }
        }
    }
    // 2. Owner exe / window icon (same path the dock uses everywhere else).
    if !exe_path.is_empty() {
        if let Ok(u) = crate::icons::get_icon_data_url(exe_path, Some(owner_hwnd)) {
            return u;
        }
    }
    // 3. Generic glyph so the grid never renders a broken image.
    GENERIC_TRAY_ICON.to_string()
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

fn loword_hiword(lo: u16, hi: u16) -> u32 {
    (lo as u32) | ((hi as u32) << 16)
}

fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

unsafe fn post(owner: HWND, msg: u32, wparam: usize, lparam: u32) -> bool {
    // MAKELPARAM semantics: pack as u32, then sign-extend to the signed LPARAM.
    PostMessageW(owner, msg, WPARAM(wparam), LPARAM(lparam as i32 as isize)).is_ok()
}

/// Forward a left-click to the owning app (activate / toggle its window).
///
/// NOTIFYICON_VERSION_4 (`NIN_SELECT`) and legacy (`WM_LBUTTONDOWN/UP`)
/// callback encodings are *disjoint*: a correct handler reads the event code
/// from lParam's low word for v4 but treats the whole lParam as the message
/// for legacy, so a well-behaved app only reacts to the format it registered.
/// We therefore send v4 when we know it, legacy when we know it, and **both**
/// when the version is unknown — maximising "the click does something" without
/// risk of a double-activation. `SetForegroundWindow` (via `win32::focus`) is
/// the final fallback so the app at least surfaces.
pub fn activate(owner_hwnd: isize, uid: u32, callback_msg: u32, version: u32) -> Result<()> {
    if owner_hwnd == 0 {
        return Err(anyhow!("tray_activate: null owner hwnd"));
    }
    let owner = HWND(owner_hwnd as *mut c_void);
    if callback_msg == 0 {
        // No callback registered — best we can do is surface the window.
        let _ = crate::win32::focus(owner_hwnd);
        return Ok(());
    }
    let (x, y) = cursor_pos();
    let mut delivered = false;
    unsafe {
        if version == 4 || version == 0 {
            let w = loword_hiword(x as u16, y as u16) as usize;
            let l = loword_hiword(NIN_SELECT as u16, uid as u16);
            delivered |= post(owner, callback_msg, w, l);
        }
        if version != 4 {
            // legacy: wParam = uID, lParam = mouse message
            let down = post(owner, callback_msg, uid as usize, WM_LBUTTONDOWN);
            let up = post(owner, callback_msg, uid as usize, WM_LBUTTONUP);
            delivered |= down && up;
        }
    }
    if !delivered {
        let _ = crate::win32::focus(owner_hwnd);
    }
    Ok(())
}

/// Forward a right-click so the owning app pops its *own* native context menu.
///
/// Applies the documented (KB135788) foreground workaround: the owner must be
/// the foreground window for `TrackPopupMenu` to behave and dismiss cleanly.
/// Our HUD is a `WS_EX_NOACTIVATE` window, so we push the owner foreground
/// first (reusing the proven `win32::force_foreground` attach-input trick),
/// forward the contextmenu event, then post a benign `WM_NULL` so the menu's
/// modal loop tears down instead of lingering.
pub fn open_menu(owner_hwnd: isize, uid: u32, callback_msg: u32, version: u32) -> Result<()> {
    if owner_hwnd == 0 {
        return Err(anyhow!("tray_open_menu: null owner hwnd"));
    }
    let owner = HWND(owner_hwnd as *mut c_void);
    crate::win32::force_foreground(owner_hwnd);
    if callback_msg == 0 {
        return Ok(());
    }
    let (x, y) = cursor_pos();
    unsafe {
        if version == 4 || version == 0 {
            let w = loword_hiword(x as u16, y as u16) as usize;
            let l = loword_hiword(WM_CONTEXTMENU as u16, uid as u16);
            let _ = post(owner, callback_msg, w, l);
        }
        if version != 4 {
            let _ = post(owner, callback_msg, uid as usize, WM_RBUTTONDOWN);
            let _ = post(owner, callback_msg, uid as usize, WM_RBUTTONUP);
        }
        // KB135788 tail: nudge the message loop so the popup dismisses cleanly.
        let _ = PostMessageW(owner, WM_NULL, WPARAM(0), LPARAM(0));
    }
    Ok(())
}

/// Re-enumerate with verbose logging — dumps each button's raw TRAYDATA bytes,
/// parsed fields, tooltip and owner exe to `debug.log`. The primary tool for
/// confirming the struct offsets against the running shell. Mirrors the shape
/// of the existing `audio_diagnostics()` command.
pub fn diagnostics() -> String {
    crate::glog!("[tray] ===== tray_diagnostics begin =====");
    match read_tray_icons_inner(true) {
        Ok(icons) => {
            for ic in &icons {
                crate::glog!(
                    "[tray] icon id={} hwnd=0x{:x} uid={} cb=0x{:x} ver={} pid={} exe={} tooltip={:?} iconBytes={}",
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
                "[tray] ===== tray_diagnostics end ({} icons) =====",
                icons.len()
            );
            format!(
                "tray_diagnostics: {} icon(s) enumerated — raw TRAYDATA byte dumps written to debug.log",
                icons.len()
            )
        }
        Err(e) => {
            crate::glog!("[tray] tray_diagnostics FAILED: {e}");
            format!("tray unavailable: {e}")
        }
    }
}

// ───────────────────────────── poller ─────────────────────────────
//
// Reads run ONLY while the HUD is visible. `start()` is called from
// `toggle_hud → show`, `stop()` from `toggle_hud → hide`. A monotonic
// generation counter supersedes any previously-running loop, so rapid
// toggles can never leave two pollers alive.

static TRAY_GEN: AtomicU64 = AtomicU64::new(0);

/// Begin polling the tray (~1.5 s) and emitting `tray:changed`, with one
/// immediate read on start. Safe to call repeatedly; the previous loop exits.
pub fn start(app: AppHandle) {
    let generation = TRAY_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    // Read off-thread so toggle_hud stays snappy (cross-process reads + icon
    // PNG encoding can take a few ms).
    std::thread::spawn(move || {
        emit_once(&app);
        loop {
            std::thread::sleep(Duration::from_millis(1500));
            if TRAY_GEN.load(Ordering::SeqCst) != generation {
                break; // superseded by a newer start() or stopped by stop()
            }
            emit_once(&app);
        }
    });
}

/// Stop the active poller (HUD hidden). Bumping the generation invalidates the
/// running loop so it exits on its next tick.
pub fn stop() {
    TRAY_GEN.fetch_add(1, Ordering::SeqCst);
}

fn emit_once(app: &AppHandle) {
    // Skip the emit on a read error so the HUD keeps its last good grid rather
    // than flickering to empty; the unavailable state is surfaced separately by
    // the `list_tray_icons` command the frontend calls on show.
    if let Ok(icons) = read_tray_icons() {
        let _ = app.emit("tray:changed", icons);
    }
}
