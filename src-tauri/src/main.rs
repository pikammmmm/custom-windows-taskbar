#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod windows_setup;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tauri::Builder::default()
        .setup(|app| {
            windows_setup::create_windows(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
