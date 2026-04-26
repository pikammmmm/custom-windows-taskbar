use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const CF_UNICODETEXT: u32 = 13;

#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub display_name: String,
    pub version: Option<String>,
    pub size_bytes: u64,
}

/// Cheap inspection of an .exe — file size from the filesystem, version
/// from the PE's VS_VERSION_INFO resource. Returns None for version on files
/// that don't ship one (most non-Microsoft exes only set FileVersion, some
/// ship nothing at all).
pub fn app_info(exe_path: &str) -> Result<AppInfo> {
    let p = Path::new(exe_path);
    let size_bytes = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let display_name = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let version = read_file_version(exe_path).ok();
    Ok(AppInfo { display_name, version, size_bytes })
}

fn read_file_version(exe_path: &str) -> Result<String> {
    let wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut handle: u32 = 0;
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), Some(&mut handle));
        if size == 0 {
            return Err(anyhow!("no version resource"));
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(
            PCWSTR(wide.as_ptr()),
            0,
            size,
            buf.as_mut_ptr() as *mut _,
        )?;

        // Query the Translation table — the FIRST language/codepage entry tells
        // us which sub-block to read FileVersion from. Default to en-US (0409)
        // + Unicode (04B0) if the table is missing or malformed.
        let mut trans_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut trans_len: u32 = 0;
        let trans_query: Vec<u16> = "\\VarFileInfo\\Translation"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut lang_codepage = (0x0409u16, 0x04B0u16);
        if VerQueryValueW(
            buf.as_ptr() as *const _,
            PCWSTR(trans_query.as_ptr()),
            &mut trans_ptr,
            &mut trans_len,
        )
        .as_bool()
            && !trans_ptr.is_null()
            && trans_len >= 4
        {
            let pair = trans_ptr as *const u16;
            lang_codepage = (*pair, *pair.add(1));
        }

        let sub_block = format!(
            "\\StringFileInfo\\{:04x}{:04x}\\FileVersion",
            lang_codepage.0, lang_codepage.1
        );
        let sub_wide: Vec<u16> = sub_block.encode_utf16().chain(std::iter::once(0)).collect();
        let mut value_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut value_len: u32 = 0;
        if !VerQueryValueW(
            buf.as_ptr() as *const _,
            PCWSTR(sub_wide.as_ptr()),
            &mut value_ptr,
            &mut value_len,
        )
        .as_bool()
            || value_ptr.is_null()
            || value_len == 0
        {
            return Err(anyhow!("FileVersion not present"));
        }
        let slice = std::slice::from_raw_parts(value_ptr as *const u16, value_len as usize);
        let len = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
        Ok(String::from_utf16_lossy(&slice[..len]))
    }
}

/// Open File Explorer with the target file pre-selected. Equivalent to
/// "Show in folder" — uses explorer.exe's /select command since the shell
/// API for this (SHOpenFolderAndSelectItems) needs a PIDL we don't have.
pub fn show_in_explorer(path: &str) -> Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path))
        .spawn()?;
    Ok(())
}

/// Re-launch the executable elevated. The OS shows the UAC consent prompt;
/// the user can decline. If they decline we still return Ok — the user's
/// choice isn't an error from our side.
pub fn run_as_admin(path: &str) -> Result<()> {
    invoke_shell_verb(path, "runas")
}

/// Open the standard Win32 file-properties dialog (Properties from the
/// right-click shell menu).
pub fn show_properties(path: &str) -> Result<()> {
    invoke_shell_verb(path, "properties")
}

fn invoke_shell_verb(path: &str, verb: &str) -> Result<()> {
    let path_w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let verb_w: Vec<u16> = verb.encode_utf16().chain(std::iter::once(0)).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        hwnd: HWND(std::ptr::null_mut()),
        lpVerb: PCWSTR(verb_w.as_ptr()),
        lpFile: PCWSTR(path_w.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info)? };
    Ok(())
}

/// Put `text` on the system clipboard as Unicode. Uses the global-alloc +
/// CF_UNICODETEXT dance because that's the only format every paste target
/// (browsers, editors, terminal, Win+V history) reliably accepts.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    unsafe {
        OpenClipboard(HWND(std::ptr::null_mut()))?;
        // EmptyClipboard transfers ownership of the previously-set data
        // back to the OS, which frees it. Required before SetClipboardData.
        let _ = EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
        if h.is_invalid() {
            let _ = CloseClipboard();
            return Err(anyhow!("GlobalAlloc failed"));
        }
        let dst = GlobalLock(h) as *mut u16;
        if dst.is_null() {
            let _ = CloseClipboard();
            return Err(anyhow!("GlobalLock failed"));
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
        let _ = GlobalUnlock(h);
        // SetClipboardData transfers ownership of the HGLOBAL to the OS —
        // do NOT free it ourselves on success.
        if SetClipboardData(CF_UNICODETEXT, HANDLE(h.0)).is_err() {
            let _ = CloseClipboard();
            return Err(anyhow!("SetClipboardData failed"));
        }
        let _ = CloseClipboard();
    }
    Ok(())
}
