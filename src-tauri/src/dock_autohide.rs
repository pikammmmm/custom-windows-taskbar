use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, PhysicalPosition};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::dwm;

const POLL_MS: u64 = 80;
const HIDE_AFTER_MS: u128 = 1500;
const TRIGGER_PX: i32 = 4;

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

    loop {
        std::thread::sleep(Duration::from_millis(POLL_MS));
        let mut p = POINT { x: 0, y: 0 };
        unsafe {
            if GetCursorPos(&mut p).is_err() { continue; }
        }

        let in_trigger = p.y >= screen_h - TRIGGER_PX
            && p.x >= dock_left
            && p.x <= dock_right;
        let in_dock = visible
            && p.y >= shown_y
            && p.y <= shown_y + dock_h
            && p.x >= dock_left
            && p.x <= dock_right;

        if in_trigger || in_dock {
            last_in_zone = Instant::now();
            if !visible {
                let _ = window.set_position(PhysicalPosition { x: dock_left, y: shown_y });
                visible = true;
            }
        } else if visible && last_in_zone.elapsed().as_millis() > HIDE_AFTER_MS {
            let _ = window.set_position(PhysicalPosition { x: dock_left, y: hidden_y });
            visible = false;
        }

        // Periodically re-assert topmost so the dock + HUD stay above
        // borderless-fullscreen apps and F11'd browsers.
        if topmost_tick.elapsed() >= Duration::from_millis(700) {
            if dock_hwnd != 0 { dwm::assert_topmost(dock_hwnd); }
            if hud_hwnd != 0 { dwm::assert_topmost(hud_hwnd); }
            topmost_tick = Instant::now();
        }
    }
}
