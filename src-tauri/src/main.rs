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

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::launch,
            commands::focus_window,
            commands::minimize_window,
            commands::close_window,
            commands::foreground_hwnd,
            commands::get_pinned,
            commands::get_icon,
            commands::pin_app,
            commands::unpin_app,
            commands::set_hud_position,
            commands::set_autostart,
            commands::get_autostart,
            commands::set_volume,
            commands::set_mute,
            commands::open_start_menu,
            commands::hide_windows_taskbar,
            commands::show_windows_taskbar,
            commands::toggle_hud,
            commands::close_hwnds,
        ])
        .setup(|app| {
            let pinned_path = config::pinned_path()?;
            // First-run migration: if pinned.json doesn't exist yet, seed it
            // from the user's existing Windows-taskbar pins.
            if !pinned_path.exists() {
                match import_pinned::read_taskbar_pins() {
                    Ok(seed) if !seed.is_empty() => {
                        if let Err(e) = pinned::save_to(&pinned_path, &seed) {
                            tracing::warn!("seed pinned.json failed: {e}");
                        } else {
                            tracing::info!("seeded {} pinned apps from Windows taskbar", seed.len());
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

            app.manage(pinned_state);
            windows_setup::create_windows(app)?;
            app_tracker::spawn_poller(app.handle().clone(), std::time::Duration::from_millis(500));
            widget_state::spawn(app.handle().clone(), std::time::Duration::from_secs(1));

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
