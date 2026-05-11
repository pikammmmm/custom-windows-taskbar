use crate::{app_actions, win32, pinned, icons, config, autostart, shell_taskbar, import_pinned, stash};
use crate::win32::CommandHidden;
use crate::widgets::{audio, clipboard as clip, files, keyboard, media, start_menu, warp, weather};
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
    // shell:AppsFolder\<AppID> launches a UWP / Store app the same way the
    // OS Start menu does. CreateProcess can't resolve these — they go
    // through the shell namespace and only explorer.exe knows how.
    if path.starts_with("shell:") {
        return Command::new("explorer.exe")
            .arg(&path)
            .hidden()
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("launch failed: {e}"));
    }
    // Resolve stale versioned paths before doing anything else. Roblox,
    // MSIX, and Squirrel apps all delete the previous version's directory
    // on auto-update, which means a pinned path becomes a dead path the
    // moment the user accepts an update. Find the live sibling instead so
    // the click "just works."
    let path = resolve_stale_versioned_path(&path).unwrap_or(path);
    crate::glog!("launch: resolved path = {path}");
    // .exe runs directly via CreateProcess; everything else (docs, scripts,
    // .lnk shortcuts, archives, web URLs, …) goes through ShellExecuteW
    // with the "open" verb — same code path as double-clicking the file in
    // Explorer. cmd /c start was unreliable for shortcuts whose paths
    // contain cmd metacharacters or unusual shell associations (game
    // launcher .lnks were a recurring miss).
    let is_exe = path.to_lowercase().ends_with(".exe");
    // UWP / MSIX apps installed under \Program Files\WindowsApps\ have
    // restricted ACLs — direct CreateProcess often fails with access
    // denied even though Explorer can launch them. Route those through
    // ShellExecute "open" verb instead, which uses the package's
    // declared entry point. This is what lets the user re-open closed
    // Microsoft Store apps (Terminal, Calculator, Claude) from the
    // dock without seeing "nothing happens" silent failures.
    if is_exe && path.to_lowercase().contains("\\windowsapps\\") {
        crate::glog!("launch: routing WindowsApps path via ShellExecute");
        return app_actions::invoke_shell_verb(&path, "open")
            .map_err(|e| format!("launch failed: {e}"));
    }
    if is_exe {
        // Squirrel-installed apps (Discord, Slack, GitHub Desktop, WhatsApp,
        // Teams (older), Atom, etc.) live at <App>\app-VERSION\<App>.exe
        // with an Update.exe launcher in the grandparent dir. The versioned
        // path rotates on update, and even when the file still exists,
        // launching the .exe directly skips Squirrel's bootstrap — apps can
        // refuse to start, fail to single-instance, or break their auto-
        // updater. Always go through Update.exe when the layout matches.
        if let Some(res) = try_squirrel_launch(&path) {
            return res;
        }
        // Set CWD to the exe's own directory. Many games and bundled apps
        // load DLLs / configs from a working directory matching their
        // install dir — Hitman 3's launcher, Steam shortcuts, anything
        // shipping side-by-side resources. Inheriting glassbar's CWD
        // makes them fail silently. Mirrors what Explorer does on
        // double-click.
        let mut cmd = Command::new(&path);
        if let Some(parent) = std::path::Path::new(&path).parent() {
            if !parent.as_os_str().is_empty() {
                cmd.current_dir(parent);
            }
        }
        cmd.hidden()
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("launch failed: {e}"))
    } else {
        app_actions::invoke_shell_verb(&path, "open")
            .map_err(|e| format!("launch failed: {e}"))
    }
}

/// If `path` exists, return None. Otherwise, when the path contains a
/// versioned directory segment (`version-<hash>`, `app-X.Y.Z`, or an
/// MSIX-style `Name_Ver_Arch__Hash` segment) that no longer exists,
/// scan the parent of that segment for a sibling subdir with the same
/// shape, prefer the one most recently modified, and reconstruct the
/// path with the live segment swapped in.
///
/// Catches the most common "I clicked Roblox and nothing happened"
/// failure mode: Roblox/Discord/Claude auto-update, the previous version
/// dir is removed, the pinned exe path now points at nothing.
fn resolve_stale_versioned_path(path: &str) -> Option<String> {
    use std::path::{Component, PathBuf};
    if std::path::Path::new(path).exists() {
        return None;
    }
    let pb = PathBuf::from(path);
    let comps: Vec<_> = pb.components().collect();

    // Walk components looking for a versioned-shape segment. We test
    // against three patterns: Squirrel `app-X.Y.Z`, Roblox `version-<id>`,
    // MSIX `Name_Ver_Arch__Hash`. First match wins. (The earlier `?`
    // here was a bug — it bailed out the entire function on the first
    // *non*-versioned segment like `Users`, before we reached the real
    // versioned dir deeper in the path.)
    for (idx, c) in comps.iter().enumerate() {
        let Component::Normal(seg) = c else { continue };
        let seg_str = seg.to_string_lossy();
        let Some(pattern) = detect_versioned_pattern(&seg_str) else { continue };
        if !pattern.matches(&seg_str) { continue; }

        // Reconstruct the parent path of this segment.
        let mut parent = PathBuf::new();
        for c in &comps[..idx] {
            parent.push(c.as_os_str());
        }
        let parent = parent;
        let Ok(rd) = std::fs::read_dir(&parent) else { return None };

        // Find sibling subdirs that share the pattern, prefer most-recent.
        let mut candidates: Vec<(std::path::PathBuf, std::time::SystemTime)> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if !pattern.matches(&name) { return None; }
                let mtime = e.metadata().ok().and_then(|m| m.modified().ok())?;
                Some((e.path(), mtime))
            })
            .collect();
        if candidates.is_empty() { return None; }
        candidates.sort_by_key(|(_, t)| std::cmp::Reverse(*t));

        // Try each candidate in age order, pick the first whose
        // reconstructed full path actually has the file we want.
        let tail: PathBuf = comps[idx + 1..].iter().map(|c| c.as_os_str()).collect();
        for (winner, _) in candidates {
            let candidate_full = winner.join(&tail);
            if candidate_full.exists() {
                return Some(candidate_full.to_string_lossy().into_owned());
            }
        }
        return None;
    }
    None
}

enum VersionedPattern {
    Squirrel,
    Versioned,
    Msix { name: String, hash: String },
}

impl VersionedPattern {
    fn matches(&self, seg: &str) -> bool {
        let s = seg.to_lowercase();
        match self {
            VersionedPattern::Squirrel => s.starts_with("app-"),
            VersionedPattern::Versioned => s.starts_with("version-"),
            VersionedPattern::Msix { name, hash } => {
                // `<name>_<ver>_<arch>__<hash>`. Split on `__` first so
                // the hash separator isn't eaten by the leading splitn
                // on `_` (the bug the previous shape had — splitn(4,
                // '_') consumed all four underscores in
                // `Microsoft.WindowsTerminal_1.24.10921.0_x64__8wekyb…`
                // and the `__hash` half lost its `__` marker).
                let Some((left, right)) = s.split_once("__") else { return false };
                // left = `<name>_<ver>_<arch>`. rsplit so we pick name
                // off the front even if name contains underscores.
                let left_parts: Vec<&str> = left.rsplitn(3, '_').collect();
                if left_parts.len() != 3 { return false; }
                let other_name = left_parts[2];
                other_name == name.to_lowercase() && right == hash.to_lowercase()
            }
        }
    }
}

fn detect_versioned_pattern(seg: &str) -> Option<VersionedPattern> {
    let s = seg.to_lowercase();
    if s.starts_with("app-") { return Some(VersionedPattern::Squirrel); }
    if s.starts_with("version-") { return Some(VersionedPattern::Versioned); }
    // MSIX: Name_Ver_Arch__Hash. Split on `__` first so we don't
    // mis-allocate underscores between segments.
    let (left, right) = s.split_once("__")?;
    let left_parts: Vec<&str> = left.rsplitn(3, '_').collect();
    if left_parts.len() != 3 { return None; }
    Some(VersionedPattern::Msix {
        name: left_parts[2].to_string(),
        hash: right.to_string(),
    })
}

/// Detect the Squirrel install layout (`<App>\app-VERSION\<App>.exe` with
/// `<App>\Update.exe` alongside) and launch via Update.exe's `--processStart`
/// flag, which is what Squirrel installer-published apps want. Returns
/// `Some(Result)` if the launch was attempted, `None` if the layout doesn't
/// match and the caller should fall back to a direct CreateProcess.
fn try_squirrel_launch(path: &str) -> Option<Result<(), String>> {
    let p = std::path::Path::new(path);
    let parent = p.parent()?;
    let grandparent = parent.parent()?;
    let update_exe = grandparent.join("Update.exe");

    // Parent dir must be the versioned `app-*` folder Squirrel creates.
    let parent_name = parent.file_name()?.to_str()?;
    if !parent_name.starts_with("app-") { return None; }
    if !update_exe.exists() { return None; }

    let exe_name = p.file_name()?.to_str()?;

    // The dock only routes to `launch` when no visible window exists for
    // this exe (otherwise it focuses the existing window). Squirrel's
    // --processStart silently no-ops if it sees the app is already
    // running — which is the bug that left Discord stuck headless after
    // a tray-minimize. Kill any existing instance first so Squirrel
    // always spawns a fresh window. None of these apps (Discord, Slack,
    // WhatsApp, GitHub Desktop) hold unsaved state, so the kill is safe.
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", exe_name])
        .hidden()
        .output();
    std::thread::sleep(std::time::Duration::from_millis(300));

    let result = Command::new(&update_exe)
        .args(["--processStart", exe_name])
        .current_dir(grandparent)
        .hidden()
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("squirrel launch failed ({}): {e}", update_exe.display()));
    Some(result)
}

/// Open a Windows shell URI (`ms-settings:`, `https://`, etc) via cmd /c
/// start. The empty title arg is required when start receives a single
/// quoted token — otherwise cmd misparses it as the title.
#[tauri::command]
pub fn launch_uri(uri: String) -> Result<(), String> {
    Command::new("cmd")
        .args(["/c", "start", "", &uri])
        .hidden()
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

#[derive(Serialize)]
pub struct GeoCity {
    pub name: String,
    pub admin: Option<String>,
    pub country: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

/// Open-Meteo's geocoding API — name → list of candidate cities. Used by
/// the HUD's city picker so the user can switch weather location without
/// editing settings.json by hand. No API key required.
#[tauri::command]
pub fn geocode_city(query: String) -> Result<Vec<GeoCity>, String> {
    let q = query.trim();
    if q.is_empty() { return Ok(Vec::new()); }
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=8&language=en&format=json",
        urlencode(q)
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(8))
        .build();
    let body: serde_json::Value = agent
        .get(&url)
        .set("User-Agent", "glassbar/0.1")
        .call()
        .map_err(|e| format!("geocode failed: {e}"))?
        .into_json()
        .map_err(|e| format!("geocode parse failed: {e}"))?;
    let results = body.get("results").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    Ok(results.into_iter().filter_map(|r| {
        Some(GeoCity {
            name: r.get("name")?.as_str()?.to_string(),
            admin: r.get("admin1").and_then(|x| x.as_str()).map(str::to_string),
            country: r.get("country").and_then(|x| x.as_str()).map(str::to_string),
            lat: r.get("latitude")?.as_f64()?,
            lon: r.get("longitude")?.as_f64()?,
        })
    }).collect())
}

/// Persist the selected city so the next weather poll uses it. Frontend
/// hands us the geocode result; we save name + coords to settings.json,
/// then poke the weather probe so the HUD updates within ~1s instead of
/// waiting up to 15 minutes for the next scheduled poll.
#[tauri::command]
pub fn set_weather_city(name: String, lat: f64, lon: f64) -> Result<(), String> {
    let mut s = config::load_settings().map_err(|e| e.to_string())?;
    s.weather_city = Some(name);
    s.weather_lat = Some(lat);
    s.weather_lon = Some(lon);
    config::save_settings(&s).map_err(|e| e.to_string())?;
    weather::request_refresh();
    Ok(())
}

/// Read the currently saved city — frontend uses this to pre-fill the
/// picker on first paint and to detect first-run state (None = ask).
#[tauri::command]
pub fn get_weather_city() -> Option<GeoCity> {
    let s = config::load_settings().ok()?;
    Some(GeoCity {
        name: s.weather_city?,
        admin: None,
        country: None,
        lat: s.weather_lat?,
        lon: s.weather_lon?,
    })
}

/// Minimal URL-component encode — geocoding queries are short, plain
/// city names so we just escape the bytes ureq's URL parser would refuse.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.bytes() {
        match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(c as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", c)),
        }
    }
    out
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

/// Set the system master volume. Returns the percent Windows actually
/// committed (endpoints snap to discrete steps), and broadcasts an
/// `audio:changed` event so the dock chip + HUD update without waiting
/// for the next snapshot tick. The HUD slider uses the returned value
/// to keep its user-intent cache in sync — without that the slider
/// would visually jump 1-2 percentage points when intent expires.
///
/// Also persists the committed value to settings.json so the HUD can
/// seed its slider with it on reopen — without persistence the slider
/// briefly flashed back to its HTML default (50%) on every show until
/// the next snapshot tick (~400 ms) overwrote it. Save is best-effort:
/// any write failure is logged but not surfaced.
#[tauri::command]
pub fn set_volume(app: AppHandle, percent: u8) -> Result<u8, String> {
    let actual = audio::set_volume(percent)?;
    let _ = app.emit("audio:changed", actual);
    if let Ok(mut s) = config::load_settings() {
        if s.volume_percent != Some(actual) {
            s.volume_percent = Some(actual);
            if let Err(e) = config::save_settings(&s) {
                crate::glog!("set_volume: save settings failed: {e}");
            }
        }
    }
    Ok(actual)
}

/// Returns the last-set volume from settings.json — None if the user
/// hasn't moved the slider yet on this install. The HUD calls this on
/// startup and on show-anim to seed the slider before the first snapshot
/// tick lands, avoiding a brief flash to the HTML-default value.
#[tauri::command]
pub fn get_settings_volume() -> Option<u8> {
    config::load_settings().ok().and_then(|s| s.volume_percent)
}

/// Returns the *current* system volume, not the last-persisted value.
/// HUD seeds the slider from this on reopen so the displayed % matches
/// reality even if the user changed system volume via media keys /
/// Windows tray slider since the last glassbar interaction.
#[tauri::command]
pub fn get_current_volume() -> audio::AudioState {
    audio::current()
}

/// Dump every audio endpoint's reported scalar to debug.log. Called from
/// the HUD when the user clicks a "diagnose volume" button — for now also
/// invokable directly so we have a way to triage the recurring "HUD shows
/// wrong %" reports without a fresh build.
#[tauri::command]
pub fn audio_diagnostics() -> audio::AudioState {
    audio::log_endpoint_diagnostics();
    audio::current()
}

/// Switch the active keyboard layout to the layout identified by the
/// raw HKL value (returned in the snapshot's `keyboard.installed`).
/// Same effect as Win+Space, scoped to the foreground window.
#[tauri::command]
pub fn set_keyboard_layout(hkl: u32) -> Result<(), String> {
    keyboard::activate(hkl)
}

/// Append a single line to %APPDATA%\glassbar\debug.log from the
/// frontend. Lets clipboard / dock / HUD JS log lifecycle events
/// (panel-show focus events, click handlers, listener registrations)
/// without each surface inventing its own console-routing scheme.
/// Cheap enough to call freely; the logger drops on I/O failure.
#[tauri::command]
pub fn dbg_log(message: String) {
    crate::glog!("[js] {message}");
}

#[tauri::command]
pub fn set_mute(app: AppHandle, muted: bool) -> Result<(), String> {
    audio::set_mute(muted)?;
    // Re-broadcast current volume + new mute state via the snapshot path
    // — there's no per-state event for mute; the next snapshot tick will
    // reflect it within ~400ms. (Skipping a synthetic event here keeps
    // the wire format simple.)
    let _ = app.emit("audio:mute-changed", muted);
    Ok(())
}

#[tauri::command]
pub fn list_audio_devices() -> Result<Vec<audio::AudioDevice>, String> {
    audio::list_devices()
}

/// Same as list_audio_devices but takes a `flow` arg so the dock's volume
/// menu can show separate sections for output (speakers/headphones) and
/// input (microphones/line-in). Without this the menu only ever showed
/// the output side.
#[tauri::command]
pub fn list_audio_devices_for(flow: audio::Flow) -> Result<Vec<audio::AudioDevice>, String> {
    let result = audio::list_devices_for(flow);
    match &result {
        Ok(list) => crate::glog!(
            "list_audio_devices_for({:?}): {} devices [{}]",
            flow,
            list.len(),
            list.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(" | ")
        ),
        Err(e) => crate::glog!("list_audio_devices_for({:?}) FAILED: {e}", flow),
    }
    result
}

#[tauri::command]
pub fn set_default_audio_device(id: String) -> Result<(), String> {
    crate::glog!("set_default_audio_device: id={id}");
    let result = audio::set_default_device(&id);
    match &result {
        Ok(()) => crate::glog!("set_default_audio_device: ok"),
        Err(e) => crate::glog!("set_default_audio_device: FAIL {e}"),
    }
    result
}

#[tauri::command]
pub fn warp_toggle(app: AppHandle, connect: bool) -> Result<(), String> {
    let result = warp::toggle(connect);
    // Force an immediate status re-read so the snapshot reflects the new
    // state on the next ~400ms tick instead of waiting up to 5s for the
    // next scheduled probe. The user perceives the button as
    // "responsive" instead of "didn't do anything."
    warp::refresh_global();
    // Re-emit the audio-changed event as a generic "snapshot may have
    // changed" nudge — same pattern set_volume uses to push instant UI
    // updates. The dock and HUD just-re-render their tray chips.
    let _ = app.emit("warp:changed", connect);
    result
}

/// System power actions: lock / sleep / signout / restart / shutdown.
/// Single command keyed by string so the HUD doesn't have to register
/// five separate handlers. Restart/shutdown/signout require a confirm
/// click on the JS side — they're irreversible from here.
#[tauri::command]
pub fn power_action(action: String) -> Result<(), String> {
    let result = match action.as_str() {
        "lock"     => Command::new("rundll32.exe")
            .args(["user32.dll,LockWorkStation"])
            .hidden()
            .spawn(),
        // SetSuspendState arg "0,1,0" = sleep (not hibernate), force, no wake events.
        "sleep"    => Command::new("rundll32.exe")
            .args(["powrprof.dll,SetSuspendState", "0,1,0"])
            .hidden()
            .spawn(),
        "signout"  => Command::new("shutdown").args(["/l"]).hidden().spawn(),
        "restart"  => Command::new("shutdown").args(["/r", "/t", "0"]).hidden().spawn(),
        "shutdown" => Command::new("shutdown").args(["/s", "/t", "0"]).hidden().spawn(),
        other => return Err(format!("unknown power action: {other}")),
    };
    result.map(|_| ()).map_err(|e| format!("power_action failed: {e}"))
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
        // emit_to+eval pair: eval is the reliable backstop because
        // emit_to can be silently dropped on Tauri 2 (same root cause as
        // the clipboard-empty bug).
        let _ = app.emit_to("hud", "hud:hide-anim", ());
        let _ = win.eval("window.__glassbarHudPlayHideAnim && window.__glassbarHudPlayHideAnim();");
        let win_clone = win.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(190));
            let _ = win_clone.hide();
        });
        Ok(false)
    } else {
        win.show().map_err(|e| e.to_string())?;
        win.set_always_on_top(true).map_err(|e| e.to_string())?;
        // Same eval bypass as clipboard show. The named event
        // (hud:show-anim) was missed often enough that the volume slider
        // and entrance animation didn't replay on reopen — the HUD
        // looked like it had reset to defaults. Eval is the reliable
        // path; the named emit stays as a fast-path for surfaces that
        // happen to receive it.
        let _ = app.emit_to("hud", "hud:show-anim", ());
        let _ = win.eval("window.__glassbarHudPlayShowAnim && window.__glassbarHudPlayShowAnim();");
        Ok(true)
    }
}

/// "Close all" from the dock's right-click menu — matches Task Manager's
/// End Task semantics. Posts WM_CLOSE first so apps that handle it can
/// flush state, then after a short grace window force-kills the windowed
/// process AND any helper processes sharing the same exe basename. The
/// helper-cleanup matters for Squirrel apps (Discord, Slack, GitHub
/// Desktop, WhatsApp) which leave GPU / renderer / network helpers alive
/// otherwise — preventing the next dock click from spawning a fresh
/// window because Squirrel sees the app as still-running.
#[tauri::command]
pub fn close_hwnds(hwnds: Vec<isize>) -> Result<(), String> {
    use std::collections::HashSet;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;

    // Capture the exe basename of every hwnd BEFORE we kill anything,
    // since the PID lookups become unreliable once the process exits.
    let mut image_names: HashSet<String> = HashSet::new();
    for &h in &hwnds {
        let pid = win32::pid_of(h);
        if pid == 0 { continue; }
        if let Some(exe) = win32::exe_of_pid(pid) {
            if let Some(name) = std::path::Path::new(&exe)
                .file_name().and_then(|n| n.to_str())
            {
                image_names.insert(name.to_string());
            }
        }
    }

    // Phase 1 — graceful WM_CLOSE for every hwnd.
    for &h in &hwnds {
        let _ = win32::close(h);
    }

    // Phase 2 — wait briefly, then force-kill anything still alive.
    // Spawned so the IPC reply can return immediately to the dock UI.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        let mut killed: HashSet<u32> = HashSet::new();
        for h in hwnds {
            unsafe { if !IsWindow(HWND(h as *mut _)).as_bool() { continue; } }
            let pid = win32::pid_of(h);
            if pid == 0 || !killed.insert(pid) { continue; }
            let _ = win32::terminate_process_of(h);
        }
        // Phase 3 — taskkill /F /IM each exe basename. Catches helper
        // processes (Squirrel renderers, GPU helpers) that don't have
        // visible windows so the next launch sees a fully-clean slate.
        for name in image_names {
            let _ = Command::new("taskkill")
                .args(["/F", "/IM", &name])
                .hidden()
                .output();
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
    // Clamp the menu height to the monitor — without this a power-user menu
    // taller than the screen would extend off-bottom and the rows you want
    // would be unreachable. The CSS makes #items scroll inside the
    // remaining space.
    let mut h_px = (args.height as f64 * scale).round() as i32;
    let max_h = mon_h - 16;
    if h_px > max_h { h_px = max_h; }
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

// ────────────────────────────────────────────────────────────────────────
// Win+X power-user menu — replaces Windows' built-in WinX popup with the
// glassbar-themed menu window. Items are mostly `launch_uri` (settings
// pages + .msc consoles) so we don't need bespoke handlers per row.
// ────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn show_power_menu(app: AppHandle) -> Result<(), String> {
    use serde_json::json;
    // Toggle: a second Win+X press while the menu is already up should
    // dismiss it, not re-show. Without this the rapid-tap that users
    // naturally do (Win+X, Win+X to "make sure it opened") races our
    // keyhook + the focus-loss auto-hide and can leave the OS Win+X menu
    // surfacing on the second press.
    if let Some(win) = app.get_webview_window("menu") {
        if win.is_visible().unwrap_or(false) {
            return hide_menu(app);
        }
    }

    let items = json!([
        { "label": "Apps & Features",       "action": "launch_uri", "args": { "uri": "ms-settings:appsfeatures" } },
        { "label": "Power Options",         "action": "launch_uri", "args": { "uri": "ms-settings:powersleep" } },
        { "label": "Settings",              "action": "launch_uri", "args": { "uri": "ms-settings:" } },
        { "label": "Network Connections",   "action": "launch_uri", "args": { "uri": "ms-settings:network-status" } },
        { "kind":  "separator" },
        { "label": "Device Manager",        "action": "launch", "args": { "path": "devmgmt.msc" } },
        { "label": "Disk Management",       "action": "launch", "args": { "path": "diskmgmt.msc" } },
        { "label": "Computer Management",   "action": "launch", "args": { "path": "compmgmt.msc" } },
        { "label": "Event Viewer",          "action": "launch", "args": { "path": "eventvwr.msc" } },
        { "label": "Task Manager",          "action": "launch", "args": { "path": "taskmgr.exe" } },
        { "kind":  "separator" },
        { "label": "Terminal",              "action": "launch", "args": { "path": "wt.exe" } },
        { "label": "Terminal (Admin)",      "action": "run_as_admin", "args": { "path": "wt.exe" } },
        { "label": "File Explorer",         "action": "launch", "args": { "path": "explorer.exe" } },
        { "label": "Search",                "action": "show_spotlight", "args": {} },
        { "kind":  "separator" },
        { "label": "Sign out",              "action": "power_action", "args": { "action": "signout" } },
        { "label": "Lock",                  "action": "power_action", "args": { "action": "lock" } },
        { "label": "Sleep",                 "action": "power_action", "args": { "action": "sleep" } },
        { "label": "Restart",               "action": "power_action", "args": { "action": "restart" }, "danger": true },
        { "label": "Shut down",             "action": "power_action", "args": { "action": "shutdown" }, "danger": true },
    ]);

    // Anchor the menu in the bottom-left corner of the primary monitor —
    // that's where the real Win+X opens, hugging the start area. show_menu
    // anchors the menu's bottom-right at the cursor coords we hand it, so
    // (240, screen_h - 60) lands the menu just above-and-left of the dock,
    // mirroring the OS placement.
    let win = app.get_webview_window("menu")
        .ok_or_else(|| "menu window missing".to_string())?;
    let monitor = win.current_monitor().ok().flatten()
        .ok_or_else(|| "no current monitor".to_string())?;
    let mon_h = monitor.size().height as i32;
    show_menu(app, ShowMenuArgs {
        items,
        x: 240,
        y: mon_h - 60,
        // Match the menu's natural width — show_menu re-clamps if needed.
        width: 220,
        // ~22 rows × 30px + some padding. show_menu clamps if it would
        // spill off the top.
        height: 560,
    })
}

// ────────────────────────────────────────────────────────────────────────
// Win+V clipboard history panel — read live history, set the clipboard
// when the user picks an entry, paste into the previously-focused window.
// ────────────────────────────────────────────────────────────────────────

/// Per-entry payload sent to the panel. The `kind` field is a tagged enum
/// so the JS can render text rows and image previews differently without
/// any null-checking on a single shared `text` field.
#[derive(Serialize)]
#[serde(tag = "kind")]
pub enum ClipboardItemKind {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        width: u32,
        height: u32,
        /// PNG data URL — drop into <img src=> to render the preview.
        /// Capped at IMAGE_PREVIEW_BYTES so a 50 MP screenshot doesn't
        /// blow up the IPC message.
        data_url: Option<String>,
        byte_size: usize,
    },
}

#[derive(Serialize)]
pub struct ClipboardItem {
    /// Stable entry id — handed back to clipboard_use_entry so we don't
    /// have to ship full image payloads through the IPC just to identify
    /// "the one the user clicked."
    pub id: u64,
    pub item: ClipboardItemKind,
    /// Seconds since the entry was captured. Frontend formats as "just
    /// now" / "5m ago" / "2h ago" without us shipping a timezone-aware
    /// timestamp through the IPC boundary.
    pub age_secs: u64,
}

/// Cap on the PNG payload we'll inline into the panel as a data URL. Big
/// screenshots get rendered as a placeholder card with size info — paste
/// still works because we hold the full PNG server-side.
const IMAGE_PREVIEW_BYTES: usize = 750_000;

#[tauri::command]
pub fn clipboard_history() -> Vec<ClipboardItem> {
    use base64::Engine;
    let entries = clip::history();
    crate::glog!("clipboard_history called, returning {} entries", entries.len());
    entries.into_iter().map(|e| {
        let item = match e.kind {
            clip::ClipKind::Text(text) => ClipboardItemKind::Text { text },
            clip::ClipKind::Image(img) => {
                let byte_size = img.png.len();
                let data_url = if byte_size <= IMAGE_PREVIEW_BYTES {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&img.png);
                    Some(format!("data:image/png;base64,{b64}"))
                } else {
                    None
                };
                ClipboardItemKind::Image {
                    width: img.width,
                    height: img.height,
                    data_url,
                    byte_size,
                }
            }
        };
        ClipboardItem {
            id: e.id,
            item,
            age_secs: e.at.elapsed().as_secs(),
        }
    }).collect()
}

/// Track which HWND was foreground right before we showed the clipboard
/// panel. After the user picks an entry we restore focus there and synth
/// Ctrl+V — same flow as Windows' own Win+V.
fn pre_clipboard_fg() -> &'static Mutex<isize> {
    static SLOT: OnceLock<Mutex<isize>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(0))
}

#[tauri::command]
pub fn show_clipboard(app: AppHandle) -> Result<(), String> {
    crate::glog!("show_clipboard invoked");
    // Toggle on repeat press — same rationale as show_power_menu. Without
    // this a rapid Win+V double-tap re-opens (and re-positions, and
    // re-focuses) the panel, which can race the OS's own Win+V detector.
    if let Some(win) = app.get_webview_window("clipboard") {
        if win.is_visible().unwrap_or(false) {
            crate::glog!("show_clipboard: panel already visible, hiding");
            return hide_clipboard(app);
        }
    }

    // Capture the working-window HWND BEFORE the panel takes focus, so we
    // can paste back into it after the user picks an entry.
    *pre_clipboard_fg().lock().unwrap() = win32::foreground_hwnd();

    let win = app.get_webview_window("clipboard")
        .ok_or_else(|| "clipboard window missing".to_string())?;
    let monitor = win.current_monitor().ok().flatten()
        .ok_or_else(|| "no current monitor".to_string())?;
    let mon_w = monitor.size().width as i32;
    let mon_h = monitor.size().height as i32;
    let scale = monitor.scale_factor();
    // Same footprint as the spotlight launcher — it's the closest sibling
    // and the consistency makes the panel feel like part of the family.
    let w = (440.0 * scale).round() as i32;
    let h = (520.0 * scale).round() as i32;
    let x = (mon_w - w) / 2;
    // Slightly higher than spotlight (1/3.5) — clipboard panels are
    // taller, and starting higher keeps the panel from clipping the dock.
    let y = (mon_h as f64 / 5.0) as i32;
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
    // Broadcast emit + JS-eval bypass. The event is for surfaces that
    // happen to listen, but the v0.1.13/14 logs proved the named
    // listener AND the focus-gain handler both fail to fire on the
    // user's machine — the panel becomes visible but never refreshes,
    // which is the "Win+V opens an empty panel" symptom. Calling eval
    // on the webview pokes the panel's global refresh function
    // directly: no event bus, no focus-event race, no Tauri 2 weird
    // emit semantics. JS exposes __glassbarClipboardRefresh on module
    // load. We schedule on the next macrotask via setTimeout(0) so the
    // call lands after Tauri's own show-pipeline finishes initialising
    // the webview state.
    let _ = app.emit("clipboard:show", ());
    let _ = win.eval(
        "setTimeout(() => { \
            try { window.__glassbarClipboardRefresh && window.__glassbarClipboardRefresh(); } \
            catch (e) { console.error('clipboard refresh eval failed', e); } \
        }, 0);"
    );
    crate::glog!("show_clipboard: clipboard:show emitted + eval-refresh dispatched");
    Ok(())
}

#[tauri::command]
pub fn hide_clipboard(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("clipboard") {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Set the clipboard to the entry identified by `id`, hide the panel,
/// restore focus to the window that had it before the panel opened, and
/// synthesise Ctrl+V so the content lands wherever the user was actively
/// typing. Dispatches on entry kind — text vs image — at the copy step.
#[tauri::command]
pub fn clipboard_use_entry(app: AppHandle, id: u64) -> Result<(), String> {
    use std::time::Duration;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
        KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_CONTROL,
    };

    crate::glog!("clipboard_use_entry id={id}");
    let entry = clip::find(id).ok_or_else(|| {
        crate::glog!("clipboard_use_entry: id {id} not found");
        "clipboard entry not found".to_string()
    })?;

    match &entry.kind {
        clip::ClipKind::Text(text) => {
            app_actions::copy_to_clipboard(text).map_err(|e| e.to_string())?;
        }
        clip::ClipKind::Image(img) => {
            app_actions::copy_image_to_clipboard(img.width, img.height, &img.png)
                .map_err(|e| e.to_string())?;
        }
    }
    clip::note_self_write();

    if let Some(win) = app.get_webview_window("clipboard") {
        let _ = win.hide();
    }

    let target = *pre_clipboard_fg().lock().unwrap();
    // Restore focus on a delay so the panel's hide() has fully relinquished
    // — Windows races SetForegroundWindow against our window's WM_KILLFOCUS
    // otherwise. 120ms is comfortably past the racing window on the slow
    // Win11 builds I've seen and still feels instant to the user.
    //
    // focus_aggressive (AttachThreadInput) — not focus — because raw
    // SetForegroundWindow is blocked by Win11's foreground-window
    // restrictions when called from a process that doesn't already own
    // the foreground.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(120));
        if target != 0 {
            let _ = win32::focus_aggressive(target);
            // Tiny extra beat so the focus-change event has time to land
            // in the target's window proc before SendInput fires keys.
            std::thread::sleep(Duration::from_millis(30));
        }

        // Use scancode for V — robust against keyboard layouts that
        // remap virtual keys (Dvorak, AZERTY, etc.). Ctrl is the same
        // VK on every layout so we leave it as-is.
        unsafe {
            let v_scan = MapVirtualKeyW(0x56, MAPVK_VK_TO_VSC) as u16;
            let inputs = [
                kb_vk(VK_CONTROL.0, false),
                kb_scan(v_scan, false),
                kb_scan(v_scan, true),
                kb_vk(VK_CONTROL.0, true),
            ];
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }

        unsafe fn kb_vk(vk: u16, key_up: bool) -> INPUT {
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(vk),
                        wScan: 0,
                        dwFlags: if key_up { KEYEVENTF_KEYUP } else { Default::default() },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        }
        unsafe fn kb_scan(scan: u16, key_up: bool) -> INPUT {
            let mut flags = KEYEVENTF_SCANCODE;
            if key_up { flags |= KEYEVENTF_KEYUP; }
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: scan,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        }
    });

    Ok(())
}

/// Clear the in-memory clipboard history. Hooked up to the Clear button
/// in the panel's footer.
#[tauri::command]
pub fn clipboard_clear() {
    clip::clear();
}
