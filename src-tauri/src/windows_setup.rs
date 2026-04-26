use tauri::utils::config::Color;
use tauri::{App, WebviewUrl, WebviewWindowBuilder};
use window_vibrancy::{apply_acrylic, apply_blur, apply_mica};
use anyhow::Result;

use crate::dwm;

fn force_webview_transparent(window: &tauri::WebviewWindow) {
    let _ = window.with_webview(|wv| {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
        };
        use windows_core::Interface;
        unsafe {
            let controller = wv.controller();
            if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
                let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                    R: 0, G: 0, B: 0, A: 0,
                });
            }
        }
    });
}

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
        .background_color(Color(0, 0, 0, 0))
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .build()?;
    force_webview_transparent(&dock);
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
        .background_color(Color(0, 0, 0, 0))
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .build()?;
    force_webview_transparent(&hud);
    apply_glass(&hud);

    Ok(())
}

fn apply_glass(window: &tauri::WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else { return; };
    let h = hwnd.0 as isize;

    // Try Win11's modern backdrop first.
    let modern_ok = dwm::set_backdrop(h, dwm::BACKDROP_ACRYLIC);
    if !modern_ok {
        let _ = dwm::set_backdrop(h, dwm::BACKDROP_MICA);
    }

    // Layered window with uniform alpha — guarantees see-through glass even
    // on builds where Tauri's transparent flag doesn't give the window
    // per-pixel-alpha capability. Alpha 240 ≈ 94% opaque keeps icons and
    // text readable while still reading as a glassy panel.
    dwm::make_layered_with_alpha(h, 240);

    dwm::round_corners(h);
}
