#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};
use tauri::{Manager, Emitter};

mod windows_setup;
mod pinned;
mod config;
mod win32;
mod app_tracker;
mod icons;
mod commands;
mod widgets;
mod widget_state;
mod autostart;
mod dwm;
mod import_pinned;
mod shell_taskbar;
mod dock_autohide;
mod keyhook;
mod app_actions;
mod stash;
mod wndproc;
mod logger;

/// Watch the Windows-taskbar pin folder and merge *newly* pinned entries
/// into our pinned.json. Critical: only items the user has pinned to the
/// taskbar SINCE the last seen sync are added — anything we've already
/// imported once stays out of the way, so unpinning a dock icon doesn't
/// get undone the next tick. The set of previously-imported paths is
/// persisted to imported_taskbar.json so it also survives restarts.
fn sync_taskbar_pins_loop(
    tb_dir: std::path::PathBuf,
    pinned_path: std::path::PathBuf,
    pinned_state: pinned::PinnedHandle,
) {
    use std::collections::HashSet;
    use std::time::Duration;

    let imported_path = match config::imported_taskbar_path() {
        Ok(p) => p,
        Err(e) => { tracing::warn!("imported_taskbar_path failed: {e}"); return; }
    };
    // Initialise from disk so a fresh launch doesn't re-pin everything we
    // already imported on the last run.
    let mut imported: HashSet<String> = std::fs::read_to_string(&imported_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.into_iter().map(|p| p.to_lowercase()).collect())
        .unwrap_or_default();

    let mut last_count: usize = 0;
    loop {
        std::thread::sleep(Duration::from_secs(5));
        if !tb_dir.is_dir() { continue; }
        let count = std::fs::read_dir(&tb_dir)
            .map(|rd| rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension()
                    .and_then(|x| x.to_str())
                    .map(|s| s.eq_ignore_ascii_case("lnk"))
                    == Some(true))
                .count())
            .unwrap_or(0);
        if count == last_count { continue; }
        last_count = count;

        let Ok(taskbar_pins) = import_pinned::read_taskbar_pins() else { continue };
        let mut guard = pinned_state.lock().unwrap();
        let mut changed = false;
        let mut imported_changed = false;
        for tp in taskbar_pins {
            let key = tp.path.to_lowercase();
            // Already imported once → respect any later unpin from the dock.
            if imported.contains(&key) { continue; }
            imported.insert(key);
            imported_changed = true;
            // Don't double-add if it's somehow already in the dock list.
            if guard.iter().any(|p| p.path.eq_ignore_ascii_case(&tp.path)) {
                continue;
            }
            guard.push(tp);
            changed = true;
        }
        if changed {
            if let Err(e) = pinned::save_to(&pinned_path, &guard) {
                tracing::warn!("save pinned.json after taskbar sync failed: {e}");
            }
        }
        if imported_changed {
            let list: Vec<String> = imported.iter().cloned().collect();
            if let Err(e) = std::fs::write(&imported_path,
                serde_json::to_string_pretty(&list).unwrap_or("[]".into())) {
                tracing::warn!("save imported_taskbar.json failed: {e}");
            }
        }
    }
}

/// Bail out immediately if another glassbar.exe is already running. Uses a
/// named kernel mutex so the lock is reliable across processes (cargo
/// builds + autostart + manual relaunch can otherwise pile up instances).
/// Returns false if we should exit without starting.
fn acquire_singleton() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Local\\glassbar-singleton-7c3e".encode_utf16()
        .chain(std::iter::once(0)).collect();
    unsafe {
        let handle = CreateMutexW(None, false, PCWSTR(name.as_ptr()));
        // Intentionally leak the handle — we want the OS to release it on
        // process exit so a second instance can take over after a crash.
        if handle.is_err() { return true; } // can't tell either way; allow start
        let already = GetLastError() == ERROR_ALREADY_EXISTS;
        std::mem::forget(handle);
        !already
    }
}

fn main() {
    if !acquire_singleton() {
        // Another instance is already running — exit silently without trying
        // to start the Tauri runtime or its windows.
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // File logger lives at %APPDATA%\glassbar\debug.log. The init banner
    // gives every session a clear "==== start ====" line so a user-shared
    // log isn't ambiguous about which run produced what.
    logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_drag::init())
        .invoke_handler(tauri::generate_handler![
            commands::launch,
            commands::launch_uri,
            commands::focus_window,
            commands::minimize_window,
            commands::close_window,
            commands::foreground_hwnd,
            commands::get_pinned,
            commands::get_icon,
            commands::pin_app,
            commands::pin_dropped,
            commands::set_pinned_order,
            commands::recent_files,
            commands::stash_list,
            commands::stash_add,
            commands::stash_remove,
            commands::stash_clear,
            commands::search_apps,
            commands::show_spotlight,
            commands::hide_spotlight,
            commands::unpin_app,
            commands::set_hud_position,
            commands::geocode_city,
            commands::set_weather_city,
            commands::get_weather_city,
            commands::set_autostart,
            commands::get_autostart,
            commands::set_volume,
            commands::get_settings_volume,
            commands::audio_diagnostics,
            commands::set_mute,
            commands::list_audio_devices,
            commands::set_default_audio_device,
            commands::warp_toggle,
            commands::power_action,
            commands::media_toggle_play,
            commands::media_next,
            commands::media_prev,
            commands::open_start_menu,
            commands::minimize_all_windows,
            commands::hide_windows_taskbar,
            commands::show_windows_taskbar,
            commands::toggle_hud,
            commands::close_hwnds,
            commands::app_info,
            commands::show_in_explorer,
            commands::run_as_admin,
            commands::show_properties,
            commands::copy_to_clipboard,
            commands::show_menu,
            commands::get_menu_items,
            commands::hide_menu,
            commands::show_power_menu,
            commands::clipboard_history,
            commands::show_clipboard,
            commands::hide_clipboard,
            commands::clipboard_use_entry,
            commands::clipboard_clear,
        ])
        .setup(|app| {
            let pinned_path = config::pinned_path()?;
            // First-run migration: if pinned.json doesn't exist yet, seed it
            // from the user's existing Windows-taskbar pins. The same paths
            // also seed imported_taskbar.json so the live sync respects any
            // later unpin from the dock instead of re-adding them.
            if !pinned_path.exists() {
                match import_pinned::read_taskbar_pins() {
                    Ok(seed) if !seed.is_empty() => {
                        if let Err(e) = pinned::save_to(&pinned_path, &seed) {
                            tracing::warn!("seed pinned.json failed: {e}");
                        } else {
                            tracing::info!("seeded {} pinned apps from Windows taskbar", seed.len());
                        }
                        if let Ok(imp_path) = config::imported_taskbar_path() {
                            let lc: Vec<String> = seed.iter()
                                .map(|p| p.path.to_lowercase()).collect();
                            let _ = std::fs::write(&imp_path,
                                serde_json::to_string_pretty(&lc).unwrap_or("[]".into()));
                        }
                    }
                    Ok(_) => tracing::info!("no Windows-taskbar pins to import"),
                    Err(e) => tracing::warn!("import_pinned failed: {e}"),
                }
            }
            let initial = pinned::load_from(&pinned_path).unwrap_or_default();
            let pinned_state: pinned::PinnedHandle = Arc::new(Mutex::new(initial));

            let settings = config::load_settings().unwrap_or_default();
            if settings.auto_start && !autostart::is_enabled() {
                let _ = autostart::enable();
            } else if !settings.auto_start && autostart::is_enabled() {
                let _ = autostart::disable();
            }

            let pinned_clone = pinned_state.clone();
            let app_handle = app.handle().clone();
            let _watcher = pinned::watch(pinned_path.clone(), move |apps| {
                *pinned_clone.lock().unwrap() = apps.clone();
                let _ = app_handle.emit("pinned:changed", apps);
            })?;
            std::mem::forget(_watcher); // keep watcher alive for app lifetime

            // Live sync from the Windows taskbar pin folder. Anything the
            // user pins to the OS taskbar shows up on the dock too — without
            // this we only import once at first run. We additively merge:
            // new TaskBar pins get added, but pins that exist only on the
            // dock (drag-to-pin) stay put.
            let taskbar_pinned_path = pinned_path.clone();
            let taskbar_pinned_state = pinned_state.clone();
            if let Some(tb_dir) = import_pinned::taskbar_pin_dir() {
                std::thread::spawn(move || sync_taskbar_pins_loop(
                    tb_dir, taskbar_pinned_path, taskbar_pinned_state));
            }

            app.manage(pinned_state);

            // Load the file stash from disk and register as Tauri-managed state
            // so commands can pick it up via `State<'_, stash::StashHandle>`.
            let stash_initial = stash::load().unwrap_or_default();
            let stash_state: stash::StashHandle = Arc::new(Mutex::new(stash_initial));
            app.manage(stash_state);

            windows_setup::create_windows(app)?;
            app_tracker::spawn_poller(app.handle().clone(), std::time::Duration::from_millis(500));
            // 400ms tick — fast enough that a volume nudge or a network
            // status change shows up almost immediately on the dock and
            // HUD. The min_gap inside widget_state still throttles emits
            // when nothing actually changed, so the IPC isn't busy.
            widget_state::spawn(app.handle().clone(), std::time::Duration::from_millis(400));

            // Hide the original Windows taskbar so glassbar owns the strip.
            // Re-asserted periodically because shell restarts (explorer crash,
            // multi-monitor changes) can re-show it.
            let _ = shell_taskbar::hide_windows_taskbar();
            std::thread::spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let _ = shell_taskbar::hide_windows_taskbar();
            });

            // Auto-hide dock + keep dock/HUD pinned above fullscreen apps.
            dock_autohide::spawn(app.handle().clone());

            // Low-level keyboard hook so Win-key tap toggles the dock
            // (chord support preserved — Win+R, Win+E, etc still work).
            keyhook::spawn();

            // Spotlight indexes — both run on their own background threads
            // and refresh on a slow loop so newly installed apps / saved
            // files show up without a glassbar restart.
            widgets::start_menu::spawn();
            widgets::files::spawn();

            // Win+V clipboard history — polls CF_UNICODETEXT every ~700ms
            // and keeps a 25-entry in-memory ring buffer. Memory-only by
            // design (see widgets/clipboard.rs for rationale).
            widgets::clipboard::spawn();

            // Re-strip decorations after Tauri's late init has settled — on
            // some Win11 builds the framework re-applies WS_CAPTION between
            // window build and first paint, so stripping once at build time
            // isn't enough.
            let app_for_strip = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(800));
                for label in ["dock", "hud"] {
                    if let Some(win) = app_for_strip.get_webview_window(label) {
                        if let Ok(hwnd) = win.hwnd() {
                            dwm::strip_decorations(hwnd.0 as isize);
                        }
                    }
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app, event| {
            // Restore the Windows taskbar on graceful exit so the user is not
            // left without a working shell if they uninstall / quit glassbar.
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                let _ = shell_taskbar::show_windows_taskbar();
            }
        });
}
