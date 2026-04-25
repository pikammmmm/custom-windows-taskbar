use crate::{win32, pinned, icons, config};
use std::process::Command;
use serde::Serialize;
use tauri::State;

#[tauri::command]
pub fn launch(path: String) -> Result<(), String> {
    Command::new(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("launch failed: {e}"))
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
pub fn get_icon(exe_path: String) -> Result<String, String> {
    icons::get_icon_data_url(&exe_path).map_err(|e| e.to_string())
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
