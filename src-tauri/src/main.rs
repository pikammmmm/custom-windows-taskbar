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
        ])
        .setup(|app| {
            let pinned_path = config::pinned_path()?;
            let initial = pinned::load_from(&pinned_path).unwrap_or_default();
            let pinned_state: pinned::PinnedHandle = Arc::new(Mutex::new(initial));

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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
