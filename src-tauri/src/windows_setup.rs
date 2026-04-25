use tauri::{App, WebviewUrl, WebviewWindowBuilder};
use anyhow::Result;

pub fn create_windows(app: &mut App) -> Result<()> {
    let primary = app.primary_monitor()?
        .ok_or_else(|| anyhow::anyhow!("no primary monitor"))?;
    let size = primary.size();
    let scale = primary.scale_factor();

    let screen_w = size.width as f64 / scale;
    let screen_h = size.height as f64 / scale;

    // Dock: bottom-center, 700x64
    let dock_w = 700.0;
    let dock_h = 64.0;
    let dock_x = (screen_w - dock_w) / 2.0;
    let dock_y = screen_h - dock_h - 12.0;

    WebviewWindowBuilder::new(app, "dock", WebviewUrl::App("dock/index.html".into()))
        .title("glassbar-dock")
        .inner_size(dock_w, dock_h)
        .position(dock_x, dock_y)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .build()?;

    // HUD: top-right corner, 280x220
    let hud_w = 280.0;
    let hud_h = 220.0;
    let hud_x = screen_w - hud_w - 12.0;
    let hud_y = 12.0;

    WebviewWindowBuilder::new(app, "hud", WebviewUrl::App("hud/index.html".into()))
        .title("glassbar-hud")
        .inner_size(hud_w, hud_h)
        .position(hud_x, hud_y)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .build()?;

    Ok(())
}
