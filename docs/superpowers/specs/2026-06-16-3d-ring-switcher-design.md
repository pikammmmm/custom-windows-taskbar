# 3D Ring Switcher — Design

**Date:** 2026-06-16
**Status:** Approved (design), pending implementation plan
**Home:** New `switcher` feature inside glassbar (single process, single keyboard hook)

## 1. Overview

Replace Windows' built-in Alt+Tab with a 3D **spinning ring** of live window
snapshots. Holding **Alt** and tapping **Tab** opens a fullscreen, transparent,
dimmed overlay showing every alt-tab-eligible window as a textured card arranged
around a horizontal ring (turntable). Each **Tab** spins the ring forward one
slot; **Shift+Tab** spins it back. The front-facing, camera-facing card is the
current selection. Releasing **Alt** commits — the overlay disappears and the
selected window is brought to the foreground. **Esc** cancels with no switch.

Goal: it must *feel* like a real Alt+Tab replacement (MRU ordering, hold-Alt /
tap-Tab muscle memory, instant open) while looking dramatically better — real
3D perspective, depth, reflection, and a satisfying spin.

## 2. Decisions

| Question | Decision |
| --- | --- |
| Visual style | Spinning ring / turntable carousel |
| Trigger | Replace the real Alt+Tab (low-level hook swallows it) |
| Project home | New feature module inside glassbar |
| Rendering tech | Three.js (WebGL) inside a transparent Tauri webview |
| Thumbnail source | `PrintWindow(PW_RENDERFULLCONTENT)` snapshot on open (v1) |

### Approaches considered

- **Rendering:** Three.js (chosen) vs. CSS-3D transforms (no reflections/
  lighting, degrades past ~15 cards) vs. native `wgpu` overlay (fastest, but
  large effort and no reuse of the existing Tauri webview). Three.js gives real
  perspective/lighting/reflection with full reuse of glassbar's webview stack.
- **Thumbnails:** `PrintWindow` snapshot (chosen — captures occluded/background
  windows, simple, fast enough for a momentary UI) vs. DWM live thumbnails
  (cannot be composited into a 3D WebGL scene — they draw on their own layer, so
  they are fundamentally incompatible with a true ring) vs.
  `Windows.Graphics.Capture` (modern, GPU, handles hardware-accelerated apps and
  enables a *live* front card — deferred to phase 2 for async complexity).

## 3. Architecture & data flow

```
 Alt+Tab chord                       capture + enumerate
  (keyboard) ──► keyhook.rs ──► switcher.rs ──► PrintWindow snapshots
                  (state machine)    │                │
                                     ▼                ▼
                          Tauri "switcher" webview  app_actions.rs
                          (Three.js ring)           (foreground on commit)
```

- **`keyhook.rs` (extend existing):** glassbar already runs one
  `WH_KEYBOARD_LL` hook with its own message loop. Extend it to detect the
  Alt+Tab chord and drive a small **input state machine** (the only new logic
  worth heavy unit-testing). It never renders or focuses anything; it emits
  intent (open / advance / retreat / cancel / commit) to `switcher.rs`.
- **`switcher.rs` (new):** owns the session. On open it enumerates eligible
  windows (reusing the `win32` enumerator + `app_tracker` grouping/filtering),
  captures thumbnails, builds the payload, and shows a dedicated Tauri
  `WebviewWindow` named `switcher`. The webview is transparent, borderless,
  fullscreen on the active monitor, top-most, and **`WS_EX_NOACTIVATE`** —
  showing it must NOT steal focus, or it would corrupt the z-order we just
  captured for MRU ordering.
- **Webview (new UI):** receives `open({ windows, selected })`, renders the
  ring with Three.js, and on each `select(index)` event animates the spin. The
  webview is purely presentational — **it never decides focus**.
- **Commit path:** on Alt-up the webview just closes; **Rust** calls glassbar's
  existing window-activation routine (`app_actions.rs`) to foreground the chosen
  `hwnd`, reusing its proven foreground-lock handling.

### IPC payload (Rust → webview)

```
open: {
  windows: [
    { id: <hwnd as i64>, title: String, app: String,
      thumb: "data:image/png;base64,...", isMinimized: bool }
  ],
  selected: <index>
}
select: { index: <usize> }
close:  {}   // cancel — no commit
```

Thumbnails are sent as data-URLs for v1. If the IPC payload becomes heavy
(many large windows), fall back to writing PNGs to a temp dir and serving via a
custom `asset://`-style protocol.

## 4. Input state machine (in the hook)

State: `alt_down: bool`, `session: Option<SwitcherSession>`.

| Event | Condition | Action | Swallow key? |
| --- | --- | --- | --- |
| Alt down | — | `alt_down = true` | no (Alt passes) |
| Tab down | `alt_down && !session` | open session, **pre-select index 1** | **yes** |
| Tab down | `alt_down && session` | advance selection (+1, wrap) | **yes** |
| Shift+Tab down | `alt_down && session` | retreat selection (−1, wrap) | **yes** |
| Esc down | `session` active | cancel: close, no commit | **yes** |
| Alt up | `session` active | commit selected, close | no |
| Alt up | no session | — | no |

- **Pre-select index 1** so a quick Alt+Tab tap switches to the *previous*
  window — exact real-Alt+Tab muscle memory.
- Tab / Shift+Tab are swallowed (`return LRESULT(1)`) only while a session is
  active, so Windows' native switcher never appears. The Alt key itself always
  passes through to apps.
- The hook callback must stay tiny and non-blocking. Enumeration + capture +
  webview show happen off the hook thread (signal via atomics/channel, like the
  existing `take_*_request()` pattern), so the hook returns immediately.

## 5. Window list & ordering

Standard alt-tab eligibility filter, applied during enumeration:

- top-level, `IsWindowVisible`
- non-empty title
- is an app window: `WS_EX_APPWINDOW`, **or** (no owner **and** not
  `WS_EX_TOOLWINDOW`)
- not DWM-cloaked (`DWMWA_CLOAKED == 0`) — excludes other virtual desktops and
  suspended UWP apps

Ordered by **MRU / Z-order**: current foreground window first, then top-down
Z-order. Combined with pre-select index 1, this reproduces the native feel.
Minimized windows are included (they are alt-tab-eligible).

## 6. The ring (Three.js)

- **Layout:** N cards on a circle of radius `R` in the XZ plane, evenly spaced at
  `angle = baseAngle + i * (2π / N)`. The whole ring's `baseAngle` animates so
  the selected card lands at the front (`angle = 0`), facing the camera.
- **Front card:** scaled up, fully opaque, sharp, with a glossy floor reflection
  and a soft drop shadow. A title + app-icon label sits beneath it.
- **Side cards:** rotate tangentially to the ring, recede in Z, dim, and apply a
  slight blur for depth — perspective fog toward the back.
- **Spin:** selection change eases the ring rotation with a spring (e.g. damped
  lerp), with a hint of motion blur. Snappy enough to keep up with fast Tabbing
  (animations interrupt/retarget, never queue).
- **Backdrop:** a dark, blurred scrim dimming the desktop for focus.
- **Mouse (bonus):** hover a card to select, scroll wheel to spin, click to
  commit.
- **Layout math is unit-testable:** `index → (angle, position, scale, opacity)`
  is a pure function tested independently of the renderer.

## 7. Commit, cancel, focus

- **Commit (Alt-up):** webview closes; Rust foregrounds the selected `hwnd` via
  `app_actions.rs`. If the window is minimized, restore it first. Foreground-
  lock is relaxed because we act from within input processing; fall back to the
  `AttachThreadInput` trick glassbar already uses.
- **Cancel (Esc):** close the overlay, change nothing.

## 8. Edge cases

- **0 windows:** no-op (don't open).
- **1 window:** show it; commit simply re-focuses it.
- **Many windows (20+):** spacing tightens around the full circle; only the
  front arc renders sharply, the back fogs out — no overflow.
- **Minimized / blank capture:** `PrintWindow` may return blank for minimized or
  protected windows → fall back to a large app-icon card with the title.
- **Multi-monitor:** overlay shown on the monitor holding the current foreground
  window; ring centered there.
- **Rapid Alt+Tab tap (open+commit within one frame):** the state machine must
  resolve a commit even if the webview hasn't finished its first paint — Rust
  holds the authoritative selection index, independent of render state.

## 9. Testing

- **Rust unit tests:** the input state machine (event sequences → action +
  swallow decisions, incl. pre-select-1, wrap-around, cancel, fast tap); the
  eligibility filter and MRU ordering (table-driven over synthetic window sets).
- **JS unit test:** the ring layout math (`index → angle/position/scale`).
- **Manual verification:** build the real glassbar exe and confirm Alt+Tab opens
  the ring, Tab/Shift+Tab spin it, Alt-up focuses the right window, Esc cancels,
  and the native switcher never appears.

## 10. Build & verify

- Iterate using glassbar's existing build (`cargo tauri build` / dev run) and the
  deployed exe.
- Verification is behavioral: real Alt+Tab spins the ring and commits focus to
  the front card; no regressions to glassbar's existing Win-key hook behavior.

## 11. Out of scope (phase 2+)

- Live `Windows.Graphics.Capture` thumbnails (live front card, GPU-accelerated
  apps).
- Per-app / window-filter settings, themes, ring-vs-grid toggle.
- Virtual-desktop awareness (show/switch across desktops).
- Touch / gesture input.
