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

    let dock_w = 900.0;
    let dock_h = 64.0;
    let dock = WebviewWindowBuilder::new(app, "dock", WebviewUrl::App("dock/index.html".into()))
        .title("")
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
    apply_noactivate(&dock);
    clip_to_rounded(&dock, 22.0, scale);

    let hud_w = 280.0;
    let hud_h = 500.0;
    let settings = crate::config::load_settings().unwrap_or_default();
    let (hud_x, hud_y) = settings.hud_position
        .unwrap_or((screen_w - hud_w - 12.0, 12.0));
    let hud = WebviewWindowBuilder::new(app, "hud", WebviewUrl::App("hud/index.html".into()))
        .title("")
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
    apply_noactivate(&hud);
    clip_to_rounded(&hud, 22.0, scale);

    // Spotlight launcher window. Created hidden + centered; show_spotlight
    // repositions and shows it when the Ctrl+Alt+Space hotkey fires.
    let spotlight = WebviewWindowBuilder::new(app, "spotlight", WebviewUrl::App("spotlight/index.html".into()))
        .title("")
        .inner_size(560.0, 440.0)
        .position((screen_w - 560.0) / 2.0, screen_h / 4.0)
        .decorations(false)
        .transparent(true)
        .background_color(Color(0, 0, 0, 0))
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .visible(false)
        .build()?;
    force_webview_transparent(&spotlight);
    apply_glass(&spotlight);
    // Like the right-click menu, spotlight needs focus for keyboard input;
    // do NOT mark it no-activate.

    // Pre-create the right-click context menu window. Hidden until something
    // calls `show_menu`. Fixed initial size; show_menu re-sizes per content.
    let menu = WebviewWindowBuilder::new(app, "menu", WebviewUrl::App("menu/index.html".into()))
        .title("")
        .inner_size(240.0, 400.0)
        .position(0.0, 0.0)
        .decorations(false)
        .transparent(true)
        .background_color(Color(0, 0, 0, 0))
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .visible(false)
        .build()?;
    force_webview_transparent(&menu);
    apply_glass(&menu);
    // Intentionally NOT calling apply_noactivate on the menu — a menu must
    // be able to take focus so its onFocusChanged listener fires (used to
    // pull items + auto-dismiss on blur) and so Escape keydown reaches it.
    // The dock/HUD stay no-activate because clicking them shouldn't steal
    // focus from the user's working window.

    // Don't apply rounded region here — show_menu re-sizes the window per use,
    // and SetWindowRgn is one-shot per dimensions. CSS border-radius on the
    // menu card handles visual rounding instead.

    Ok(())
}

fn clip_to_rounded(window: &tauri::WebviewWindow, radius_logical: f64, scale: f64) {
    let Ok(hwnd) = window.hwnd() else { return; };
    let Ok(size) = window.outer_size() else { return; };
    let radius_px = (radius_logical * scale).round() as i32;
    dwm::apply_rounded_region(hwnd.0 as isize, size.width as i32, size.height as i32, radius_px);
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
    // per-pixel-alpha capability.
    dwm::make_layered_with_alpha(h, 160);
    dwm::strip_decorations(h);
    dwm::suppress_nc_rendering(h);
    dwm::round_corners(h);
}

/// Add WS_EX_NOACTIVATE so clicking the window doesn't steal focus from the
/// user's working window. Applied to dock + HUD, NOT to the menu (which
/// needs focus to handle Escape and auto-dismiss).
fn apply_noactivate(window: &tauri::WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else { return; };
    dwm::make_noactivate(hwnd.0 as isize);
}
