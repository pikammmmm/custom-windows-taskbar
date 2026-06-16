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
