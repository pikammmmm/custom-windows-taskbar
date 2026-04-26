use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::{commands, dwm, keyhook, win32};

/// Don't auto-dismiss the menu in the brief moment right after show — gives
/// it time to take focus and avoids racing the very click that opened it.
const MENU_DISMISS_GRACE_MS: u128 = 250;

const POLL_MS: u64 = 16;          // ~60 Hz so position interpolation looks smooth
const HIDE_AFTER_MS: u128 = 1500;
const TRIGGER_PX: i32 = 4;
const SLIDE_MS: f64 = 260.0;      // length of the show/hide slide
const TOPMOST_REASSERT_MS: u128 = 700;

pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || run(app));
}

fn run(app: AppHandle) {
    // Give Tauri a moment to settle the dock window's position.
    std::thread::sleep(Duration::from_millis(500));
    let Some(window) = app.get_webview_window("dock") else { return };
    let Ok(initial_pos) = window.outer_position() else { return };
    let Ok(size) = window.outer_size() else { return };
    let Ok(Some(monitor)) = window.current_monitor() else { return };

    let screen_h = monitor.size().height as i32;
    let dock_left = initial_pos.x;
    let dock_right = dock_left + size.width as i32;
    let shown_y = initial_pos.y;
    let dock_h = size.height as i32;
    // Slide the window fully off-screen — global cursor polling will pull
    // it back when the user reaches the bottom-of-screen trigger zone.
    let hidden_y = screen_h;

    let mut visible = true;
    // True between Win-tap-to-show and the next Win-tap. While set, cursor-based
    // auto-hide is suppressed so the dock stays put — matches the user's mental
    // model of "tap Win to show, tap again to hide."
    let mut win_pinned = false;
    let mut last_in_zone = Instant::now();
    let dock_hwnd = window.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    let hud_hwnd = app.get_webview_window("hud")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
        .unwrap_or(0);
    let menu_window = app.get_webview_window("menu");
    let menu_hwnd = menu_window.as_ref()
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
        .unwrap_or(0);
    // True once we've observed the menu as the foreground window since the
    // most recent `show_menu`. We only allow dismiss after this flips —
    // otherwise a menu that fails to activate would auto-hide instantly
    // when the grace window expires.
    let mut menu_was_foreground = false;
    let mut last_menu_shown_seen: Option<Instant> = None;
    let mut topmost_tick = Instant::now();

    let mut current_y = shown_y;
    let mut target_y = shown_y;
    let mut anim_from_y = shown_y;
    let mut anim_start: Option<Instant> = None;

    loop {
        std::thread::sleep(Duration::from_millis(POLL_MS));
        let mut p = POINT { x: 0, y: 0 };
        unsafe {
            if GetCursorPos(&mut p).is_err() { continue; }
        }

        let in_trigger = p.y >= screen_h - TRIGGER_PX
            && p.x >= dock_left
            && p.x <= dock_right;
        // While shown, keep the cursor "active" if it sits anywhere inside the
        // dock box. Use current_y so we don't mistakenly poll against the
        // shown position while the dock is mid-slide-down.
        let in_dock = visible
            && p.y >= current_y
            && p.y <= current_y + dock_h
            && p.x >= dock_left
            && p.x <= dock_right;

        if in_trigger || in_dock {
            last_in_zone = Instant::now();
            if !visible {
                visible = true;
                start_anim(&mut target_y, &mut anim_from_y, &mut anim_start, current_y, shown_y);
            }
        } else if visible && !win_pinned && last_in_zone.elapsed().as_millis() > HIDE_AFTER_MS {
            visible = false;
            start_anim(&mut target_y, &mut anim_from_y, &mut anim_start, current_y, hidden_y);
        }

        // Ctrl+Alt+Space — show the spotlight launcher.
        if keyhook::take_spotlight_request() {
            let _ = app.emit("spotlight:hotkey", ());
            // Inline call to show_spotlight via the same AppHandle.
            if let Some(win) = app.get_webview_window("spotlight") {
                if let Ok(Some(monitor)) = win.current_monitor() {
                    let mw = monitor.size().width as i32;
                    let mh = monitor.size().height as i32;
                    let scale = monitor.scale_factor();
                    let w = (560.0 * scale).round() as i32;
                    let h = (440.0 * scale).round() as i32;
                    let x = (mw - w) / 2;
                    let y = (mh as f64 / 3.5) as i32;
                    let _ = win.set_size(tauri::PhysicalSize::new(w as u32, h as u32));
                    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
                    let _ = win.show();
                    let _ = win.set_always_on_top(true);
                    let _ = win.set_focus();
                    let _ = app.emit_to("spotlight", "spotlight:show", ());
                }
            }
        }

        // Win-key tap toggles visibility regardless of cursor position.
        if keyhook::take_toggle_request() {
            if visible {
                visible = false;
                win_pinned = false;
                start_anim(&mut target_y, &mut anim_from_y, &mut anim_start, current_y, hidden_y);
            } else {
                visible = true;
                win_pinned = true;
                last_in_zone = Instant::now();
                start_anim(&mut target_y, &mut anim_from_y, &mut anim_start, current_y, shown_y);
            }
        }

        // Drive the slide animation if one is in progress.
        if let Some(started) = anim_start {
            let t = (started.elapsed().as_secs_f64() * 1000.0 / SLIDE_MS).min(1.0);
            let eased = ease_out_cubic(t);
            let new_y = lerp(anim_from_y, target_y, eased);
            if new_y != current_y {
                current_y = new_y;
                let _ = window.set_position(PhysicalPosition { x: dock_left, y: current_y });
            }
            if t >= 1.0 {
                anim_start = None;
            }
        }

        // Auto-dismiss the right-click menu when the user clicks away.
        // Focus events from the menu's WebView2 don't fire reliably, so we
        // poll the foreground window instead.
        //
        // Algorithm:
        //   - When show_menu records a fresh timestamp, reset our tracking.
        //   - While menu is open, watch the foreground HWND.
        //   - First time fg == menu_hwnd, latch `menu_was_foreground`.
        //   - Once latched and grace expired, fg != menu_hwnd means the user
        //     clicked something else — hide the menu.
        // The latch guards against the case where the menu fails to activate
        // (we'd otherwise dismiss it the instant the grace window ended).
        if menu_hwnd != 0 {
            if let Some(shown) = commands::last_menu_shown_at() {
                if last_menu_shown_seen != Some(shown) {
                    menu_was_foreground = false;
                    last_menu_shown_seen = Some(shown);
                }
                let fg = win32::foreground_hwnd();
                if fg == menu_hwnd {
                    menu_was_foreground = true;
                } else if menu_was_foreground
                    && shown.elapsed().as_millis() > MENU_DISMISS_GRACE_MS
                {
                    if let Some(menu_win) = &menu_window {
                        let _ = menu_win.hide();
                    }
                    commands::clear_menu_shown_at();
                    last_menu_shown_seen = None;
                }
            } else {
                last_menu_shown_seen = None;
                menu_was_foreground = false;
            }
        }

        // Re-strip decorations EVERY tick — strip_decorations is a cheap no-op
        // when WS_CAPTION isn't set, so polling at 60Hz catches the OS
        // re-asserting it within one frame instead of up to TOPMOST_REASSERT_MS
        // later (which is what caused the random white bar at the top of the HUD).
        if dock_hwnd != 0 { dwm::strip_decorations(dock_hwnd); }
        if hud_hwnd != 0 { dwm::strip_decorations(hud_hwnd); }

        // Re-assert topmost on a slower cadence — this one isn't a no-op so we
        // don't want it 60×/sec.
        if topmost_tick.elapsed().as_millis() >= TOPMOST_REASSERT_MS {
            if dock_hwnd != 0 { dwm::assert_topmost(dock_hwnd); }
            if hud_hwnd != 0 { dwm::assert_topmost(hud_hwnd); }
            topmost_tick = Instant::now();
        }
    }
}

fn start_anim(
    target: &mut i32,
    from: &mut i32,
    start: &mut Option<Instant>,
    current_y: i32,
    new_target: i32,
) {
    *target = new_target;
    *from = current_y;
    *start = Some(Instant::now());
}

fn ease_out_cubic(t: f64) -> f64 {
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

fn lerp(a: i32, b: i32, t: f64) -> i32 {
    a + ((b - a) as f64 * t).round() as i32
}
