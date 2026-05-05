//! Win+V clipboard history. Polls the system clipboard on a slow loop and
//! keeps a ring buffer of the most recent entries — text and images both.
//! Windows' built-in clipboard service is opt-in and exposes no public read
//! API, so polling is the universally-available path.
//!
//! Uses the `arboard` crate (which internally uses clipboard-win on Windows)
//! for the actual read because the hand-rolled OpenClipboard/GetClipboardData
//! dance was failing intermittently — most likely because OpenClipboard from
//! a non-message-loop thread is unreliable. arboard creates a hidden message-
//! only window internally and runs the clipboard handshake against it,
//! which is the documented "right way" to read the clipboard from a
//! background thread.
//!
//! Every stage of the loop logs through `crate::logger` so we can finally
//! see what's happening on real user systems. The log lives at
//! `%APPDATA%\glassbar\debug.log` and rotates at ~1 MB.
//!
//! In-memory only by design. A persistent on-disk history would create a
//! sensitive footprint (passwords, tokens) the user didn't ask for; the
//! README's Privacy section commits to "no clipboard data leaves your
//! machine" and not writing it to disk is the strongest version of that.

use anyhow::{anyhow, Result};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::glog;

const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// How many entries to remember. Matches Windows' own Win+V default —
/// enough to be useful, small enough that the dropdown stays scannable.
const HISTORY_CAP: usize = 25;
/// After we set the clipboard ourselves (clipboard_use_entry), suppress the
/// next poll-detected change so we don't double-count our own write.
const SELF_WRITE_GRACE: Duration = Duration::from_millis(800);

/// What kind of payload an entry holds. Frontends render text and image
/// rows differently; the paste-back path also dispatches on this.
#[derive(Debug, Clone)]
pub enum ClipKind {
    Text(String),
    Image(ClipImage),
}

#[derive(Clone)]
pub struct ClipImage {
    pub width: u32,
    pub height: u32,
    /// PNG-encoded bytes. We pre-encode at capture time so display +
    /// paste-back share the same buffer — saves us re-encoding on every
    /// preview render and avoids holding raw RGBA arrays for every entry.
    pub png: Vec<u8>,
}

impl std::fmt::Debug for ClipImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("png_len", &self.png.len())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct ClipEntry {
    /// Stable ID — used by the frontend to identify "use this entry"
    /// without having to round-trip the full payload (especially for
    /// large images, where echoing megabytes back through the IPC just to
    /// say "that one" is wasteful).
    pub id: u64,
    pub kind: ClipKind,
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
    next_id: u64,
}

/// Spawn the polling thread. Idempotent in practice because main.rs
/// only calls this once at startup.
pub fn spawn() {
    glog!("clipboard::spawn called");

    // Disable Windows' own clipboard-history feature so the OS Win+V panel
    // can't surface alongside (or under) ours. Without this, suppressing
    // the V keydown in the low-level hook isn't always enough — under the
    // right timing the OS clipboard service still pops its own panel,
    // which then peeks out from behind ours when the user taps Win+V a
    // second time. Disabling the feature is the only durable fix.
    //
    // We don't restore this on shutdown — the user can re-enable in
    // Settings → System → Clipboard if they want the OS panel back.
    match disable_os_clipboard_history() {
        Ok(()) => glog!("clipboard: OS history registry set to 0"),
        Err(e) => glog!("clipboard: failed to disable OS history: {e}"),
    }

    std::thread::spawn(|| {
        glog!("clipboard: poll thread started");
        // One arboard handle per thread — creating it once is cheaper
        // than per-tick (each new() allocates a hidden message window).
        // If construction fails, the thread quietly retries inside the
        // loop so a transient COM-init race at startup doesn't kill it.
        let mut clip: Option<arboard::Clipboard> = None;
        // Fingerprint of the most-recently-captured payload so we don't
        // log a noisy "ingested" line on every 500 ms tick when the user
        // hasn't actually copied anything new. Empty string = nothing yet.
        let mut last_fp = String::new();
        let mut tick: u64 = 0;
        loop {
            std::thread::sleep(POLL_INTERVAL);
            tick = tick.wrapping_add(1);
            if clip.is_none() {
                match arboard::Clipboard::new() {
                    Ok(c) => {
                        glog!("clipboard: arboard handle created");
                        clip = Some(c);
                    }
                    Err(e) => {
                        glog!("clipboard: arboard::new() failed: {e}");
                        continue;
                    }
                }
            }
            let cb = clip.as_mut().unwrap();

            // Try text first — it's the overwhelmingly common case. Fall
            // through to image if there's no text format on the clipboard.
            match cb.get_text() {
                Ok(text) => {
                    let fp = format!("t:{}", short_fingerprint(text.as_bytes()));
                    if fp != last_fp {
                        last_fp = fp;
                        glog!("clipboard: ingest text len={}", text.len());
                        ingest_text(text);
                    }
                    continue;
                }
                Err(arboard::Error::ContentNotAvailable) => {
                    // Fall through and try the image format below.
                }
                Err(e) => {
                    glog!("clipboard: get_text error: {e:?} — dropping handle");
                    clip = None;
                    continue;
                }
            }

            match cb.get_image() {
                Ok(img) => {
                    let fp = format!("i:{}x{}:{}",
                        img.width, img.height,
                        short_fingerprint(&img.bytes[..img.bytes.len().min(4096)]));
                    if fp != last_fp {
                        last_fp = fp;
                        glog!("clipboard: ingest image {}x{}", img.width, img.height);
                        ingest_image(img);
                    }
                }
                Err(arboard::Error::ContentNotAvailable) => {
                    // Neither text nor image — likely files, custom format,
                    // or empty clipboard. Reset fingerprint so the next
                    // copied text/image fires the ingestion log.
                    if !last_fp.is_empty() {
                        last_fp.clear();
                    }
                }
                Err(e) => {
                    glog!("clipboard: get_image error: {e:?} — dropping handle");
                    clip = None;
                }
            }
        }
    });
}

/// Cheap rolling content fingerprint — we just want to detect change, not
/// guard against collisions. Sum + length + first bytes is plenty.
fn short_fingerprint(bytes: &[u8]) -> String {
    let sum: u64 = bytes.iter().take(2048).fold(0u64, |a, b| a.wrapping_add(*b as u64));
    let prefix: String = bytes.iter().take(8).map(|b| format!("{:02x}", b)).collect();
    format!("{}-{:x}-{}", bytes.len(), sum, prefix)
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

fn ingest_text(text: String) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        glog!("clipboard: skip empty text");
        return;
    }
    let mut s = state().lock().unwrap();

    // Suppress capture of our own clipboard_use_entry write — without this
    // every paste would reorder the history (the picked entry would jump
    // back to the top via the polling loop), making the top-of-history
    // unstable while the user is browsing.
    if let Some(t) = s.last_self_write {
        if t.elapsed() < SELF_WRITE_GRACE {
            glog!("clipboard: skip text — within self-write grace");
            return;
        }
    }

    // De-dupe: if this text matches an existing entry, lift it to the top
    // instead of pushing a duplicate. Matches the behaviour of every
    // clipboard-history app the user might already be familiar with.
    if let Some(pos) = s.entries.iter().position(|e| matches!(&e.kind, ClipKind::Text(t) if t == &text)) {
        let mut entry = s.entries.remove(pos);
        entry.at = Instant::now();
        s.entries.insert(0, entry);
        glog!("clipboard: text deduped, lifted to top");
        return;
    }

    let id = next_id(&mut s);
    s.entries.insert(0, ClipEntry { id, kind: ClipKind::Text(text), at: Instant::now() });
    if s.entries.len() > HISTORY_CAP {
        s.entries.truncate(HISTORY_CAP);
    }
    glog!("clipboard: text entry stored, history len={}", s.entries.len());
}

fn ingest_image(img: arboard::ImageData<'_>) {
    use image::{ImageBuffer, Rgba};
    use std::io::Cursor;

    let width = img.width as u32;
    let height = img.height as u32;
    let expected_bytes = width as usize * height as usize * 4;
    if img.bytes.len() != expected_bytes {
        glog!("clipboard: image byte size mismatch (got {}, expected {})", img.bytes.len(), expected_bytes);
        return;
    }
    // Sanity bound — arboard returns owned bytes so we can move them
    // into ImageBuffer without copying the RGBA array twice. Anything
    // bigger than ~25 MP we just refuse — memory cost balloons too fast.
    if width == 0 || height == 0 || width as u64 * height as u64 > 25_000_000 {
        glog!("clipboard: image dimensions out of range ({}x{})", width, height);
        return;
    }

    let buf = match ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, img.bytes.into_owned()) {
        Some(b) => b,
        None => { glog!("clipboard: ImageBuffer::from_raw failed"); return; }
    };

    let mut png = Vec::with_capacity(64 * 1024);
    if let Err(e) = buf.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png) {
        glog!("clipboard: PNG encode failed: {e}");
        return;
    }

    let mut s = state().lock().unwrap();
    if let Some(t) = s.last_self_write {
        if t.elapsed() < SELF_WRITE_GRACE {
            glog!("clipboard: skip image — within self-write grace");
            return;
        }
    }

    // De-dupe images by (width, height, png-len) — exact-content match would
    // need a hash. Given screenshots typically have unique sizes-or-content,
    // this catches the common "same screenshot pasted twice" case.
    if let Some(pos) = s.entries.iter().position(|e| matches!(&e.kind, ClipKind::Image(img) if img.width == width && img.height == height && img.png.len() == png.len())) {
        let mut entry = s.entries.remove(pos);
        entry.at = Instant::now();
        s.entries.insert(0, entry);
        glog!("clipboard: image deduped, lifted to top");
        return;
    }

    let id = next_id(&mut s);
    s.entries.insert(0, ClipEntry {
        id,
        kind: ClipKind::Image(ClipImage { width, height, png }),
        at: Instant::now(),
    });
    if s.entries.len() > HISTORY_CAP {
        s.entries.truncate(HISTORY_CAP);
    }
    glog!("clipboard: image entry stored, history len={}", s.entries.len());
}

fn next_id(s: &mut State) -> u64 {
    s.next_id = s.next_id.wrapping_add(1);
    s.next_id
}

/// Snapshot the current history. Cloned on the way out so callers can hold
/// the data without keeping the mutex.
pub fn history() -> Vec<ClipEntry> {
    state().lock().unwrap().entries.clone()
}

/// Look up an entry by id — used by the use-entry command so the frontend
/// only has to ship the id back, not the full payload.
pub fn find(id: u64) -> Option<ClipEntry> {
    state().lock().unwrap().entries.iter().find(|e| e.id == id).cloned()
}

/// Mark that we just wrote to the clipboard ourselves — the polling loop
/// will skip the next change so the entry doesn't reorder mid-paste.
pub fn note_self_write() {
    state().lock().unwrap().last_self_write = Some(Instant::now());
}

/// Drop every entry. Hooked up to the "Clear" button in the panel.
pub fn clear() {
    glog!("clipboard: cleared by user");
    state().lock().unwrap().entries.clear();
}
