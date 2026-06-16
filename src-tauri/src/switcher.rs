//! 3D ring Alt+Tab switcher: session state, selection math, and the
//! background loop that turns keyhook intents into webview events + focus.

/// Initial highlighted index when the ring first opens.
/// Forward (Alt+Tab) pre-selects index 1 — the window behind the current
/// foreground — so a quick tap switches to the previous window, exactly
/// like the native switcher. Reverse (Alt+Shift+Tab) pre-selects the last.
/// `len` is the number of windows (caller guarantees len >= 1).
pub fn initial_index(dir: i32, len: usize) -> usize {
    debug_assert!(len >= 1);
    if dir >= 0 {
        if len >= 2 { 1 } else { 0 }
    } else {
        len - 1
    }
}

/// Advance the selection by `delta` slots with wrap-around.
/// `len` is the number of windows (caller guarantees len >= 1).
pub fn step_index(cur: usize, delta: i32, len: usize) -> usize {
    debug_assert!(len >= 1);
    let n = len as i64;
    let mut i = (cur as i64 + delta as i64) % n;
    if i < 0 { i += n; }
    i as usize
}

use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::{capture, commands, keyhook, win32};

/// One window as presented to the ring UI.
#[derive(Debug, Clone, Serialize)]
pub struct SwitcherItem {
    pub id: i64, // hwnd
    pub title: String,
    pub exe_path: String,
    pub minimized: bool,
}

struct Session {
    items: Vec<SwitcherItem>,
    index: usize,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

const THUMB_MAX_PX: u32 = 512;
const POLL_MS: u64 = 8;

/// Spawn the switcher event loop. Mirrors dock_autohide::spawn.
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || run(app));
}

fn run(app: AppHandle) {
    loop {
        std::thread::sleep(Duration::from_millis(POLL_MS));

        let open_dir = keyhook::take_switcher_open();
        if open_dir != 0 {
            open_session(&app, open_dir);
        }

        let step = keyhook::take_switcher_step();
        if step != 0 {
            if let Some(s) = SESSION.lock().unwrap().as_mut() {
                if !s.items.is_empty() {
                    s.index = step_index(s.index, step, s.items.len());
                    let _ = app.emit_to("switcher", "switcher:select", s.index);
                }
            }
        }

        if keyhook::take_switcher_cancel() {
            commands::hide_switcher_overlay(&app);
            *SESSION.lock().unwrap() = None;
        }

        if keyhook::take_switcher_commit() {
            let target = {
                let guard = SESSION.lock().unwrap();
                guard.as_ref().and_then(|s| s.items.get(s.index)).map(|it| it.id as isize)
            };
            commands::hide_switcher_overlay(&app);
            *SESSION.lock().unwrap() = None;
            if let Some(hwnd) = target {
                focus_after_delay(hwnd);
            }
        }
    }
}

/// Mouse hover/scroll sync: set the authoritative selection index.
pub fn set_index(index: usize) {
    if let Some(s) = SESSION.lock().unwrap().as_mut() {
        if index < s.items.len() {
            s.index = index;
        }
    }
}

/// Mouse click commit: focus the window at `index` and tear down the session.
pub fn commit_index(app: &AppHandle, index: usize) {
    let target = {
        let guard = SESSION.lock().unwrap();
        guard.as_ref().and_then(|s| s.items.get(index)).map(|it| it.id as isize)
    };
    commands::hide_switcher_overlay(app);
    *SESSION.lock().unwrap() = None;
    keyhook::reset_switcher_session();
    if let Some(hwnd) = target {
        focus_after_delay(hwnd);
    }
}

/// Foreground `hwnd` after a short delay so our overlay's hide() fully
/// relinquishes first (Win11 races SetForegroundWindow vs WM_KILLFOCUS —
/// same 90ms trick the clipboard paste flow uses).
fn focus_after_delay(hwnd: isize) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(90));
        let _ = win32::focus_aggressive(hwnd);
    });
}

#[derive(Serialize, Clone)]
struct OpenPayload {
    windows: Vec<SwitcherItem>,
    selected: usize,
}

fn open_session(app: &AppHandle, dir: i32) {
    let anchor = win32::foreground_hwnd();
    let windows = match win32::enumerate_windows() {
        Ok(w) => w,
        Err(_) => {
            keyhook::reset_switcher_session();
            return;
        }
    };
    if windows.is_empty() {
        keyhook::reset_switcher_session();
        return;
    }

    let items: Vec<SwitcherItem> = windows
        .into_iter()
        .map(|w| SwitcherItem {
            id: w.hwnd as i64,
            title: w.title,
            minimized: win32::is_iconic(w.hwnd),
            exe_path: w.exe_path,
        })
        .collect();

    let index = initial_index(dir, items.len());

    if commands::show_switcher_overlay(app, anchor).is_err() {
        keyhook::reset_switcher_session();
        return;
    }

    let payload = OpenPayload { windows: items.clone(), selected: index };
    let _ = app.emit_to("switcher", "switcher:open", payload.clone());
    // eval fallback: stash on window in case the listener was mid-registration.
    if let Some(win) = app.get_webview_window("switcher") {
        if let Ok(json) = serde_json::to_string(&payload) {
            let _ = win.eval(&format!(
                "window.__switcherPending = {json}; window.__switcherApply && window.__switcherApply(window.__switcherPending);"
            ));
        }
    }

    *SESSION.lock().unwrap() = Some(Session { items: items.clone(), index });

    // Stream thumbnails in the background so open feels instant.
    let app2 = app.clone();
    std::thread::spawn(move || {
        for it in items {
            if let Ok(url) = capture::window_thumbnail_data_url(it.id as isize, THUMB_MAX_PX) {
                #[derive(Serialize, Clone)]
                struct Thumb {
                    id: i64,
                    thumb: String,
                }
                let _ = app2.emit_to("switcher", "switcher:thumb", Thumb { id: it.id, thumb: url });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_forward_picks_second() {
        assert_eq!(initial_index(1, 5), 1);
    }

    #[test]
    fn initial_forward_single_window_picks_only() {
        assert_eq!(initial_index(1, 1), 0);
    }

    #[test]
    fn initial_reverse_picks_last() {
        assert_eq!(initial_index(-1, 5), 4);
    }

    #[test]
    fn step_wraps_forward() {
        assert_eq!(step_index(4, 1, 5), 0);
    }

    #[test]
    fn step_wraps_backward() {
        assert_eq!(step_index(0, -1, 5), 4);
    }

    #[test]
    fn step_multi_delta() {
        assert_eq!(step_index(0, 3, 5), 3);
        assert_eq!(step_index(1, -3, 5), 3);
    }
}
