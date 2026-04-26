use tauri::{App, WebviewUrl, WebviewWindowBuilder};
use window_vibrancy::{apply_acrylic, apply_blur, apply_mica};
use anyhow::Result;

use crate::dwm;

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
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .build()?;
    apply_glass(&dock);

    let hud_w = 280.0;
    let hud_h = 380.0;
    let settings = crate::config::load_settings().unwrap_or_default();
    let (hud_x, hud_y) = settings.hud_position
        .unwrap_or((screen_w - hud_w - 12.0, 12.0));
    let hud = WebviewWindowBuilder::new(app, "hud", WebviewUrl::App("hud/index.html".into()))
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
    apply_glass(&hud);

    Ok(())
}

fn apply_glass(window: &tauri::WebviewWindow) {
    // transparent(true) is required so WebView2 doesn't paint an opaque
    // background on top of the Mica effect. With it set, Mica blurs the
    // desktop wallpaper at the window's screen position. Auto theme so
    // the tint matches the user's light/dark mode preference.
    let e_mica = apply_mica(window, None).err();
    let e_acrylic = if e_mica.is_some() {
        apply_acrylic(window, Some((20, 20, 28, 180))).err()
    } else {
        None
    };
    let e_blur = if e_mica.is_some() && e_acrylic.is_some() {
        apply_blur(window, Some((20, 20, 28, 180))).err()
    } else {
        None
    };
    if e_mica.is_some() && e_acrylic.is_some() && e_blur.is_some() {
        tracing::warn!(?e_mica, ?e_acrylic, ?e_blur, "no glass effect available");
    }

    if let Ok(hwnd) = window.hwnd() {
        dwm::round_corners(hwnd.0 as isize);
    }
}
