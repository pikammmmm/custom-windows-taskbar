#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};
use tauri::{Manager, Emitter};

mod windows_setup;
mod pinned;
mod config;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tauri::Builder::default()
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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
