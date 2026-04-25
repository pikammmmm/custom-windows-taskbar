use serde::Serialize;
use std::collections::BTreeMap;
use crate::win32::WindowInfo;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WindowRef {
    pub hwnd: isize,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunningApp {
    pub exe_path: String,
    pub windows: Vec<WindowRef>,
}

pub fn group(windows: Vec<WindowInfo>) -> Vec<RunningApp> {
    let mut by_exe: BTreeMap<String, Vec<WindowRef>> = BTreeMap::new();
    for w in windows {
        by_exe.entry(w.exe_path).or_default().push(WindowRef { hwnd: w.hwnd, title: w.title });
    }
    by_exe.into_iter()
        .map(|(exe_path, mut windows)| {
            windows.sort_by_key(|w| w.hwnd);
            RunningApp { exe_path, windows }
        })
        .collect()
}

pub fn changed(prev: &[RunningApp], next: &[RunningApp]) -> bool {
    prev != next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(hwnd: isize, title: &str, exe: &str) -> WindowInfo {
        WindowInfo { hwnd, title: title.into(), exe_path: exe.into() }
    }

    #[test]
    fn group_empty_returns_empty() {
        assert_eq!(group(vec![]), vec![]);
    }

    #[test]
    fn group_single_window() {
        let result = group(vec![w(1, "A", "C:\\a.exe")]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].exe_path, "C:\\a.exe");
        assert_eq!(result[0].windows.len(), 1);
    }

    #[test]
    fn group_multiple_windows_same_exe() {
        let result = group(vec![
            w(2, "win2", "C:\\a.exe"),
            w(1, "win1", "C:\\a.exe"),
        ]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].windows.len(), 2);
        assert_eq!(result[0].windows[0].hwnd, 1);
    }

    #[test]
    fn group_separates_different_exes() {
        let result = group(vec![
            w(1, "A", "C:\\a.exe"),
            w(2, "B", "C:\\b.exe"),
        ]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn changed_false_when_identical() {
        let a = group(vec![w(1, "A", "C:\\a.exe")]);
        let b = group(vec![w(1, "A", "C:\\a.exe")]);
        assert!(!changed(&a, &b));
    }

    #[test]
    fn changed_true_when_window_count_differs() {
        let a = group(vec![w(1, "A", "C:\\a.exe")]);
        let b = group(vec![w(1, "A", "C:\\a.exe"), w(2, "A2", "C:\\a.exe")]);
        assert!(changed(&a, &b));
    }

    #[test]
    fn changed_true_when_title_changes() {
        let a = group(vec![w(1, "A", "C:\\a.exe")]);
        let b = group(vec![w(1, "A renamed", "C:\\a.exe")]);
        assert!(changed(&a, &b));
    }
}

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use crate::win32;

pub fn spawn_poller(app: AppHandle, interval: Duration) {
    std::thread::spawn(move || {
        let prev: Arc<Mutex<Vec<RunningApp>>> = Arc::new(Mutex::new(Vec::new()));
        loop {
            std::thread::sleep(interval);
            let windows = match win32::enumerate_windows() {
                Ok(w) => w,
                Err(e) => { tracing::warn!("enumerate failed: {e}"); continue; }
            };
            let next = group(windows);
            let mut guard = prev.lock().unwrap();
            if changed(&guard, &next) {
                *guard = next.clone();
                let _ = app.emit("apps:changed", &next);
            }
        }
    });
}
