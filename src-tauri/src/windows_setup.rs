use tauri::{App, WebviewUrl, WebviewWindowBuilder};
use window_vibrancy::{apply_mica, apply_acrylic};
use anyhow::Result;

pub fn create_windows(app: &mut App) -> Result<()> {
    let primary = app.primary_monitor()?
        .ok_or_else(|| anyhow::anyhow!("no primary monitor"))?;
    let size = primary.size();
    let scale = primary.scale_factor();
    let screen_w = size.width as f64 / scale;
    let screen_h = size.height as f64 / scale;

    let dock_w = 700.0;
    let dock_h = 64.0;
    let dock = WebviewWindowBuilder::new(app, "dock", WebviewUrl::App("dock/index.html".into()))
        .title("glassbar-dock")
        .inner_size(dock_w, dock_h)
        .position((screen_w - dock_w) / 2.0, screen_h - dock_h - 12.0)
        .decorations(false).transparent(true).always_on_top(true)
        .skip_taskbar(true).resizable(false).shadow(false)
        .build()?;
    apply_blur(&dock);

    let hud_w = 280.0;
    let hud_h = 220.0;
    let settings = crate::config::load_settings().unwrap_or_default();
    let (hud_x, hud_y) = settings.hud_position
        .unwrap_or((screen_w - hud_w - 12.0, 12.0));
    let hud = WebviewWindowBuilder::new(app, "hud", WebviewUrl::App("hud/index.html".into()))
        .title("glassbar-hud")
        .inner_size(hud_w, hud_h)
        .position(hud_x, hud_y)
        .decorations(false).transparent(true).always_on_top(true)
        .skip_taskbar(true).resizable(false).shadow(false)
        .build()?;
    apply_blur(&hud);

    Ok(())
}

fn apply_blur(window: &tauri::WebviewWindow) {
    if let Err(mica_err) = apply_mica(window, Some(true)) {
        if let Err(acrylic_err) = apply_acrylic(window, Some((20, 20, 25, 160))) {
            tracing::warn!(?mica_err, ?acrylic_err, "blur unavailable on this OS");
        }
    }
}
