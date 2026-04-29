use crate::{app_actions, win32, pinned, icons, config, autostart, shell_taskbar, import_pinned, stash};
use crate::widgets::{audio, files, media, start_menu, warp};
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
    // .exe runs directly via CreateProcess; everything else (docs, scripts,
    // .lnk shortcuts, archives, …) goes through the shell so the default
    // file handler is used. Without the split, double-clicking a pinned
    // .docx would fail because Word isn't on $PATH.
    let is_exe = path.to_lowercase().ends_with(".exe");
    if is_exe {
        Command::new(&path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("launch failed: {e}"))
    } else {
        Command::new("cmd")
            .args(["/c", "start", "", &path])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("launch failed: {e}"))
    }
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
    app: AppHandle,
) -> Result<PinResult, String> {
    let mut guard = state.lock().unwrap();
    if guard.iter().any(|a| a.path.eq_ignore_ascii_case(&path)) {
        return Ok(PinResult { pinned: guard.clone() });
    }
    guard.push(pinned::PinnedApp { path, display_name, icon_path: None });
    let path_file = config::pinned_path().map_err(|e| e.to_string())?;
    pinned::save_to(&path_file, &guard).map_err(|e| e.to_string())?;
    // Emit directly instead of waiting for the notify watcher — on some Win11
    // setups self-writes don't always fire a usable Modify event, so the dock
    // wouldn't see the new pin until the next external change.
    let _ = app.emit("pinned:changed", &*guard);
    Ok(PinResult { pinned: guard.clone() })
}

#[tauri::command]
pub fn recent_files() -> Result<Vec<import_pinned::RecentFile>, String> {
    import_pinned::recent_files(7).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn stash_list(state: State<'_, stash::StashHandle>) -> Vec<stash::StashEntry> {
    state.lock().unwrap().clone()
}

#[tauri::command]
pub fn stash_add(
    paths: Vec<String>,
    state: State<'_, stash::StashHandle>,
    app: AppHandle,
) -> Result<Vec<stash::StashEntry>, String> {
    let mut guard = state.lock().unwrap();
    let mut changed = false;
    for p in paths {
        if guard.iter().any(|e| e.path.eq_ignore_ascii_case(&p)) { continue; }
        if let Some(entry) = stash::entry_for(&p) {
            guard.push(entry);
            changed = true;
        }
    }
    if changed {
        stash::save(&guard).map_err(|e| e.to_string())?;
        let _ = app.emit("stash:changed", &*guard);
    }
    Ok(guard.clone())
}

#[tauri::command]
pub fn stash_remove(
    path: String,
    state: State<'_, stash::StashHandle>,
    app: AppHandle,
) -> Result<Vec<stash::StashEntry>, String> {
    let mut guard = state.lock().unwrap();
    let before = guard.len();
    guard.retain(|e| !e.path.eq_ignore_ascii_case(&path));
    if guard.len() != before {
        stash::save(&guard).map_err(|e| e.to_string())?;
        let _ = app.emit("stash:changed", &*guard);
    }
    Ok(guard.clone())
}

#[tauri::command]
pub fn stash_clear(
    state: State<'_, stash::StashHandle>,
    app: AppHandle,
) -> Result<(), String> {
    let mut guard = state.lock().unwrap();
    guard.clear();
    stash::save(&guard).map_err(|e| e.to_string())?;
    let _ = app.emit("stash:changed", &*guard);
    Ok(())
}

/// Unified spotlight result — apps from the Start Menu index plus files
/// from the user's common folders. The `kind` field lets the UI render a
/// small label (App/File) without needing to inspect the path.
#[derive(Serialize)]
pub struct LauncherEntry {
    pub kind: &'static str,
    pub name: String,
    pub path: String,
}

#[tauri::command]
pub fn search_apps(query: String) -> Vec<LauncherEntry> {
    let mut out: Vec<LauncherEntry> = Vec::new();
    for a in start_menu::search(&query, 8) {
        out.push(LauncherEntry { kind: "app", name: a.name, path: a.path });
    }
    // Files only when the user has typed something — an empty query would
    // dump tens of thousands of indexed paths into the list.
    if !query.trim().is_empty() {
        for f in files::search(&query, 8) {
            out.push(LauncherEntry { kind: "file", name: f.name, path: f.path });
        }
    }
    out
}

#[tauri::command]
pub fn show_spotlight(app: AppHandle) -> Result<(), String> {
    let win = app.get_webview_window("spotlight")
        .ok_or_else(|| "spotlight window missing".to_string())?;
    let monitor = win.current_monitor().ok().flatten()
        .ok_or_else(|| "no current monitor".to_string())?;
    let mon_w = monitor.size().width as i32;
    let mon_h = monitor.size().height as i32;
    let scale = monitor.scale_factor();
    let w = (560.0 * scale).round() as i32;
    let h = (440.0 * scale).round() as i32;
    let x = (mon_w - w) / 2;
    // Position one-third down the screen — feels natural and leaves room
    // for the result list below the input box.
    let y = (mon_h as f64 / 3.5) as i32;
    win.set_size(PhysicalSize::new(w as u32, h as u32))
        .map_err(|e| e.to_string())?;
    win.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    win.show().map_err(|e| e.to_string())?;
    win.set_always_on_top(true).map_err(|e| e.to_string())?;
    let _ = win.set_focus();
    if let Ok(hwnd) = win.hwnd() {
        let h = hwnd.0 as isize;
        crate::dwm::strip_decorations(h);
        crate::dwm::suppress_nc_rendering(h);
    }
    let _ = app.emit_to("spotlight", "spotlight:show", ());
    Ok(())
}

#[tauri::command]
pub fn hide_spotlight(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("spotlight") {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Pin a batch of dropped paths (from a window file-drop). Resolves
/// `.lnk` to the target exe; silently skips anything that isn't a
/// launchable .exe / .lnk so the user gets no surprise from dragging
/// a folder or document onto the dock.
#[tauri::command]
pub fn pin_dropped(
    paths: Vec<String>,
    state: State<'_, pinned::PinnedHandle>,
    app: AppHandle,
) -> Result<PinResult, String> {
    let mut guard = state.lock().unwrap();
    let mut changed = false;
    for raw in paths {
        let Some((exe, display)) = import_pinned::resolve_drop(std::path::Path::new(&raw)) else {
            continue;
        };
        if guard.iter().any(|a| a.path.eq_ignore_ascii_case(&exe)) {
            continue;
        }
        guard.push(pinned::PinnedApp { path: exe, display_name: display, icon_path: None });
        changed = true;
    }
    if changed {
        let path_file = config::pinned_path().map_err(|e| e.to_string())?;
        pinned::save_to(&path_file, &guard).map_err(|e| e.to_string())?;
        let _ = app.emit("pinned:changed", &*guard);
    }
    Ok(PinResult { pinned: guard.clone() })
}

#[tauri::command]
pub fn unpin_app(
    path: String,
    state: State<'_, pinned::PinnedHandle>,
    app: AppHandle,
) -> Result<PinResult, String> {
    let mut guard = state.lock().unwrap();
    guard.retain(|a| !a.path.eq_ignore_ascii_case(&path));
    let path_file = config::pinned_path().map_err(|e| e.to_string())?;
    pinned::save_to(&path_file, &guard).map_err(|e| e.to_string())?;
    let _ = app.emit("pinned:changed", &*guard);
    Ok(PinResult { pinned: guard.clone() })
}

/// Persist a new pinned-app order. The dock hands us the full list of
/// paths in their post-drag-and-drop sequence; we permute the in-memory
/// list to match, save, and broadcast `pinned:changed` so every listener
/// (dock, HUD) re-renders.
#[tauri::command]
pub fn set_pinned_order(
    paths: Vec<String>,
    state: State<'_, pinned::PinnedHandle>,
    app: AppHandle,
) -> Result<PinResult, String> {
    let mut guard = state.lock().unwrap();
    pinned::reorder(&mut guard, &paths);
    let path_file = config::pinned_path().map_err(|e| e.to_string())?;
    pinned::save_to(&path_file, &guard).map_err(|e| e.to_string())?;
    let _ = app.emit("pinned:changed", &*guard);
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
pub fn list_audio_devices() -> Result<Vec<audio::AudioDevice>, String> {
    audio::list_devices()
}

#[tauri::command]
pub fn set_default_audio_device(id: String) -> Result<(), String> {
    audio::set_default_device(&id)
}

#[tauri::command]
pub fn warp_toggle(connect: bool) -> Result<(), String> {
    warp::toggle(connect)
}

#[tauri::command]
pub fn media_toggle_play() -> Result<(), String> {
    media::toggle_play_pause().map_err(|e| e.to_string())
}
#[tauri::command]
pub fn media_next() -> Result<(), String> {
    media::next().map_err(|e| e.to_string())
}
#[tauri::command]
pub fn media_prev() -> Result<(), String> {
    media::prev().map_err(|e| e.to_string())
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

/// "Close all" from the dock's right-click menu — matches Task Manager's
/// End Task semantics. Posts WM_CLOSE first so apps that handle it can
/// flush state, then after a short grace window force-kills the process
/// for any windows still alive.
#[tauri::command]
pub fn close_hwnds(hwnds: Vec<isize>) -> Result<(), String> {
    use std::collections::HashSet;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;

    // Phase 1 — graceful WM_CLOSE for every hwnd.
    for &h in &hwnds {
        let _ = win32::close(h);
    }

    // Phase 2 — wait briefly, then TerminateProcess on whatever is left.
    // Spawned so the IPC reply can return immediately to the dock UI.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        let mut killed: HashSet<u32> = HashSet::new();
        for h in hwnds {
            // Skip if the window already went away.
            unsafe {
                if !IsWindow(HWND(h as *mut _)).as_bool() { continue; }
            }
            let pid = win32::pid_of(h);
            if pid == 0 || !killed.insert(pid) { continue; }
            let _ = win32::terminate_process_of(h);
        }
    });

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
    // Re-strip after show — Tauri occasionally re-applies WS_CAPTION between
    // hide() and show() on Win11, which would flash min/max/close buttons
    // before our paint lands.
    if let Ok(hwnd) = win.hwnd() {
        let h = hwnd.0 as isize;
        crate::dwm::strip_decorations(h);
        crate::dwm::suppress_nc_rendering(h);
    }
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
