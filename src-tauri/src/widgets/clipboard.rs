//! Win+V clipboard history. Polls the system clipboard on a slow loop and
//! keeps a ring buffer of the most recent text entries — Windows' built-in
//! clipboard service is opt-in and exposes no public read API, so polling
//! CF_UNICODETEXT is the universally-available path.
//!
//! We deliberately keep this in-memory only. A persistent on-disk history
//! would create a sensitive footprint (passwords, tokens) the user didn't
//! ask for; the README's Privacy section commits to "no clipboard data
//! leaves your machine" and not writing it to disk is the strongest
//! version of that promise.

use anyhow::{anyhow, Result};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};

const CF_UNICODETEXT: u32 = 13;
const POLL_INTERVAL: Duration = Duration::from_millis(700);
/// How many entries to remember. Matches Windows' own Win+V default —
/// enough to be useful, small enough that the dropdown stays scannable.
const HISTORY_CAP: usize = 25;
/// After we set the clipboard ourselves (clipboard_use_entry), suppress the
/// next poll-detected change so we don't double-count our own write.
const SELF_WRITE_GRACE: Duration = Duration::from_millis(800);

#[derive(Debug, Clone)]
pub struct ClipEntry {
    /// Full text. Frontend truncates for display but the original is
    /// what we copy back when the user picks the entry.
    pub text: String,
    /// Wall-clock-ish: only used for ordering and display ("just now",
    /// "5m ago"). We ignore daylight-savings drift — close enough.
    pub at: Instant,
}

fn state() -> &'static Mutex<State> {
    static SLOT: OnceLock<Mutex<State>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(State::default()))
}

#[derive(Default)]
struct State {
    entries: Vec<ClipEntry>,
    last_self_write: Option<Instant>,
}

/// Spawn the polling thread. Idempotent in practice because main.rs
/// only calls this once at startup.
pub fn spawn() {
    // Disable Windows' own clipboard-history feature so the OS Win+V panel
    // can't surface alongside (or under) ours. Without this, suppressing
    // the V keydown in the low-level hook isn't always enough — under the
    // right timing the OS clipboard service still pops its own panel,
    // which then peeks out from behind ours when the user taps Win+V a
    // second time. Disabling the feature is the only durable fix.
    //
    // We don't restore this on shutdown — the user can re-enable in
    // Settings → System → Clipboard if they want the OS panel back.
    let _ = disable_os_clipboard_history();

    std::thread::spawn(|| loop {
        std::thread::sleep(POLL_INTERVAL);
        if let Ok(text) = read_clipboard_text() {
            ingest(text);
        }
    });
}

fn disable_os_clipboard_history() -> Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey("Software\\Microsoft\\Clipboard")
        .map_err(|e| anyhow!("create Clipboard key: {e}"))?;
    key.set_value("EnableClipboardHistory", &0u32)
        .map_err(|e| anyhow!("set EnableClipboardHistory: {e}"))?;
    Ok(())
}

fn ingest(text: String) {
    let trimmed = text.trim();
    if trimmed.is_empty() { return; }

    let mut s = state().lock().unwrap();

    // Suppress capture of our own clipboard_use_entry write — without this
    // every paste would reorder the history (the picked entry would jump
    // back to the top via the polling loop), making the top-of-history
    // unstable while the user is browsing.
    if let Some(t) = s.last_self_write {
        if t.elapsed() < SELF_WRITE_GRACE {
            return;
        }
    }

    // De-dupe: if this text matches an existing entry, lift it to the top
    // instead of pushing a duplicate. Matches the behaviour of every
    // clipboard-history app the user might already be familiar with.
    if let Some(pos) = s.entries.iter().position(|e| e.text == text) {
        let mut entry = s.entries.remove(pos);
        entry.at = Instant::now();
        s.entries.insert(0, entry);
        return;
    }

    s.entries.insert(0, ClipEntry { text, at: Instant::now() });
    if s.entries.len() > HISTORY_CAP {
        s.entries.truncate(HISTORY_CAP);
    }
}

/// Snapshot the current history. Cloned on the way out so callers can hold
/// the data without keeping the mutex.
pub fn history() -> Vec<ClipEntry> {
    state().lock().unwrap().entries.clone()
}

/// Mark that we just wrote `text` to the clipboard ourselves — the polling
/// loop will skip the next change so the entry doesn't reorder mid-paste.
pub fn note_self_write() {
    state().lock().unwrap().last_self_write = Some(Instant::now());
}

/// Drop every entry. Hooked up to the "Clear" button in the panel.
pub fn clear() {
    state().lock().unwrap().entries.clear();
}

/// Best-effort read of CF_UNICODETEXT. Errors when the clipboard is busy or
/// when no text format is present — the caller treats both as "no change."
fn read_clipboard_text() -> Result<String> {
    unsafe {
        // Skip the OpenClipboard syscall entirely when no text is on the
        // clipboard (e.g., user copied an image or file). IsClipboardFormatAvailable
        // doesn't require the clipboard to be open and is much cheaper.
        if !IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
            return Err(anyhow!("no CF_UNICODETEXT format"));
        }
        // OpenClipboard can transiently fail with ERROR_ACCESS_DENIED when
        // another app holds it (Excel mid-edit, browser drag operation,
        // installer dialog). Retry a few times with tiny backoffs — fixes
        // the case where the very first poll after a copy raced the
        // copying app's CloseClipboard.
        let mut opened = false;
        for delay_us in [0, 5_000, 15_000, 40_000] {
            if delay_us > 0 { std::thread::sleep(Duration::from_micros(delay_us)); }
            if OpenClipboard(HWND(std::ptr::null_mut())).is_ok() {
                opened = true;
                break;
            }
        }
        if !opened {
            return Err(anyhow!("OpenClipboard failed after retries"));
        }
        let result = (|| -> Result<String> {
            let h = GetClipboardData(CF_UNICODETEXT)
                .map_err(|e| anyhow!("GetClipboardData: {e}"))?;
            if h.is_invalid() {
                return Err(anyhow!("no CF_UNICODETEXT"));
            }
            // GlobalLock returns *mut c_void; we treat it as *const u16 of
            // an unknown length, terminated by a 0 word. There's no length
            // probe API for HGLOBAL clipboard data so we walk until null.
            let hglobal = windows::Win32::Foundation::HGLOBAL(h.0);
            let ptr = GlobalLock(hglobal) as *const u16;
            if ptr.is_null() {
                return Err(anyhow!("GlobalLock null"));
            }
            let mut len = 0usize;
            // Hard cap so a malformed buffer (no null terminator) can't
            // walk us off the end of the heap. 1 MB of UTF-16 is ~500K
            // chars — way more than any realistic clipboard text entry.
            while len < 524_288 && *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let s = String::from_utf16_lossy(slice);
            let _ = GlobalUnlock(hglobal);
            Ok(s)
        })();
        let _ = CloseClipboard();
        result
    }
}
