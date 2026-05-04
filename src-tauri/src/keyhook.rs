use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
    HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

// Bare-bones VK codes for chord detection — windows-rs doesn't expose these
// as named constants the way it does for the modifier keys.
const VK_V: u32 = 0x56;
const VK_X: u32 = 0x58;

static WIN_DOWN: AtomicBool = AtomicBool::new(false);
static CHORD_USED: AtomicBool = AtomicBool::new(false);
static TOGGLE_REQUESTED: AtomicBool = AtomicBool::new(false);
static SPOTLIGHT_REQUESTED: AtomicBool = AtomicBool::new(false);
static CLIPBOARD_REQUESTED: AtomicBool = AtomicBool::new(false);
static POWER_MENU_REQUESTED: AtomicBool = AtomicBool::new(false);

const LLKHF_INJECTED: u32 = 0x10;

/// Returns and clears the pending toggle request — call from the dock
/// auto-hide loop to consume Win-key tap signals.
pub fn take_toggle_request() -> bool {
    TOGGLE_REQUESTED.swap(false, Ordering::AcqRel)
}

/// Returns and clears the pending spotlight request — Ctrl+Alt+Space.
pub fn take_spotlight_request() -> bool {
    SPOTLIGHT_REQUESTED.swap(false, Ordering::AcqRel)
}

/// Returns and clears the pending clipboard-history request — Win+V.
pub fn take_clipboard_request() -> bool {
    CLIPBOARD_REQUESTED.swap(false, Ordering::AcqRel)
}

/// Returns and clears the pending power-user-menu request — Win+X.
pub fn take_power_menu_request() -> bool {
    POWER_MENU_REQUESTED.swap(false, Ordering::AcqRel)
}

/// Spawn a dedicated thread that installs a low-level keyboard hook + runs
/// its own message loop. The hook turns a "Win-alone" tap into a dock-toggle
/// signal while preserving Win+key chords (Win+R, Win+E, etc).
pub fn spawn() {
    std::thread::spawn(|| unsafe {
        let hook: HHOOK = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(callback), None, 0) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("SetWindowsHookExW failed: {e}");
                return;
            }
        };
        // Required: low-level hooks only fire while the installing thread
        // pumps messages. We don't post any messages here, but GetMessage
        // blocks the thread alive so the hook stays valid.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = hook; // keep alive until message loop ends
    });
}

unsafe extern "system" fn callback(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code != 0 {
        return CallNextHookEx(None, code, w, l);
    }
    let msg = w.0 as u32;
    let kb = &*(l.0 as *const KBDLLHOOKSTRUCT);
    let injected = (kb.flags.0 & LLKHF_INJECTED) != 0;
    let vk = kb.vkCode;
    let is_win = vk == VK_LWIN.0 as u32 || vk == VK_RWIN.0 as u32;

    if !injected {
        match msg {
            x if x == WM_KEYDOWN || x == WM_SYSKEYDOWN => {
                if is_win {
                    WIN_DOWN.store(true, Ordering::SeqCst);
                    CHORD_USED.store(false, Ordering::SeqCst);
                } else if WIN_DOWN.load(Ordering::SeqCst) {
                    // Any non-Win key while Win is held = chord (Win+R, Win+E, etc).
                    CHORD_USED.store(true, Ordering::SeqCst);
                }
                // Ctrl+Alt+Space → spotlight. We check both modifier keys via
                // GetAsyncKeyState so we don't have to track them ourselves.
                if vk == VK_SPACE.0 as u32 {
                    let ctrl = (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                    let alt  = (GetAsyncKeyState(VK_MENU.0    as i32) as u16 & 0x8000) != 0;
                    if ctrl && alt {
                        SPOTLIGHT_REQUESTED.store(true, Ordering::SeqCst);
                        return LRESULT(1); // suppress so other apps don't see it
                    }
                }
                // Win+V → glassbar clipboard history (replaces the OS panel).
                // Win+X → glassbar power-user menu (replaces the OS quick-link).
                // Both are gated on WIN_DOWN being set by us in this same hook
                // so we know the user actually held Win — relying on
                // GetAsyncKeyState here is racy because the OS may swallow Win
                // during certain shell focus events.
                //
                // When we suppress one of these we also synth a fake Ctrl tap
                // so the OS's own Win-tap detector sees a chord and doesn't
                // open Start when the user releases Win. Without this:
                //   - we return LRESULT(1) → V never reaches the OS
                //   - OS sees only Win-down then Win-up
                //   - OS opens Start menu on top of our panel
                // The Ctrl-tap is the same trick `synthesise_chord_then_release_win`
                // uses for the Win-alone-tap dock toggle.
                if WIN_DOWN.load(Ordering::SeqCst) {
                    if vk == VK_V {
                        CLIPBOARD_REQUESTED.store(true, Ordering::SeqCst);
                        synthesise_ctrl_chord_marker();
                        return LRESULT(1);
                    }
                    if vk == VK_X {
                        POWER_MENU_REQUESTED.store(true, Ordering::SeqCst);
                        synthesise_ctrl_chord_marker();
                        return LRESULT(1);
                    }
                }
            }
            x if x == WM_KEYUP || x == WM_SYSKEYUP => {
                if is_win {
                    let chord = CHORD_USED.load(Ordering::SeqCst);
                    WIN_DOWN.store(false, Ordering::SeqCst);
                    if !chord {
                        // Win was tapped alone. Suppress the user's Win-up so
                        // the OS doesn't open Start, and synthesise a
                        // Ctrl-tap + Win-up combo to (a) make the OS treat
                        // Win as a modifier in a chord (no Start menu) and
                        // (b) properly release the Win modifier state.
                        synthesise_chord_then_release_win();
                        TOGGLE_REQUESTED.store(true, Ordering::SeqCst);
                        return LRESULT(1);
                    }
                }
            }
            _ => {}
        }
    }

    CallNextHookEx(None, code, w, l)
}

unsafe fn synthesise_chord_then_release_win() {
    let inputs = [
        kb_event(VK_CONTROL.0, false),
        kb_event(VK_CONTROL.0, true),
        kb_event(VK_LWIN.0, true),
    ];
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

/// Inject a Ctrl-down/Ctrl-up while Win is still physically held — the
/// OS's Win-tap detector counts this as a chord, so when the user later
/// releases Win it won't pop the Start menu on top of our custom panel.
unsafe fn synthesise_ctrl_chord_marker() {
    let inputs = [
        kb_event(VK_CONTROL.0, false),
        kb_event(VK_CONTROL.0, true),
    ];
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

fn kb_event(vk: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { Default::default() },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
