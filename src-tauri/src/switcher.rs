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

/// Position of `anchor` (the foreground window's hwnd) within `ids`, used to
/// rotate the window list so the foreground window leads at index 0. None if
/// the anchor isn't an eligible window — the caller then can't assume a
/// "previous window" and falls back to selecting index 0.
pub fn anchor_rotation(ids: &[i64], anchor: i64) -> Option<usize> {
    ids.iter().position(|&id| id == anchor)
}

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
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
    anchor: isize, // foreground window when the session opened (for Esc-cancel)
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// Monotonic session counter. Bumped on every open so a background thumbnail
/// thread from a superseded session can detect it's stale and stop emitting.
static SWITCHER_GEN: AtomicU64 = AtomicU64::new(0);

const THUMB_MAX_PX: u32 = 1280;
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
            let new = {
                let mut guard = SESSION.lock().unwrap();
                match guard.as_mut() {
                    Some(s) if !s.items.is_empty() => {
                        s.index = step_index(s.index, step, s.items.len());
                        Some(s.index)
                    }
                    _ => None,
                }
            };
            if let Some(i) = new {
                let _ = app.emit_to("switcher", "switcher:select", i);
            }
        }

        if keyhook::take_switcher_cancel() {
            let anchor = SESSION.lock().unwrap().as_ref().map(|s| s.anchor);
            commands::hide_switcher_overlay(&app);
            *SESSION.lock().unwrap() = None;
            keyhook::reset_switcher_session();
            // Return focus to where the user was before opening the ring.
            if let Some(a) = anchor {
                focus_after_delay(a);
            }
        }

        if keyhook::take_switcher_commit() {
            commit_current(&app);
        }

        // Watchdog: a session is open but Alt is no longer physically held — the
        // hook missed the Alt-up (e.g. an elevated foreground swallowed it).
        // Commit the current selection and tear down so Tab/Esc stop being
        // swallowed globally.
        let stuck = SESSION.lock().unwrap().is_some() && !win32::is_alt_down();
        if stuck {
            commit_current(&app);
        }
    }
}

/// Mouse hover/scroll sync: set the authoritative selection index and echo it
/// back via switcher:select so the UI renders exactly what Rust holds — Rust
/// is the single source of truth, eliminating mouse/keyboard select races.
pub fn set_index(app: &AppHandle, index: usize) {
    let new = {
        let mut guard = SESSION.lock().unwrap();
        match guard.as_mut() {
            Some(s) if index < s.items.len() => {
                s.index = index;
                Some(index)
            }
            _ => None,
        }
    };
    if let Some(i) = new {
        let _ = app.emit_to("switcher", "switcher:select", i);
    }
}

/// Mouse click commit: select `index`, then commit it.
pub fn commit_index(app: &AppHandle, index: usize) {
    {
        let mut guard = SESSION.lock().unwrap();
        if let Some(s) = guard.as_mut() {
            if index < s.items.len() {
                s.index = index;
            }
        }
    }
    commit_current(app);
}

/// Commit the session's current selection: hide the overlay, clear the session
/// + hook flag, and foreground the chosen window. Shared by the Alt-release
/// commit, the stuck-session watchdog, and mouse-click commit.
fn commit_current(app: &AppHandle) {
    let target = {
        let guard = SESSION.lock().unwrap();
        guard.as_ref().and_then(|s| s.items.get(s.index)).map(|it| it.id as isize)
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
        win32::force_foreground(hwnd);
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

    let mut items: Vec<SwitcherItem> = windows
        .into_iter()
        .map(|w| SwitcherItem {
            id: w.hwnd as i64,
            title: w.title,
            minimized: win32::is_iconic(w.hwnd),
            exe_path: w.exe_path,
        })
        .collect();

    // Rotate the foreground window to index 0 so pre-select index 1 lands on
    // the genuinely previous window. EnumWindows is Z-ordered, but the anchor
    // isn't guaranteed first once filtering drops windows. If the anchor was
    // filtered out, we can't assume a "previous" — select the top window.
    let ids: Vec<i64> = items.iter().map(|it| it.id).collect();
    let index = match anchor_rotation(&ids, anchor as i64) {
        Some(pos) => {
            items.rotate_left(pos);
            initial_index(dir, items.len())
        }
        None => 0,
    };

    let generation = SWITCHER_GEN.fetch_add(1, Ordering::SeqCst) + 1;

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

    *SESSION.lock().unwrap() = Some(Session { items: items.clone(), index, anchor });

    // Stream thumbnails in the background so open feels instant. Abandon if a
    // newer session supersedes this one (stale thumbs would paint the wrong ring).
    let app2 = app.clone();
    std::thread::spawn(move || {
        for it in items {
            if SWITCHER_GEN.load(Ordering::SeqCst) != generation {
                return;
            }
            if let Ok(url) = capture::window_thumbnail_data_url(it.id as isize, THUMB_MAX_PX) {
                if SWITCHER_GEN.load(Ordering::SeqCst) != generation {
                    return;
                }
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

    #[test]
    fn anchor_rotation_finds_foreground() {
        let ids = [100, 200, 300];
        assert_eq!(anchor_rotation(&ids, 100), Some(0));
        assert_eq!(anchor_rotation(&ids, 300), Some(2));
    }

    #[test]
    fn anchor_rotation_absent_anchor_is_none() {
        let ids = [100, 200, 300];
        assert_eq!(anchor_rotation(&ids, 999), None);
    }

    #[test]
    fn rotate_then_preselect_picks_window_after_foreground() {
        // Foreground at index 2; after rotate_left(2) it leads, and forward
        // pre-select (index 1) is the window that was right after it in Z-order.
        let mut ids = vec![10, 20, 30, 40];
        let pos = anchor_rotation(&ids, 30).unwrap();
        ids.rotate_left(pos);
        assert_eq!(ids, vec![30, 40, 10, 20]);
        assert_eq!(initial_index(1, ids.len()), 1); // -> id 40, the previous window
    }
}
