use crate::{app_actions, win32, pinned, icons, config, autostart, shell_taskbar};
use crate::widgets::audio;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State};

/// Last items payload handed to `show_menu`. The menu window pulls this on
/// every show via `get_menu_items`, which avoids losing items when the
/// pre-show emit races the WebView's listener registration on first launch.
fn last_menu_items() -> &'static Mutex<Option<serde_json::Value>> {
    static SLOT: OnceLock<Mutex<Option<serde_json::Value>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Wall-clock instant the menu was most recently shown. Read by the
/// auto-dismiss poller in `dock_autohide` so we don't immediately hide the
/// menu in the brief window before it has had a chance to take focus.
fn menu_shown_at() -> &'static Mutex<Option<std::time::Instant>> {
    static SLOT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}
pub fn last_menu_shown_at() -> Option<std::time::Instant> {
    *menu_shown_at().lock().unwrap()
}
pub fn clear_menu_shown_at() {
    *menu_shown_at().lock().unwrap() = None;
}

#[tauri::command]
pub fn launch(path: String) -> Result<(), String> {
    Command::new(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("launch failed: {e}"))
}

/// Open a Windows shell URI (`ms-settings:`, `https://`, etc) via cmd /c
/// start. The empty title arg is required when start receives a single
/// quoted token — otherwise cmd misparses it as the title.
#[tauri::command]
pub fn launch_uri(uri: String) -> Result<(), String> {
    Command::new("cmd")
        .args(["/c", "start", "", &uri])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("launch_uri failed: {e}"))
}

#[tauri::command]
pub fn focus_window(hwnd: isize) -> Result<(), String> {
    win32::focus(hwnd).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn minimize_window(hwnd: isize) -> Result<(), String> {
    win32::minimize(hwnd).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn close_window(hwnd: isize) -> Result<(), String> {
    win32::close(hwnd).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn foreground_hwnd() -> isize {
    win32::foreground_hwnd()
}

#[tauri::command]
pub fn get_pinned(state: State<'_, pinned::PinnedHandle>) -> Vec<pinned::PinnedApp> {
    state.lock().unwrap().clone()
}

#[tauri::command]
pub fn get_icon(exe_path: String, hwnd: Option<isize>) -> Result<String, String> {
    icons::get_icon_data_url(&exe_path, hwnd).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct PinResult {
    pub pinned: Vec<pinned::PinnedApp>,
}

#[tauri::command]
pub fn pin_app(
    path: String,
    display_name: String,
    state: State<'_, pinned::PinnedHandle>,
) -> Result<PinResult, String> {
    let mut guard = state.lock().unwrap();
    if guard.iter().any(|a| a.path.eq_ignore_ascii_case(&path)) {
        return Ok(PinResult { pinned: guard.clone() });
    }
    guard.push(pinned::PinnedApp { path, display_name, icon_path: None });
    let path_file = config::pinned_path().map_err(|e| e.to_string())?;
    pinned::save_to(&path_file, &guard).map_err(|e| e.to_string())?;
    Ok(PinResult { pinned: guard.clone() })
}

#[tauri::command]
pub fn unpin_app(
    path: String,
    state: State<'_, pinned::PinnedHandle>,
) -> Result<PinResult, String> {
    let mut guard = state.lock().unwrap();
    guard.retain(|a| !a.path.eq_ignore_ascii_case(&path));
    let path_file = config::pinned_path().map_err(|e| e.to_string())?;
    pinned::save_to(&path_file, &guard).map_err(|e| e.to_string())?;
    Ok(PinResult { pinned: guard.clone() })
}

#[tauri::command]
pub fn set_hud_position(x: f64, y: f64) -> Result<(), String> {
    let mut s = config::load_settings().map_err(|e| e.to_string())?;
    s.hud_position = Some((x, y));
    config::save_settings(&s).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_autostart(enable: bool) -> Result<(), String> {
    let mut s = config::load_settings().map_err(|e| e.to_string())?;
    s.auto_start = enable;
    config::save_settings(&s).map_err(|e| e.to_string())?;
    if enable {
        autostart::enable().map_err(|e| e.to_string())
    } else {
        autostart::disable().map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn get_autostart() -> bool {
    autostart::is_enabled()
}

#[tauri::command]
pub fn set_volume(percent: u8) -> Result<(), String> {
    audio::set_volume(percent)
}

#[tauri::command]
pub fn set_mute(muted: bool) -> Result<(), String> {
    audio::set_mute(muted)
}

#[tauri::command]
pub fn open_start_menu() -> Result<(), String> {
    shell_taskbar::tap_start_key().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn minimize_all_windows() -> Result<(), String> {
    shell_taskbar::minimize_all_windows().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn hide_windows_taskbar() -> Result<usize, String> {
    shell_taskbar::hide_windows_taskbar().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn show_windows_taskbar() -> Result<usize, String> {
    shell_taskbar::show_windows_taskbar().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_hud(app: AppHandle) -> Result<bool, String> {
    let win = app.get_webview_window("hud")
        .ok_or_else(|| "hud window missing".to_string())?;
    let visible = win.is_visible().map_err(|e| e.to_string())?;
    if visible {
        // Play the HUD's outgoing CSS animation, then hide the window after
        // it finishes — without the delay we'd flash off mid-animation.
        let _ = app.emit_to("hud", "hud:hide-anim", ());
        let win_clone = win.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(190));
            let _ = win_clone.hide();
        });
        Ok(false)
    } else {
        win.show().map_err(|e| e.to_string())?;
        win.set_always_on_top(true).map_err(|e| e.to_string())?;
        // Fire-and-forget — HUD JS retriggers the entrance animation.
        let _ = app.emit_to("hud", "hud:show-anim", ());
        Ok(true)
    }
}

#[tauri::command]
pub fn close_hwnds(hwnds: Vec<isize>) -> Result<(), String> {
    for h in hwnds {
        let _ = win32::close(h);
    }
    Ok(())
}

#[tauri::command]
pub fn app_info(exe_path: String) -> Result<app_actions::AppInfo, String> {
    app_actions::app_info(&exe_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn show_in_explorer(path: String) -> Result<(), String> {
    app_actions::show_in_explorer(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn run_as_admin(path: String) -> Result<(), String> {
    app_actions::run_as_admin(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn show_properties(path: String) -> Result<(), String> {
    app_actions::show_properties(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn copy_to_clipboard(text: String) -> Result<(), String> {
    app_actions::copy_to_clipboard(&text).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
pub struct ShowMenuArgs {
    pub items: serde_json::Value,
    /// Cursor X in physical pixels (event.screenX × devicePixelRatio).
    pub x: i32,
    /// Cursor Y in physical pixels.
    pub y: i32,
    /// Pre-measured menu width (CSS pixels) so we can clamp to the screen.
    pub width: u32,
    /// Pre-measured menu height (CSS pixels).
    pub height: u32,
}

/// Position the dedicated menu window near the cursor, clamp it to the
/// monitor bounds, then show it. The menu's own JS renders the items via
/// the `menu:items` event we emit here.
#[tauri::command]
pub fn show_menu(app: AppHandle, args: ShowMenuArgs) -> Result<(), String> {
    let win = app.get_webview_window("menu")
        .ok_or_else(|| "no menu window".to_string())?;
    let monitor = win.current_monitor().ok().flatten()
        .ok_or_else(|| "no current monitor".to_string())?;
    let scale = monitor.scale_factor();
    let mon_w = monitor.size().width as i32;
    let mon_h = monitor.size().height as i32;
    let w_px = (args.width as f64 * scale).round() as i32;
    let h_px = (args.height as f64 * scale).round() as i32;
    // Anchor the menu's top-right corner near the cursor — most right-clicks
    // happen on dock icons whose tooltips fly upward, so opening upward feels
    // natural. Clamp so it never spills past the right or top edge.
    let mut x = args.x - w_px;
    let mut y = args.y - h_px;
    if x + w_px > mon_w { x = mon_w - w_px; }
    if x < 0 { x = 0; }
    if y + h_px > mon_h { y = mon_h - h_px; }
    if y < 0 { y = 0; }

    win.set_size(PhysicalSize::new(w_px as u32, h_px as u32))
        .map_err(|e| e.to_string())?;
    win.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    // Stash the items so the menu window can pull them on its first
    // post-show render — the event below is the fast path; `get_menu_items`
    // is the always-correct fallback when the listener isn't ready yet.
    *last_menu_items().lock().unwrap() = Some(args.items.clone());
    let _ = app.emit_to("menu", "menu:items", args.items);
    win.show().map_err(|e| e.to_string())?;
    win.set_always_on_top(true).map_err(|e| e.to_string())?;
    // Force the menu to take focus — Tauri's show() doesn't always activate
    // a topmost window that was previously hidden, and the auto-dismiss
    // poller in dock_autohide needs the menu to be the foreground at least
    // once before it's allowed to dismiss it (otherwise it'd hide a menu
    // that just never activated).
    let _ = win.set_focus();
    *menu_shown_at().lock().unwrap() = Some(std::time::Instant::now());
    Ok(())
}

/// Returns the items handed to the most recent `show_menu` call. Lets the
/// menu window's JS render without depending on event-listener timing.
#[tauri::command]
pub fn get_menu_items() -> serde_json::Value {
    last_menu_items()
        .lock()
        .unwrap()
        .clone()
        .unwrap_or(serde_json::Value::Array(vec![]))
}

#[tauri::command]
pub fn hide_menu(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("menu") {
        win.hide().map_err(|e| e.to_string())?;
    }
    clear_menu_shown_at();
    Ok(())
}
