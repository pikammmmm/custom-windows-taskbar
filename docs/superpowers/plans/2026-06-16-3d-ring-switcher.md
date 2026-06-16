# 3D Ring Switcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Windows' Alt+Tab with a fullscreen, dimmed 3D ring (turntable) of live window snapshots that you spin through by holding Alt and tapping Tab, committing focus on Alt-release.

**Architecture:** A new `switcher` feature inside glassbar. The existing `WH_KEYBOARD_LL` hook (`keyhook.rs`) is extended to recognize the Alt+Tab chord and emit intent through atomics. A new `switcher.rs` background loop (modeled on `dock_autohide.rs`) owns the session: it enumerates windows (`win32::enumerate_windows`), shows a pre-created transparent/fullscreen/topmost/`WS_EX_NOACTIVATE` Tauri webview, drives the selection index, and on commit calls `win32::focus_aggressive`. The webview is a pure renderer: Three.js draws the ring from `switcher:open` / `switcher:select` / `switcher:thumb` events and never decides focus. Thumbnails come from a new `capture.rs` (PrintWindow → DIB → downscaled PNG → base64), with the existing `icons::get_icon_data_url` as the fallback/label image.

**Tech Stack:** Rust + Tauri 2, `windows` 0.58 (Win32 GDI/Hook/DWM), `image` 0.25 (png), `base64` 0.22; frontend is plain ES-module HTML/CSS/JS (no bundler, `withGlobalTauri: true`) with a vendored Three.js ESM build.

---

## Key facts about the existing codebase (verified)

- **Hook** (`src-tauri/src/keyhook.rs`): `WH_KEYBOARD_LL` installed on a dedicated thread pumping `GetMessageW`. Callback `unsafe extern "system" fn callback(code, w, l) -> LRESULT`: `code != 0` → `CallNextHookEx`. `let msg = w.0 as u32` is one of `WM_KEYDOWN|WM_SYSKEYDOWN|WM_KEYUP|WM_SYSKEYUP`; `let kb = &*(l.0 as *const KBDLLHOOKSTRUCT)`; `kb.vkCode` is the VK. Return `LRESULT(1)` to swallow. Modifier state via `(GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0`. Injected keys (`LLKHF_INJECTED`) are already ignored. Signals use `static X: AtomicBool` + `pub fn take_x() -> bool { X.swap(false, Ordering::AcqRel) }`, consumed by the `dock_autohide` loop. **Emitting/window calls from the callback are unsound — only touch atomics there.**
- **Enumeration** (`src-tauri/src/win32.rs`): `pub fn enumerate_windows() -> anyhow::Result<Vec<WindowInfo>>` where `pub struct WindowInfo { pub hwnd: isize, pub title: String, pub exe_path: String }`. `EnumWindows` returns windows in **Z-order, top first** (so index 0 ≈ current foreground), and `enum_proc` already applies full alt-tab eligibility (visible, not tool-window, not DWM-cloaked, owner/`WS_EX_APPWINDOW`, non-empty title, excludes glassbar). No MRU history is kept — Z-order is our ordering.
- **Activation** (`win32.rs`): `pub fn focus_aggressive(hwnd: isize) -> Result<()>` restores if `IsIconic` then `SetForegroundWindow` via `AttachThreadInput`; safe to call with `0`. `pub fn foreground_hwnd() -> isize`.
- **Tauri windows** (`src-tauri/src/windows_setup.rs`): windows are pre-created hidden with `WebviewWindowBuilder` then shown on demand; helpers `force_webview_transparent`, `apply_glass`, `apply_noactivate`, `clip_to_rounded`. `src-tauri/tauri.conf.json` has `"csp": null` (Three.js + WebGL + `data:` URLs work, no change) and `withGlobalTauri: true` (use `window.__TAURI__`, **no npm imports**); `frontendDist` = `../ui`, no bundler/dev-server.
- **DWM** (`src-tauri/src/dwm.rs`): `strip_decorations(hwnd)`, `suppress_nc_rendering(hwnd)`, `set_position_topmost(hwnd,x,y)`, `make_noactivate(hwnd)`. `show()` may re-add `WS_CAPTION` on Win11 → re-strip after show.
- **Commands/events** (`src-tauri/src/commands.rs`): `#[tauri::command] pub fn name(app: AppHandle, ...) -> Result<T, String>`, registered in `tauri::generate_handler![...]` in `main.rs`. `app.emit_to("label","event",&payload)` can silently drop → pair with a `win.eval(...)` fallback. `get_icon(exe_path, hwnd) -> Result<String,String>` already returns a cached base64/SVG data URL.
- **Icons** (`src-tauri/src/icons.rs`): `pub fn get_icon_data_url(exe_path: &str, hwnd: Option<isize>) -> Result<String>` (disk-cached PNG/SVG). The `hicon_to_png` GDI pipeline (GetDC → CreateCompatibleDC → CreateDIBSection 32bpp top-down → render → GetDIBits → BGRA→RGBA swap → `image::RgbaImage` → PNG) is the exact template for window capture.
- **Cargo.toml**: `image = {0.25, default-features=false, features=["png"]}`, `base64="0.22"`, `sha2="0.10"`, `windows="0.58"` with `Win32_Graphics_Gdi`, `Win32_UI_WindowsAndMessaging`, `Win32_Graphics_Dwm`, `Win32_UI_Input_KeyboardAndMouse`, `Win32_System_Threading`. `image::imageops::resize` + `FilterType` are core (no extra feature needed).

---

## File structure

**Create:**
- `src-tauri/src/switcher.rs` — session state + pure selection logic + background loop + capture orchestration.
- `src-tauri/src/capture.rs` — `window_thumbnail_data_url(hwnd, max_px) -> Result<String>` (PrintWindow → downscaled PNG → base64).
- `ui/switcher/index.html`, `ui/switcher/style.css`, `ui/switcher/app.js` — overlay page + Three.js scene + event wiring.
- `ui/switcher/ring.js` — pure ring-layout math (browser + node importable).
- `ui/switcher/ring.test.mjs` — node unit test for ring math.
- `ui/switcher/three.module.min.js` — vendored Three.js ESM (pinned r0.160.0).

**Modify:**
- `src-tauri/src/keyhook.rs` — VK consts, Alt+Tab classify (pure) + wiring, session atomics + `take_*`/`reset` API.
- `src-tauri/src/win32.rs` — add `is_iconic(hwnd) -> bool` and `monitor_rect_for_hwnd(hwnd) -> (i32,i32,i32,i32)`.
- `src-tauri/src/windows_setup.rs` — create the hidden `switcher` webview.
- `src-tauri/src/commands.rs` — `show_switcher` / `hide_switcher` helpers + emit; register nothing new beyond what `main.rs` lists.
- `src-tauri/src/main.rs` — `mod switcher; mod capture;`, spawn `switcher::spawn(app.handle().clone())`.

**Branch:** already on `feature/ring-switcher`. Commit after every task.

---

## PHASE A — Backend plumbing (no visuals)

### Task 1: Pure selection logic in `switcher.rs`

**Files:**
- Create: `src-tauri/src/switcher.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/switcher.rs`:

```rust
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
```

- [ ] **Step 2: Register the module so it compiles**

In `src-tauri/src/main.rs`, add alongside the other `mod` lines (e.g. after `mod dock_autohide;`):

```rust
mod switcher;
mod capture;
```

Create a placeholder `src-tauri/src/capture.rs` so `mod capture;` compiles:

```rust
//! Window snapshot capture (PrintWindow -> downscaled PNG -> base64 data URL).
// Implemented in Task 3.
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cd src-tauri && cargo test switcher::tests`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/switcher.rs src-tauri/src/capture.rs src-tauri/src/main.rs
git commit -m "feat(switcher): pure selection index math + module scaffolding"
```

---

### Task 2: Alt+Tab chord classification (pure) + hook wiring

**Files:**
- Modify: `src-tauri/src/keyhook.rs`
- Test: `src-tauri/src/keyhook.rs` (`#[cfg(test)] mod switcher_tests`)

- [ ] **Step 1: Write the failing test for the pure classifier**

At the bottom of `src-tauri/src/keyhook.rs`, add the action enum, classifier, and tests. (The classifier is pure so it is fully unit-testable without the hook.)

```rust
// ─────────────────────────────────────────────────────────────────────────
// Alt+Tab ring-switcher chord classification (pure; unit-tested).
// ─────────────────────────────────────────────────────────────────────────

const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_MENU_ANY: [u32; 3] = [0x12 /*VK_MENU*/, 0xA4 /*VK_LMENU*/, 0xA5 /*VK_RMENU*/];

const WM_KEYDOWN_U: u32 = 0x0100;
const WM_KEYUP_U: u32 = 0x0101;
const WM_SYSKEYDOWN_U: u32 = 0x0104;
const WM_SYSKEYUP_U: u32 = 0x0105;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitcherAction {
    None,
    Open(i32),  // +1 forward, -1 reverse
    Step(i32),
    Commit,
    Cancel,
}

/// Decide what an incoming key event means for the switcher.
/// Returns the action and whether the key should be swallowed (LRESULT(1)).
/// `active` = a switcher session is currently open (hook-private flag).
pub fn classify_switcher(
    msg: u32,
    vk: u32,
    alt_down: bool,
    shift_down: bool,
    active: bool,
) -> (SwitcherAction, bool) {
    let is_down = msg == WM_KEYDOWN_U || msg == WM_SYSKEYDOWN_U;
    let is_up = msg == WM_KEYUP_U || msg == WM_SYSKEYUP_U;

    if is_down && vk == VK_TAB && alt_down {
        let dir = if shift_down { -1 } else { 1 };
        return if active {
            (SwitcherAction::Step(dir), true)
        } else {
            (SwitcherAction::Open(dir), true)
        };
    }
    if is_down && vk == VK_ESCAPE && active {
        return (SwitcherAction::Cancel, true);
    }
    if is_up && VK_MENU_ANY.contains(&vk) && active {
        // Don't swallow Alt-up; apps may rely on seeing it.
        return (SwitcherAction::Commit, false);
    }
    (SwitcherAction::None, false)
}

#[cfg(test)]
mod switcher_tests {
    use super::*;

    #[test]
    fn alt_tab_when_idle_opens_forward_and_swallows() {
        assert_eq!(
            classify_switcher(WM_KEYDOWN_U, VK_TAB, true, false, false),
            (SwitcherAction::Open(1), true)
        );
    }

    #[test]
    fn alt_shift_tab_when_idle_opens_reverse() {
        assert_eq!(
            classify_switcher(WM_SYSKEYDOWN_U, VK_TAB, true, true, false),
            (SwitcherAction::Open(-1), true)
        );
    }

    #[test]
    fn alt_tab_when_active_steps_forward() {
        assert_eq!(
            classify_switcher(WM_SYSKEYDOWN_U, VK_TAB, true, false, true),
            (SwitcherAction::Step(1), true)
        );
    }

    #[test]
    fn tab_without_alt_is_ignored_and_passes() {
        assert_eq!(
            classify_switcher(WM_KEYDOWN_U, VK_TAB, false, false, false),
            (SwitcherAction::None, false)
        );
    }

    #[test]
    fn escape_while_active_cancels_and_swallows() {
        assert_eq!(
            classify_switcher(WM_KEYDOWN_U, VK_ESCAPE, true, false, true),
            (SwitcherAction::Cancel, true)
        );
    }

    #[test]
    fn escape_while_idle_is_ignored() {
        assert_eq!(
            classify_switcher(WM_KEYDOWN_U, VK_ESCAPE, false, false, false),
            (SwitcherAction::None, false)
        );
    }

    #[test]
    fn alt_release_while_active_commits_without_swallow() {
        assert_eq!(
            classify_switcher(WM_KEYUP_U, 0xA4, false, false, true),
            (SwitcherAction::Commit, false)
        );
    }

    #[test]
    fn alt_release_while_idle_is_ignored() {
        assert_eq!(
            classify_switcher(WM_SYSKEYUP_U, 0x12, false, false, false),
            (SwitcherAction::None, false)
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails to compile then passes logic**

Run: `cd src-tauri && cargo test keyhook::switcher_tests`
Expected: compiles and 8 tests pass.

- [ ] **Step 3: Add the session atomics + consumer API**

Near the other `static *_REQUESTED: AtomicBool` declarations in `keyhook.rs`, add (use `AtomicI32`; add the import `use std::sync::atomic::AtomicI32;`):

```rust
static SWITCHER_OPEN: AtomicI32 = AtomicI32::new(0);   // 0 none / +1 fwd / -1 rev
static SWITCHER_STEP: AtomicI32 = AtomicI32::new(0);   // net rotation delta
static SWITCHER_COMMIT: AtomicBool = AtomicBool::new(false);
static SWITCHER_CANCEL: AtomicBool = AtomicBool::new(false);
static SWITCHER_ACTIVE: AtomicBool = AtomicBool::new(false); // session in progress

/// Consume a pending open request: returns 0 (none), +1 (forward), -1 (reverse).
pub fn take_switcher_open() -> i32 { SWITCHER_OPEN.swap(0, Ordering::AcqRel) }
/// Consume and reset the net rotation delta since last poll.
pub fn take_switcher_step() -> i32 { SWITCHER_STEP.swap(0, Ordering::AcqRel) }
/// Consume a pending commit (Alt released).
pub fn take_switcher_commit() -> bool { SWITCHER_COMMIT.swap(false, Ordering::AcqRel) }
/// Consume a pending cancel (Esc).
pub fn take_switcher_cancel() -> bool { SWITCHER_CANCEL.swap(false, Ordering::AcqRel) }
/// Force the hook session flag off — called by the consumer when it declines
/// to open (e.g. zero eligible windows) so Tab/Esc stop being swallowed.
pub fn reset_switcher_session() { SWITCHER_ACTIVE.store(false, Ordering::SeqCst); }
```

- [ ] **Step 4: Wire the classifier into the unsafe callback**

Inside `callback`, after the existing `let kb = &*(l.0 as *const KBDLLHOOKSTRUCT);` and the injected-key guard, before the existing Win-key handling, add:

```rust
{
    let vk = kb.vkCode;
    let alt = (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0;
    let shift = (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0;
    let active = SWITCHER_ACTIVE.load(Ordering::SeqCst);
    let (action, swallow) = classify_switcher(msg, vk, alt, shift, active);
    match action {
        SwitcherAction::Open(d) => {
            SWITCHER_ACTIVE.store(true, Ordering::SeqCst);
            SWITCHER_OPEN.store(d, Ordering::SeqCst);
        }
        SwitcherAction::Step(d) => { SWITCHER_STEP.fetch_add(d, Ordering::SeqCst); }
        SwitcherAction::Commit => {
            SWITCHER_ACTIVE.store(false, Ordering::SeqCst);
            SWITCHER_COMMIT.store(true, Ordering::SeqCst);
        }
        SwitcherAction::Cancel => {
            SWITCHER_ACTIVE.store(false, Ordering::SeqCst);
            SWITCHER_CANCEL.store(true, Ordering::SeqCst);
        }
        SwitcherAction::None => {}
    }
    if swallow {
        return LRESULT(1);
    }
}
```

Add `VK_SHIFT` to the `windows::...::KeyboardAndMouse` import line in `keyhook.rs` (it lives next to `VK_MENU`). Confirm `msg` is already bound as `w.0 as u32` earlier in the callback; if it is named differently, use that local.

- [ ] **Step 5: Build to verify it compiles**

Run: `cd src-tauri && cargo build`
Expected: compiles (warnings about unused consumer fns are fine until Task 6 wires them).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/keyhook.rs
git commit -m "feat(switcher): Alt+Tab chord classifier + hook session signaling"
```

---

### Task 3: Window snapshot capture in `capture.rs`

**Files:**
- Modify: `src-tauri/src/capture.rs`
- Test: `src-tauri/src/capture.rs` (a live, `#[ignore]`d smoke test)

- [ ] **Step 1: Implement the capture pipeline**

Replace the contents of `src-tauri/src/capture.rs` with:

```rust
//! Window snapshot capture: PrintWindow -> 32bpp DIB -> downscaled PNG ->
//! base64 data URL. Mirrors the GDI lifecycle proven in icons.rs.

use anyhow::{anyhow, Result};
use base64::Engine;
use windows::Win32::Foundation::{HANDLE, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsIconic, PrintWindow, PRINT_WINDOW_FLAGS,
};

// PW_RENDERFULLCONTENT — capture composited/DirectComposition content (Chrome,
// Electron, UWP). Not always exported as a named const in windows 0.58.
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(0x0000_0002);

/// Capture `hwnd` to a downscaled PNG data URL whose longest side is `max_px`.
/// Returns Err for minimized/zero-size windows or when PrintWindow yields a
/// blank buffer (hardware-accelerated fullscreen) — callers fall back to the
/// app icon.
pub fn window_thumbnail_data_url(hwnd: isize, max_px: u32) -> Result<String> {
    let png = window_thumbnail_png(hwnd, max_px)?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&png)
    ))
}

fn window_thumbnail_png(hwnd: isize, max_px: u32) -> Result<Vec<u8>> {
    let h = HWND(hwnd as *mut _);
    unsafe {
        if IsIconic(h).as_bool() {
            return Err(anyhow!("minimized"));
        }
        let mut rect = RECT::default();
        GetWindowRect(h, &mut rect)?;
        let w = (rect.right - rect.left).max(0) as u32;
        let ht = (rect.bottom - rect.top).max(0) as u32;
        if w == 0 || ht == 0 {
            return Err(anyhow!("zero-size window"));
        }

        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(anyhow!("GetDC failed"));
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_invalid() {
            ReleaseDC(None, screen_dc);
            return Err(anyhow!("CreateCompatibleDC failed"));
        }

        let result = capture_into_dib(h, screen_dc, mem_dc, w, ht);

        // Matched-pair cleanup (leaking DCs exhausts GDI after ~10 captures).
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);

        let (mut rgba, cw, ch) = result?;

        // Reject all-black/empty captures (hardware-accelerated content).
        if !rgba.chunks_exact(4).any(|px| px[0] | px[1] | px[2] != 0) {
            return Err(anyhow!("blank capture"));
        }
        // PrintWindow gives opaque content; force full alpha so resize/encoding
        // doesn't multiply against garbage alpha.
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }

        let img = image::RgbaImage::from_raw(cw, ch, rgba)
            .ok_or_else(|| anyhow!("from_raw failed"))?;
        let (tw, th) = fit(cw, ch, max_px);
        let scaled = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
        let mut out = Vec::new();
        scaled.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
        Ok(out)
    }
}

/// Render `hwnd` into a fresh top-down 32bpp DIB and return (RGBA, w, h).
unsafe fn capture_into_dib(
    h: HWND,
    screen_dc: HDC,
    mem_dc: HDC,
    w: u32,
    ht: u32,
) -> Result<(Vec<u8>, u32, u32)> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(ht as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib: HBITMAP =
        CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE(std::ptr::null_mut()), 0)?;
    if dib.is_invalid() || bits.is_null() {
        return Err(anyhow!("CreateDIBSection failed"));
    }
    let prev = SelectObject(mem_dc, HGDIOBJ(dib.0 as *mut _));

    let ok = PrintWindow(h, mem_dc, PW_RENDERFULLCONTENT);

    // Copy bits out before freeing the DIB.
    let mut rgba = std::slice::from_raw_parts(bits as *const u8, (w * ht * 4) as usize).to_vec();
    SelectObject(mem_dc, prev);
    let _ = DeleteObject(HGDIOBJ(dib.0 as *mut _));

    if !ok.as_bool() {
        return Err(anyhow!("PrintWindow returned false"));
    }
    // BGRA -> RGBA
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok((rgba, w, ht))
}

/// Scale (w,h) so the longest side == max_px (never upscale).
fn fit(w: u32, h: u32, max_px: u32) -> (u32, u32) {
    let longest = w.max(h);
    if longest <= max_px || longest == 0 {
        return (w.max(1), h.max(1));
    }
    let s = max_px as f64 / longest as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_caps_longest_side() {
        assert_eq!(fit(1920, 1080, 480), (480, 270));
        assert_eq!(fit(1080, 1920, 480), (270, 480));
    }

    #[test]
    fn fit_never_upscales() {
        assert_eq!(fit(200, 100, 480), (200, 100));
    }

    /// Live smoke test — capture the current foreground window.
    /// Run explicitly: `cargo test capture::tests::live_capture -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_capture() {
        let hwnd = crate::win32::foreground_hwnd();
        match window_thumbnail_data_url(hwnd, 480) {
            Ok(url) => {
                assert!(url.starts_with("data:image/png;base64,"));
                assert!(url.len() > 1000, "thumbnail suspiciously small: {}", url.len());
                println!("captured {} byte data url", url.len());
            }
            Err(e) => println!("capture failed (acceptable for some windows): {e}"),
        }
    }
}
```

- [ ] **Step 2: Run unit tests**

Run: `cd src-tauri && cargo test capture::tests::fit`
Expected: `fit_caps_longest_side` and `fit_never_upscales` pass.

- [ ] **Step 3: Run the live smoke test manually**

Run: `cd src-tauri && cargo test capture::tests::live_capture -- --ignored --nocapture`
Expected: prints either a multi-KB data URL length or an acceptable per-window failure message; does not panic.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/capture.rs
git commit -m "feat(capture): PrintWindow window snapshot -> downscaled PNG data URL"
```

---

### Task 4: win32 helpers (`is_iconic`, monitor rect)

**Files:**
- Modify: `src-tauri/src/win32.rs`
- Test: none (thin Win32 wrappers; covered by integration)

- [ ] **Step 1: Add helpers**

Append to `src-tauri/src/win32.rs` (add imports `IsIconic` if not present, and `MonitorFromWindow`, `GetMonitorInfoW`, `MONITORINFO`, `MONITOR_DEFAULTTONEAREST` from `windows::Win32::Graphics::Gdi`):

```rust
/// True if the window is minimized.
pub fn is_iconic(hwnd: isize) -> bool {
    unsafe { windows::Win32::UI::WindowsAndMessaging::IsIconic(HWND(hwnd as *mut _)).as_bool() }
}

/// Full bounds (x, y, width, height) of the monitor containing `hwnd`.
/// Falls back to the primary monitor metrics if anything fails.
pub fn monitor_rect_for_hwnd(hwnd: isize) -> (i32, i32, i32, i32) {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let mon = MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            let r = mi.rcMonitor;
            (r.left, r.top, r.right - r.left, r.bottom - r.top)
        } else {
            (0, 0, 1920, 1080)
        }
    }
}
```

Ensure `Win32_Graphics_Gdi` feature includes monitor APIs — it does in the existing feature list. If `MonitorFromWindow` is missing, it lives under `Win32_Graphics_Gdi`.

- [ ] **Step 2: Build**

Run: `cd src-tauri && cargo build`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/win32.rs
git commit -m "feat(win32): is_iconic + monitor_rect_for_hwnd helpers"
```

---

### Task 5: Create the hidden `switcher` webview

**Files:**
- Modify: `src-tauri/src/windows_setup.rs`

- [ ] **Step 1: Add window creation**

In `windows_setup.rs`, inside the function that builds the other windows (where `dock`/`spotlight` are created), add after an existing window build, copying the established builder + helper pattern:

```rust
// 3D ring Alt+Tab switcher overlay. Fullscreen, transparent, topmost, and
// NON-ACTIVATING: all navigation comes from the global keyboard hook, so the
// overlay must never steal foreground from the window we're about to switch
// to. Sized/positioned to the active monitor at show-time in commands.rs.
let switcher = WebviewWindowBuilder::new(
    app,
    "switcher",
    WebviewUrl::App("switcher/index.html".into()),
)
.title("")
.inner_size(1280.0, 720.0)
.position(0.0, 0.0)
.decorations(false)
.transparent(true)
.background_color(Color(0, 0, 0, 0))
.always_on_top(true)
.skip_taskbar(true)
.resizable(false)
.shadow(false)
.visible(false)
.build()?;
force_webview_transparent(&switcher);
apply_noactivate(&switcher); // do NOT take focus — hook drives navigation
```

Do **not** call `apply_glass`/`clip_to_rounded` here — the overlay is fullscreen and draws its own dim/blur in WebGL. (`apply_glass` would impose a rounded acrylic card.)

- [ ] **Step 2: Build**

Run: `cd src-tauri && cargo build`
Expected: compiles. (The page doesn't exist yet — that's fine; the window is hidden and not shown until Phase B.)

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/windows_setup.rs
git commit -m "feat(switcher): create hidden transparent fullscreen overlay window"
```

---

### Task 6: Switcher session loop + show/hide + commit

**Files:**
- Modify: `src-tauri/src/switcher.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/main.rs`

- [ ] **Step 1: Add show/hide helpers to `commands.rs`**

These are plain helpers (not `#[tauri::command]` — they're called from the Rust loop). Add to `commands.rs`:

```rust
/// Position the switcher overlay over the monitor holding `anchor_hwnd`,
/// show it, re-assert topmost, and re-strip decorations (Win11 re-adds them).
pub fn show_switcher_overlay(app: &tauri::AppHandle, anchor_hwnd: isize) -> Result<(), String> {
    use tauri::{PhysicalPosition, PhysicalSize};
    let win = app
        .get_webview_window("switcher")
        .ok_or_else(|| "switcher window missing".to_string())?;
    let (x, y, w, h) = crate::win32::monitor_rect_for_hwnd(anchor_hwnd);
    win.set_position(PhysicalPosition::new(x, y)).map_err(|e| e.to_string())?;
    win.set_size(PhysicalSize::new(w as u32, h as u32)).map_err(|e| e.to_string())?;
    win.show().map_err(|e| e.to_string())?;
    win.set_always_on_top(true).map_err(|e| e.to_string())?;
    if let Ok(hwnd) = win.hwnd() {
        let hh = hwnd.0 as isize;
        crate::dwm::strip_decorations(hh);
        crate::dwm::suppress_nc_rendering(hh);
        crate::dwm::set_position_topmost(hh, x, y);
    }
    Ok(())
}

pub fn hide_switcher_overlay(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("switcher") {
        let _ = win.hide();
    }
}
```

- [ ] **Step 2: Implement the session loop in `switcher.rs`**

Add to `src-tauri/src/switcher.rs` (above the `#[cfg(test)]` module):

```rust
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::{capture, commands, keyhook, win32};

/// One window as presented to the ring UI.
#[derive(Debug, Clone, Serialize)]
pub struct SwitcherItem {
    pub id: i64,        // hwnd
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
                // Delay so our overlay's hide() fully relinquishes before we
                // foreground the target (Win11 races SetForegroundWindow vs
                // WM_KILLFOCUS otherwise — same trick as the clipboard flow).
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(90));
                    let _ = win32::focus_aggressive(hwnd);
                });
            }
        }
    }
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

    // Show overlay positioned on the anchor's monitor, then push the payload.
    if commands::show_switcher_overlay(app, anchor).is_err() {
        keyhook::reset_switcher_session();
        return;
    }

    #[derive(Serialize, Clone)]
    struct OpenPayload {
        windows: Vec<SwitcherItem>,
        selected: usize,
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
                struct Thumb { id: i64, thumb: String }
                let _ = app2.emit_to("switcher", "switcher:thumb", Thumb { id: it.id, thumb: url });
            }
        }
    });
}
```

- [ ] **Step 3: Spawn the loop in `main.rs`**

In `main.rs` `setup`, after `dock_autohide::spawn(...)`, add:

```rust
switcher::spawn(app.handle().clone());
```

- [ ] **Step 4: Build**

Run: `cd src-tauri && cargo build`
Expected: compiles. The unused-warning on consumer fns from Task 2 is now gone.

- [ ] **Step 5: Run existing tests to confirm no regressions**

Run: `cd src-tauri && cargo test`
Expected: all tests pass (Task 1 + Task 2 + Task 3 fit tests + existing suite).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/switcher.rs src-tauri/src/commands.rs src-tauri/src/main.rs
git commit -m "feat(switcher): session loop — enumerate, show, spin, commit focus, stream thumbs"
```

---

## PHASE B — The 3D ring (frontend)

### Task 7: Pure ring-layout math + node test

**Files:**
- Create: `ui/switcher/ring.js`
- Create: `ui/switcher/ring.test.mjs`

- [ ] **Step 1: Write the failing node test**

Create `ui/switcher/ring.test.mjs`:

```js
import assert from 'node:assert';
import { ringTransform, ringRadius } from './ring.js';

// Selected card sits front-and-center, facing camera (angle 0).
{
  const t = ringTransform(2, 2, 6, { radius: 5 });
  assert.ok(Math.abs(t.angle) < 1e-9, `selected angle should be 0, got ${t.angle}`);
  assert.ok(Math.abs(t.z - 5) < 1e-9, `selected should be at +radius z, got ${t.z}`);
  assert.ok(t.scale > 1.0, 'selected scale should be emphasized');
  assert.ok(Math.abs(t.opacity - 1) < 1e-9, 'selected fully opaque');
}

// Neighbours are offset by one angular step each side.
{
  const total = 8;
  const step = (2 * Math.PI) / total;
  const left = ringTransform(1, 2, total, { radius: 5 });
  const right = ringTransform(3, 2, total, { radius: 5 });
  assert.ok(Math.abs(left.angle - (-step)) < 1e-9, `left neighbour angle ${left.angle}`);
  assert.ok(Math.abs(right.angle - step) < 1e-9, `right neighbour angle ${right.angle}`);
  assert.ok(left.scale < 1.0 && right.scale < 1.0, 'neighbours smaller than selected');
  assert.ok(left.opacity < 1 && right.opacity < 1, 'neighbours dimmer');
}

// Wrap: with total=6, index 5 relative to selected 0 is one step left (-step), not +5 steps.
{
  const total = 6;
  const step = (2 * Math.PI) / total;
  const t = ringTransform(5, 0, total, { radius: 5 });
  assert.ok(Math.abs(t.angle - (-step)) < 1e-9, `wrapped angle should be -step, got ${t.angle}`);
}

// Radius scales with count so big lists don't overlap.
assert.ok(ringRadius(12) > ringRadius(4), 'radius grows with window count');

console.log('ring.test.mjs: all assertions passed');
```

- [ ] **Step 2: Run it to verify it fails**

Run: `node ui/switcher/ring.test.mjs`
Expected: FAIL — `Cannot find module './ring.js'`.

- [ ] **Step 3: Implement `ring.js`**

Create `ui/switcher/ring.js`:

```js
// Pure ring-layout math. Importable from the browser (<script type=module>)
// and from node (tests). No DOM, no Three.js dependency.

// Shortest signed angular offset (in steps) from `selected` to `i`, wrapped
// to [-total/2, total/2] so the ring takes the short way round.
function signedOffset(i, selected, total) {
  let d = i - selected;
  const half = total / 2;
  if (d > half) d -= total;
  if (d < -half) d += total;
  return d;
}

// Ring radius grows with the number of cards so a crowded ring doesn't overlap.
export function ringRadius(total) {
  return Math.max(4.5, 0.85 * total);
}

// Transform for card `i` given the currently `selected` index out of `total`.
// Returns { angle, x, z, rotY, scale, opacity, blur } in scene units.
// angle 0 == front-center (largest z, facing camera).
export function ringTransform(i, selected, total, opts = {}) {
  const radius = opts.radius ?? ringRadius(total);
  const step = (2 * Math.PI) / total;
  const off = signedOffset(i, selected, total);
  const angle = off * step;

  // Position on a circle in the XZ plane; front of ring is +z toward camera.
  const x = Math.sin(angle) * radius;
  const z = Math.cos(angle) * radius;

  // Cards face outward along the ring tangent; the front one faces the camera.
  const rotY = -angle;

  // Emphasis falls off with angular distance from the front.
  const a = Math.abs(angle);
  const selected_scale = 1.35;
  const min_scale = 0.7;
  const scale = i === selected ? selected_scale : Math.max(min_scale, 1.0 - a * 0.18);
  const opacity = i === selected ? 1.0 : Math.max(0.25, 1.0 - a * 0.35);
  const blur = i === selected ? 0 : Math.min(1, a * 0.4);

  return { angle, x, z, rotY, scale, opacity, blur };
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `node ui/switcher/ring.test.mjs`
Expected: `ring.test.mjs: all assertions passed`.

- [ ] **Step 5: Commit**

```bash
git add ui/switcher/ring.js ui/switcher/ring.test.mjs
git commit -m "feat(switcher-ui): pure ring-layout math + node test"
```

---

### Task 8: Vendor Three.js (ESM)

**Files:**
- Create: `ui/switcher/three.module.min.js`

- [ ] **Step 1: Download a pinned Three.js ESM build**

Run (PowerShell):

```powershell
Invoke-WebRequest -Uri "https://unpkg.com/three@0.160.0/build/three.module.min.js" -OutFile "ui/switcher/three.module.min.js"
```

- [ ] **Step 2: Verify it's a self-contained ESM (no bare imports)**

Run: `node -e "const s=require('fs').readFileSync('ui/switcher/three.module.min.js','utf8'); console.log('bytes',s.length); console.log('has bare import:', /from\s*[\"']three[\"']/.test(s));"`
Expected: a large byte count (~600KB+) and `has bare import: false`.

- [ ] **Step 3: Commit**

```bash
git add ui/switcher/three.module.min.js
git commit -m "chore(switcher-ui): vendor Three.js r0.160.0 ESM build"
```

---

### Task 9: Overlay page scaffold + transparent WebGL scene

**Files:**
- Create: `ui/switcher/index.html`
- Create: `ui/switcher/style.css`
- Create: `ui/switcher/app.js`

- [ ] **Step 1: Create `index.html`**

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8" />
  <title>switcher</title>
  <link rel="stylesheet" href="style.css" />
</head>
<body>
  <div id="scrim"></div>
  <canvas id="ring"></canvas>
  <div id="label"><span id="label-title"></span></div>
  <script type="module" src="app.js"></script>
</body>
</html>
```

- [ ] **Step 2: Create `style.css`**

```css
html, body {
  margin: 0;
  width: 100%;
  height: 100%;
  overflow: hidden;
  background: transparent;
  cursor: default;
  user-select: none;
}
#scrim {
  position: fixed;
  inset: 0;
  background: radial-gradient(ellipse at center, rgba(8,10,18,0.55) 0%, rgba(4,6,12,0.82) 100%);
  backdrop-filter: blur(18px) saturate(120%);
  -webkit-backdrop-filter: blur(18px) saturate(120%);
  opacity: 0;
  transition: opacity 160ms ease;
}
#scrim.show { opacity: 1; }
#ring {
  position: fixed;
  inset: 0;
  width: 100%;
  height: 100%;
  display: block;
}
#label {
  position: fixed;
  left: 0;
  right: 0;
  bottom: 9%;
  text-align: center;
  color: #f3f5ff;
  font: 600 22px/1.3 "Segoe UI", system-ui, sans-serif;
  text-shadow: 0 2px 18px rgba(0,0,0,0.7);
  opacity: 0;
  transition: opacity 140ms ease;
  pointer-events: none;
}
#label.show { opacity: 1; }
```

- [ ] **Step 3: Create `app.js` (scene bootstrap only)**

```js
import * as THREE from './three.module.min.js';
import { ringTransform, ringRadius } from './ring.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const canvas = document.getElementById('ring');
const scrim = document.getElementById('scrim');
const labelEl = document.getElementById('label');
const labelTitle = document.getElementById('label-title');

const renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true });
renderer.setClearColor(0x000000, 0); // transparent — desktop shows through scrim
renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));

const scene = new THREE.Scene();
const camera = new THREE.PerspectiveCamera(45, 1, 0.1, 100);
camera.position.set(0, 1.1, 11);
camera.lookAt(0, 0, 0);

scene.add(new THREE.AmbientLight(0xffffff, 0.85));
const key = new THREE.DirectionalLight(0xffffff, 0.7);
key.position.set(2, 4, 6);
scene.add(key);

function resize() {
  const w = window.innerWidth, h = window.innerHeight;
  renderer.setSize(w, h, false);
  camera.aspect = w / h;
  camera.updateProjectionMatrix();
}
window.addEventListener('resize', resize);
resize();

let rafRunning = false;
function startLoop() {
  if (rafRunning) return;
  rafRunning = true;
  const tick = () => {
    if (!rafRunning) return;
    renderer.render(scene, camera);
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}
function stopLoop() { rafRunning = false; }

// Exposed for Task 10+.
window.__switcherApply = (payload) => { /* filled in Task 10 */ };

// Apply any payload that arrived via eval before listeners registered.
async function init() {
  await listen('switcher:open', (e) => window.__switcherApply(e.payload));
  if (window.__switcherPending) window.__switcherApply(window.__switcherPending);
}
init();

export { THREE, scene, camera, renderer, startLoop, stopLoop, scrim, labelEl, labelTitle, ringTransform, ringRadius };
```

- [ ] **Step 4: Build and run glassbar; verify the overlay window loads without error**

Run: `cd src-tauri && cargo build` then launch the built exe (or `cargo tauri dev`).
Manual check: trigger Alt+Tab once (it will show an empty transparent overlay + scrim for now since cards aren't built yet). Open the switcher window's devtools if available, or check `%APPDATA%\glassbar` logs / the glassbar log file for JS errors. Expected: no module-load or WebGL errors; a dim scrim appears and Alt-release dismisses it.

- [ ] **Step 5: Commit**

```bash
git add ui/switcher/index.html ui/switcher/style.css ui/switcher/app.js
git commit -m "feat(switcher-ui): transparent WebGL scene scaffold + open-event wiring"
```

---

### Task 10: Build cards from the open payload

**Files:**
- Modify: `ui/switcher/app.js`

- [ ] **Step 1: Implement card construction + layout in `__switcherApply`**

Replace the placeholder `window.__switcherApply` and add helpers in `app.js`:

```js
const CARD_W = 3.2, CARD_H = 2.0;
const loader = new THREE.TextureLoader();

let cards = [];        // [{ mesh, id, item }]
let selected = 0;
let radius = ringRadius(1);

function clearCards() {
  for (const c of cards) {
    scene.remove(c.mesh);
    c.mesh.geometry.dispose();
    c.mesh.material.map?.dispose();
    c.mesh.material.dispose();
  }
  cards = [];
}

// 1x1 dark placeholder texture (data URL) until a real thumb/icon arrives.
const PLACEHOLDER =
  'data:image/svg+xml;utf8,' +
  encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="160" height="100"><rect width="100%" height="100%" rx="10" fill="%23222838"/></svg>');

function makeCard(item) {
  const geo = new THREE.PlaneGeometry(CARD_W, CARD_H);
  const mat = new THREE.MeshBasicMaterial({
    map: loader.load(PLACEHOLDER),
    transparent: true,
    toneMapped: false,
  });
  const mesh = new THREE.Mesh(geo, mat);
  scene.add(mesh);
  return { mesh, id: item.id, item };
}

function layout(animated = true) {
  const total = cards.length || 1;
  radius = ringRadius(total);
  cards.forEach((c, i) => {
    const t = ringTransform(i, selected, total, { radius });
    const target = {
      x: t.x, y: 0, z: t.z, rotY: t.rotY,
      scale: t.scale, opacity: t.opacity,
    };
    if (!animated) {
      c.mesh.position.set(target.x, target.y, target.z);
      c.mesh.rotation.y = target.rotY;
      c.mesh.scale.setScalar(target.scale);
      c.mesh.material.opacity = target.opacity;
      c.mesh.renderOrder = Math.round(target.z * 100);
    } else {
      c._target = target;
    }
  });
}

function updateLabel() {
  const c = cards[selected];
  if (c) {
    labelTitle.textContent = c.item.title || '';
    labelEl.classList.add('show');
  }
}

window.__switcherApply = (payload) => {
  if (!payload || !Array.isArray(payload.windows)) return;
  clearCards();
  cards = payload.windows.map(makeCard);
  selected = Math.min(payload.selected ?? 0, Math.max(0, cards.length - 1));
  layout(false);
  updateLabel();
  scrim.classList.add('show');
  startLoop();
  // Lazy-load app icons as the immediate texture (sharp thumbs replace later).
  cards.forEach((c) => applyIcon(c));
};

async function applyIcon(card) {
  try {
    const url = await invoke('get_icon', { exePath: card.item.exe_path, hwnd: card.id });
    if (url) setTexture(card, url, /*isIcon*/ true);
  } catch { /* keep placeholder */ }
}

function setTexture(card, url, isIcon) {
  loader.load(url, (tex) => {
    tex.colorSpace = THREE.SRGBColorSpace;
    card.mesh.material.map?.dispose();
    card.mesh.material.map = tex;
    // Icons are square; keep them centered without stretching the plane.
    card.mesh.material.needsUpdate = true;
    card._isIcon = isIcon;
  });
}
```

- [ ] **Step 2: Add the per-frame ease toward targets in the render loop**

In the `tick` function in `startLoop`, before `renderer.render(...)`, add easing:

```js
    const k = 0.22; // ease factor (snappy but smooth)
    for (const c of cards) {
      const t = c._target;
      if (!t) continue;
      c.mesh.position.x += (t.x - c.mesh.position.x) * k;
      c.mesh.position.z += (t.z - c.mesh.position.z) * k;
      c.mesh.rotation.y += (t.rotY - c.mesh.rotation.y) * k;
      const s = c.mesh.scale.x + (t.scale - c.mesh.scale.x) * k;
      c.mesh.scale.setScalar(s);
      c.mesh.material.opacity += (t.opacity - c.mesh.material.opacity) * k;
      c.mesh.renderOrder = Math.round(c.mesh.position.z * 100);
    }
```

- [ ] **Step 3: Build + run; verify cards appear in a ring**

Run the built glassbar; Alt+Tab. Expected: a ring of cards (app icons initially), front card emphasized, title label shown. Alt-release dismisses.

- [ ] **Step 4: Commit**

```bash
git add ui/switcher/app.js
git commit -m "feat(switcher-ui): build textured cards on a ring from open payload"
```

---

### Task 11: Spin on select + scrim/label lifecycle

**Files:**
- Modify: `ui/switcher/app.js`

- [ ] **Step 1: Handle `switcher:select` and dismissal**

Add to `init()` in `app.js`:

```js
  await listen('switcher:select', (e) => {
    selected = e.payload | 0;
    layout(true);
    updateLabel();
  });
  // The overlay window is hidden by Rust on commit/cancel; when it's hidden
  // the page keeps state but should reset visuals on next open. Also stop the
  // RAF loop when the document is hidden to save GPU.
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      stopLoop();
      scrim.classList.remove('show');
      labelEl.classList.remove('show');
    }
  });
```

- [ ] **Step 2: Build + run; verify spinning**

Run glassbar; hold Alt, tap Tab repeatedly and Shift+Tab. Expected: the ring eases/spins to bring the newly-selected card to front; label updates; fast tabbing retargets smoothly without queueing. Alt-release focuses the front window.

- [ ] **Step 3: Commit**

```bash
git add ui/switcher/app.js
git commit -m "feat(switcher-ui): animate ring spin on selection + lifecycle cleanup"
```

---

### Task 11b: Visual polish — floor reflection, depth fog, side-card falloff

**Files:**
- Modify: `ui/switcher/app.js`

This implements spec §6's "glossy floor reflection", depth recession, and per-card
falloff. True per-card Gaussian blur needs render targets (deferred); we approximate
depth with scene fog + darkening, which reads as the same effect at switcher speed.

- [ ] **Step 1: Add depth fog to the scene**

In `app.js`, right after `const scene = new THREE.Scene();`, add:

```js
scene.fog = new THREE.Fog(0x05070c, 9, 22); // recede + darken cards toward the back
```

- [ ] **Step 2: Give each card a mirrored reflection mesh**

Replace `makeCard` and `clearCards` in `app.js` with:

```js
function makeCard(item) {
  const geo = new THREE.PlaneGeometry(CARD_W, CARD_H);
  const tex = loader.load(PLACEHOLDER);
  const mat = new THREE.MeshBasicMaterial({ map: tex, transparent: true, toneMapped: false });
  const mesh = new THREE.Mesh(geo, mat);
  scene.add(mesh);

  // Reflection: same texture, flipped on Y, faded, sitting just below the card.
  const refMat = new THREE.MeshBasicMaterial({
    map: tex, transparent: true, opacity: 0.0, toneMapped: false,
    depthWrite: false,
  });
  const reflection = new THREE.Mesh(geo, refMat);
  reflection.scale.y = -1; // mirror
  scene.add(reflection);

  return { mesh, reflection, id: item.id, item };
}

function clearCards() {
  for (const c of cards) {
    scene.remove(c.mesh);
    scene.remove(c.reflection);
    c.mesh.geometry.dispose();
    c.mesh.material.map?.dispose();
    c.mesh.material.dispose();
    c.reflection.material.dispose();
  }
  cards = [];
}
```

- [ ] **Step 3: Apply darkening from `blur` and drive reflections in the easing loop**

In the `tick` easing loop in `startLoop`, replace the per-card block with:

```js
    const k = 0.22;
    const total = cards.length || 1;
    cards.forEach((c, i) => {
      const tr = ringTransform(i, selected, total, { radius });
      const t = c._target || tr;
      c.mesh.position.x += (t.x - c.mesh.position.x) * k;
      c.mesh.position.z += (t.z - c.mesh.position.z) * k;
      c.mesh.rotation.y += (t.rotY - c.mesh.rotation.y) * k;
      const s = c.mesh.scale.x + (t.scale - c.mesh.scale.x) * k;
      c.mesh.scale.setScalar(s);
      c.mesh.material.opacity += (t.opacity - c.mesh.material.opacity) * k;
      // Darken side cards (cheap stand-in for blur/defocus).
      const shade = 1 - (tr.blur * 0.45);
      c.mesh.material.color.setScalar(shade);
      c.mesh.renderOrder = Math.round(c.mesh.position.z * 100);

      // Reflection mirrors the card below the floor line, fading with distance.
      const floorY = -(CARD_H * 0.5) * s - 0.06;
      c.reflection.position.set(c.mesh.position.x, floorY - (CARD_H * 0.5) * s, c.mesh.position.z);
      c.reflection.rotation.y = c.mesh.rotation.y;
      c.reflection.scale.set(s, -s, s);
      c.reflection.material.color.setScalar(shade);
      c.reflection.material.opacity += ((t.opacity * 0.22) - c.reflection.material.opacity) * k;
      c.reflection.renderOrder = c.mesh.renderOrder - 1;
    });
```

(Remove the old `for (const c of cards) { ... }` easing block this replaces.)

- [ ] **Step 4: Set reflection textures alongside the main texture**

In `setTexture`, after assigning `card.mesh.material.map = tex;`, also point the
reflection at the same texture:

```js
    card.reflection.material.map = tex;
    card.reflection.material.needsUpdate = true;
```

- [ ] **Step 5: Build + run; verify the look**

Run glassbar; Alt+Tab. Expected: front card bright and sharp with a soft mirrored
reflection beneath it; side/back cards darken and recede into fog; spin still smooth.

- [ ] **Step 6: Commit**

```bash
git add ui/switcher/app.js
git commit -m "feat(switcher-ui): floor reflections, depth fog, and side-card falloff"
```

---

### Task 12: Live thumbnails replace icon placeholders

**Files:**
- Modify: `ui/switcher/app.js`

- [ ] **Step 1: Listen for `switcher:thumb` and swap the texture**

Add to `init()`:

```js
  await listen('switcher:thumb', (e) => {
    const { id, thumb } = e.payload || {};
    const card = cards.find((c) => c.id === id);
    if (card && thumb) setTexture(card, thumb, /*isIcon*/ false);
  });
```

- [ ] **Step 2: Build + run; verify thumbnails stream in**

Run glassbar; Alt+Tab. Expected: cards start as app icons, then upgrade to actual window snapshots within a moment as captures complete; minimized/protected windows keep their icon.

- [ ] **Step 3: Commit**

```bash
git add ui/switcher/app.js
git commit -m "feat(switcher-ui): swap card textures to live window snapshots as they arrive"
```

---

### Task 13: Mouse interaction (hover/scroll/click)

**Files:**
- Modify: `ui/switcher/app.js`

- [ ] **Step 1: Add raycaster hover, wheel-to-spin, click-to-commit**

Add to `app.js` (the overlay is `WS_EX_NOACTIVATE`, so it still receives mouse messages without stealing focus; clicks invoke the same commands the hook would):

```js
const raycaster = new THREE.Raycaster();
const pointer = new THREE.Vector2();

function pickIndex(ev) {
  pointer.x = (ev.clientX / window.innerWidth) * 2 - 1;
  pointer.y = -(ev.clientY / window.innerHeight) * 2 + 1;
  raycaster.setFromCamera(pointer, camera);
  const hits = raycaster.intersectObjects(cards.map((c) => c.mesh), false);
  if (!hits.length) return -1;
  return cards.findIndex((c) => c.mesh === hits[0].object);
}

window.addEventListener('mousemove', (ev) => {
  const i = pickIndex(ev);
  if (i >= 0 && i !== selected) {
    selected = i;
    layout(true);
    updateLabel();
    // Keep Rust's authoritative index in sync so Alt-release commits this one.
    invoke('switcher_set_index', { index: selected }).catch(() => {});
  }
});

window.addEventListener('wheel', (ev) => {
  if (!cards.length) return;
  const dir = ev.deltaY > 0 ? 1 : -1;
  selected = (selected + dir + cards.length) % cards.length;
  layout(true);
  updateLabel();
  invoke('switcher_set_index', { index: selected }).catch(() => {});
}, { passive: true });

window.addEventListener('click', (ev) => {
  const i = pickIndex(ev);
  if (i >= 0) {
    invoke('switcher_commit', { index: i }).catch(() => {});
  }
});
```

- [ ] **Step 2: Add the two commands in `commands.rs`**

```rust
#[tauri::command]
pub fn switcher_set_index(index: usize) {
    crate::switcher::set_index(index);
}

#[tauri::command]
pub fn switcher_commit(app: tauri::AppHandle, index: usize) {
    crate::switcher::commit_index(&app, index);
}
```

Register both in the `tauri::generate_handler![...]` list in `main.rs`.

- [ ] **Step 3: Add `set_index` / `commit_index` to `switcher.rs`**

```rust
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
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(90));
            let _ = win32::focus_aggressive(hwnd);
        });
    }
}
```

- [ ] **Step 4: Build + run; verify mouse**

Run glassbar; Alt+Tab, then (still holding Alt) move the mouse over cards (front follows), scroll to spin, click a card to switch. Expected: all three work; clicking commits and focuses.

- [ ] **Step 5: Commit**

```bash
git add ui/switcher/app.js src-tauri/src/commands.rs src-tauri/src/switcher.rs src-tauri/src/main.rs
git commit -m "feat(switcher): mouse hover/scroll/click interaction synced with session"
```

---

## PHASE C — Edge cases & verification

### Task 14: Edge cases (single window, minimized fallback, blank capture)

**Files:**
- Modify: `ui/switcher/app.js`, `src-tauri/src/switcher.rs` (only if a gap surfaces)

- [ ] **Step 1: Verify single-window behavior**

Manual: with only one eligible window open besides glassbar, Alt+Tab. Expected: ring shows one card centered; Alt-release re-focuses it; no crash, no divide-by-zero (radius/`ringRadius(1)` is finite, `initial_index(_,1)==0`).

- [ ] **Step 2: Verify minimized + protected windows fall back to icon**

Manual: minimize a window, open a protected/GPU app (a fullscreen video or game windowed). Alt+Tab. Expected: minimized window shows its app icon (capture returns Err "minimized"); GPU window shows icon if `PrintWindow` blanked. Title labels correct.

- [ ] **Step 3: Verify many windows**

Manual: open 12+ windows. Alt+Tab. Expected: ring radius grows (`ringRadius`), cards don't overlap at the front, back cards fog/dim; spinning stays smooth.

- [ ] **Step 4: Confirm the native switcher never appears**

Manual: hold Alt, tap Tab quickly several times. Expected: only the 3D ring appears — Windows' built-in switcher is fully suppressed (Tab swallowed while Alt held + session active).

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "fix(switcher): edge-case handling for single/minimized/many/GPU windows"
```

---

### Task 15: Full integration verification + regression check

**Files:** none (verification only)

- [ ] **Step 1: Run the complete Rust test suite**

Run: `cd src-tauri && cargo test`
Expected: all pass (selection math, classifier, fit).

- [ ] **Step 2: Run the JS ring test**

Run: `node ui/switcher/ring.test.mjs`
Expected: all assertions pass.

- [ ] **Step 3: Confirm no regression to existing glassbar hotkeys**

Manual: verify Win-tap (dock toggle), Win+V (clipboard), Win+X (power menu), Ctrl+Alt+Space (spotlight) all still work — the new Alt+Tab branch in the hook must not interfere with these (it only fires on `VK_TAB`/`VK_ESCAPE`/Alt-up while active).

- [ ] **Step 4: Full Alt+Tab acceptance pass**

Manual acceptance:
- Alt+Tab (tap, release) → switches to previous window.
- Alt held + multiple Tabs → ring spins forward; release commits the front card.
- Alt+Shift+Tab → reverse.
- Esc while ring open → cancels, no switch, original window keeps focus.
- Mouse hover/scroll/click during the session work.
- Overlay appears on the monitor of the active window.

- [ ] **Step 5: Final commit + branch is ready**

```bash
git add -A
git commit -m "test(switcher): full integration verification pass"
```

---

## Notes & risks

- **Alt-release vs swallowed Tab:** because the hook swallows Tab while Alt is held, Windows never starts its own Alt+Tab, so we rely on the hook's own `VK_MENU` key-up to detect commit (handled in `classify_switcher`). This is the single most important behavior to verify on real hardware.
- **`PrintWindow` on GPU apps** can return black; `capture.rs` detects the all-black case and the UI keeps the app icon. Live capture for those is a phase-2 `Windows.Graphics.Capture` upgrade (see spec §11).
- **emit_to drops:** the `switcher:open` payload also goes through a `win.eval` + `window.__switcherPending` fallback (cold-start race). `switcher:select`/`switcher:thumb` are sent repeatedly/independently and a rare drop only delays one card, so no fallback is needed there.
- **Focus race:** commit foregrounds the target after a 90 ms delay post-hide, mirroring the proven clipboard-paste timing.
- **`WS_EX_NOACTIVATE`:** correct here because navigation is entirely hook-driven; the overlay never needs keyboard focus, and not activating preserves the captured Z-order and avoids a focus round-trip on commit.
