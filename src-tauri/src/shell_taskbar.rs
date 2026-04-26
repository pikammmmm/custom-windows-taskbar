use anyhow::Result;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, BOOL, TRUE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_LWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, ShowWindow, SW_HIDE, SW_SHOW,
};

/// Hide the primary `Shell_TrayWnd` and every `Shell_SecondaryTrayWnd`
/// (multi-monitor secondary trays). Returns the count of windows hidden.
pub fn hide_windows_taskbar() -> Result<usize> {
    apply_to_taskbars(SW_HIDE)
}

pub fn show_windows_taskbar() -> Result<usize> {
    apply_to_taskbars(SW_SHOW)
}

fn apply_to_taskbars(cmd: windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD) -> Result<usize> {
    let mut count = 0;
    unsafe {
        let primary = FindWindowW(PCWSTR(wide("Shell_TrayWnd").as_ptr()), PCWSTR::null())
            .unwrap_or_default();
        if primary.0 as isize != 0 {
            let _ = ShowWindow(primary, cmd);
            count += 1;
        }
        let mut secondaries: Vec<HWND> = Vec::new();
        let _ = EnumWindows(Some(collect_secondary), LPARAM(&mut secondaries as *mut _ as isize));
        for h in &secondaries {
            let _ = ShowWindow(*h, cmd);
            count += 1;
        }
    }
    Ok(count)
}

unsafe extern "system" fn collect_secondary(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut buf = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut buf);
    if len > 0 {
        let class = String::from_utf16_lossy(&buf[..len as usize]);
        if class == "Shell_SecondaryTrayWnd" {
            let list = &mut *(lparam.0 as *mut Vec<HWND>);
            list.push(hwnd);
        }
    }
    TRUE
}

/// Send a single tap of the Windows key — opens / toggles the Start menu.
pub fn tap_start_key() -> Result<()> {
    unsafe {
        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_LWIN,
                        wScan: 0,
                        dwFlags: Default::default(),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_LWIN,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
