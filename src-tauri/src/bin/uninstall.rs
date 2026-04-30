//! Standalone uninstaller for glassbar. Cargo picks this up automatically
//! from src/bin/, producing target/release/uninstall.exe alongside the main
//! glassbar.exe. Console subsystem (no `windows_subsystem = "windows"`) so
//! the user sees what's happening.
//!
//! Steps, in order:
//! 1. Stop any running glassbar.exe.
//! 2. Re-show the Windows taskbar (Shell_TrayWnd + every Shell_SecondaryTrayWnd) —
//!    glassbar hides them on launch and only the running app's exit handler
//!    restores them, so we have to do it ourselves.
//! 3. Delete the HKCU Run autostart entry.
//! 4. Wipe user data (%APPDATA%\glassbar\) and the WebView2 cache
//!    (%LOCALAPPDATA%\com.glassbar.app\).
//! 5. If the MSI is installed, hand off to msiexec /x for the registered
//!    uninstall — that's how Windows expects MSIs to be removed.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use winreg::enums::*;
use winreg::RegKey;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, ShowWindow, SW_SHOW,
};

fn main() {
    println!("glassbar uninstaller");
    println!("====================");
    println!();

    step("Stopping any running glassbar.exe");
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "glassbar.exe"])
        .output();

    step("Restoring the Windows taskbar");
    restore_taskbars();

    step("Removing autostart entry");
    let run = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_READ | KEY_WRITE,
        );
    if let Ok(run) = run {
        // Try a few common casings just in case.
        for name in ["glassbar", "Glassbar", "GLASSBAR"] {
            let _ = run.delete_value(name);
        }
    }

    step("Removing user data and cache");
    let mut data_targets: Vec<PathBuf> = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        data_targets.push(PathBuf::from(&appdata).join("glassbar"));
        data_targets.push(PathBuf::from(&appdata).join("com.glassbar.app"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        data_targets.push(PathBuf::from(&local).join("com.glassbar.app"));
    }
    for path in data_targets {
        if path.exists() {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => println!("    removed {}", path.display()),
                Err(e) => println!("    couldn't remove {} ({})", path.display(), e),
            }
        }
    }

    step("Looking up MSI install");
    if let Some(product_code) = find_installed_msi_code() {
        println!("    found {}, handing off to Windows Installer…", product_code);
        // /qb shows a small progress bar so the user can see something is
        // happening — silent mode would feel like nothing's running.
        let _ = Command::new("msiexec")
            .args(["/x", &product_code, "/qb"])
            .status();
    } else {
        println!("    no MSI install found (portable .exe — just delete it)");
    }

    println!();
    println!("Done. Press Enter to close.");
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
}

fn step(label: &str) {
    println!("[*] {label}");
}

/// Walk Apps & Features and return the MSI ProductCode of any entry whose
/// DisplayName contains "glassbar" (case-insensitive). Returns None if no
/// match — the user is on the portable build, or never installed via MSI.
fn find_installed_msi_code() -> Option<String> {
    let candidates = [
        (HKEY_LOCAL_MACHINE, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];
    for (root, path) in candidates {
        let Ok(key) = RegKey::predef(root).open_subkey(path) else { continue };
        for sub_name in key.enum_keys().filter_map(|r| r.ok()) {
            let Ok(sub) = key.open_subkey(&sub_name) else { continue };
            let display: Result<String, _> = sub.get_value("DisplayName");
            if let Ok(d) = display {
                if d.to_lowercase().contains("glassbar") {
                    println!("    found '{d}' under {sub_name}");
                    return Some(sub_name);
                }
            }
        }
    }
    None
}

/// Re-show the primary taskbar plus every secondary-monitor taskbar.
fn restore_taskbars() {
    unsafe {
        let class: Vec<u16> = "Shell_TrayWnd".encode_utf16().chain(std::iter::once(0)).collect();
        let primary = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()).unwrap_or_default();
        if primary.0 as isize != 0 {
            let _ = ShowWindow(primary, SW_SHOW);
        }
        let mut secondaries: Vec<HWND> = Vec::new();
        let _ = EnumWindows(
            Some(collect_secondary_trays),
            LPARAM(&mut secondaries as *mut _ as isize),
        );
        for h in secondaries {
            let _ = ShowWindow(h, SW_SHOW);
        }
    }
}

unsafe extern "system" fn collect_secondary_trays(hwnd: HWND, lparam: LPARAM) -> BOOL {
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
