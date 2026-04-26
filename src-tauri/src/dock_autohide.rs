use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, PhysicalPosition};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::dwm;

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
    let mut last_in_zone = Instant::now();
    let dock_hwnd = window.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    let hud_hwnd = app.get_webview_window("hud")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
        .unwrap_or(0);
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
        } else if visible && last_in_zone.elapsed().as_millis() > HIDE_AFTER_MS {
            visible = false;
            start_anim(&mut target_y, &mut anim_from_y, &mut anim_start, current_y, hidden_y);
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

        // Periodically re-assert topmost so the dock + HUD stay above
        // borderless-fullscreen apps and F11'd browsers.
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
