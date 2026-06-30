# HUD System Tray Mirror — Design Spec (v2: Shell_NotifyIcon interception)

**Date:** 2026-06-30
**Project:** glassbar (custom Windows taskbar) — `%USERPROFILE%\custom-taskbar\`
**Status:** Approved + spike-proven; ready for implementation
**Branch:** `feat/hud-system-tray-mirror`

## Goal

Add a collapsible "System Tray" section to the glassbar HUD that mirrors the
Windows notification-area icons (normally on the real taskbar, which glassbar
hides) and lets the user act on each: activate, open the app's own menu, quit,
force-quit.

## Decisions (locked)

- **Interaction: Hybrid** — left-click activates the owning app; right-click
  forwards so the app's own native menu pops; HUD glass menu also offers
  explicit actions.
- **Quit: graceful + force** — `WM_CLOSE` then `TerminateProcess`.
- **Scope:** all registered tray icons (interception gives the full set; the
  "visible vs hidden overflow" distinction no longer applies).

## ⚠️ Approach changed from v1 — READ THIS

**v1 (toolbar scraping via `TB_GETBUTTON` + `ReadProcessMemory`) is DEAD on this
OS.** A probe of the live tray tree on Windows 11 build 26200 showed: `Shell_TrayWnd`
contains only XAML hosts (`Windows.UI.Core.CoreWindow`,
`Windows.UI.Composition.DesktopWindowContentBridge`) plus an **empty**
`TrayNotifyWnd` — there is **no `SysPager`, no `ToolbarWindow32`, and no
`NotifyIconOverflowWindow`**. The notification area is now pure XAML/WinUI;
nothing to scrape.

**v2 (this spec): become the notification area via `Shell_NotifyIcon`
interception.** This is the established shell-replacement technique
(CairoShell / ManagedShell) and it fits glassbar already replacing the taskbar.

### Spike evidence (proven in a live spike, 2026-06-30)

A standalone harness registered a window class `Shell_TrayWnd`, created a hidden
topmost window, and broadcast `TaskbarCreated`:
- `FindWindow("Shell_TrayWnd")` returned **our** window (beat explorer) →
  `routedToUs=True`.
- In 8s it captured **31 `WM_COPYDATA` (dwData=1)** messages: 13 NIM_ADD,
  6 NIM_MODIFY, 8 NIM_DELETE, 4 NIM_SETVERSION — with real tooltips from the
  running tray apps (chat, gaming, AV, etc.).
- Each message buffer was **1484 bytes** (`SHELLTRAYDATA` incl. 64-bit
  `NOTIFYICONDATAW` on this build).
- Cleanup re-broadcast `TaskbarCreated` → explorer reclaimed the icons.

## Architecture

### Tray-host (new core), `src-tauri/src/tray_host.rs`

Replaces v1's `tray_mirror.rs` reader. A dedicated **thread with its own message
loop** that:

1. Registers window class `Shell_TrayWnd` (per-process; coexists with
   explorer's), creates a hidden `WS_POPUP` top-level window with
   `WS_EX_TOPMOST | WS_EX_TOOLWINDOW`, and `SetWindowPos(HWND_TOPMOST)` so the
   shell's `FindWindow("Shell_TrayWnd")` returns **our** window.
2. On startup, `SendNotifyMessage(HWND_BROADCAST, RegisterWindowMessage("TaskbarCreated"))`
   so every app re-registers its tray icon to us.
3. WndProc handles `WM_COPYDATA` where `cds.dwData == 1`, parsing
   `SHELLTRAYDATA { DWORD dwUnknown; DWORD dwMessage; NOTIFYICONDATAW nid; }`:
   - `dwMessage`: 0=NIM_ADD, 1=NIM_MODIFY, 2=NIM_DELETE, 3=NIM_SETFOCUS,
     4=NIM_SETVERSION. Return `TRUE` (1) from the handler.
   - Maintain a live map keyed by **(owner hWnd, uID)** of:
     `hWnd, uID, uFlags, uCallbackMessage, hIcon, szTip, uVersion`.
   - NIM_SETVERSION stores the per-icon version (drives v4 vs legacy clicks).
   - Also handle `WM_COPYDATA` `dwData == 0` (APPBAR) by passing through; ignore
     balloon/NIN_* for now.
4. **Verify struct offsets empirically** (do NOT hardcode blindly): replicate the
   spike's capture in Rust first and log raw bytes via `tray_diagnostics()` to
   confirm field offsets in the 1484-byte buffer. Known `NOTIFYICONDATAW` field
   order (64-bit): `cbSize(4), pad(4), hWnd(8), uID(4), uFlags(4),
   uCallbackMessage(4), pad(4), hIcon(8), szTip[128 WCHAR], dwState(4),
   dwStateMask(4), szInfo[256 WCHAR], union{uTimeout/uVersion}(4),
   szInfoTitle[64 WCHAR], dwInfoFlags(4), guidItem(16), hBalloonIcon(8)`.
   `SHELLTRAYDATA.dwMessage` is at offset 4; `nid` begins at offset 8.

### Icon extraction

`NOTIFYICONDATA.hIcon` is owned by the calling process, but **icon handles are
valid cross-process within the session** → `CopyIcon` then `DrawIconEx` into a
32-bit DIB → PNG → `data:` URL. Reuse glassbar's existing `hicon_to_png` /
`icon_handle_to_data_url`. On NIM_MODIFY the icon may change → re-render.
Fallback chain: foreign-handle fail → owner exe icon (existing `get_icon` path)
→ generic glyph.

### Commands (unchanged surface — frontend wiring already built in v1)

`list_tray_icons`, `tray_activate`, `tray_open_menu`, `tray_quit`,
`tray_force_quit`, `tray_diagnostics`. Keep signatures so `ui/hud` needs no
rewire.

### Events

Emit `tray:changed(Vec<TrayIcon>)` (debounced ~150ms) on any add/modify/delete.
Unlike v1, the host runs continuously (it must, to keep the map live) — it is NOT
gated on HUD visibility. The frontend just consumes the latest list.

### Actions (hybrid; use the stored per-icon `uVersion`)

- **Activate (left-click):** `PostMessage(owner, uCallbackMessage, wParam, lParam)`.
  - v4 (`NOTIFYICON_VERSION_4`): `wParam = MAKEWPARAM(anchorX, anchorY)`
    (cursor/0), `lParam = MAKELPARAM(NIN_SELECT, uID)`.
  - legacy: `wParam = uID`, `lParam = WM_LBUTTONDOWN` then `WM_LBUTTONUP`.
  - Final fallback: `SetForegroundWindow(owner)`.
- **Right-click → app menu:** `SetForegroundWindow(owner)` workaround (glassbar's
  HUD is `WS_EX_NOACTIVATE`; reuse `win32::force_foreground` / AttachThreadInput),
  then forward `WM_CONTEXTMENU` (v4) / `WM_RBUTTONUP` (legacy); post `WM_NULL`
  after so `TrackPopupMenu` dismisses cleanly.
- **Quit:** `win32::close` (`WM_CLOSE`). **Force quit:** `win32::terminate_process_of`.

## Coexistence (CRITICAL — this is the integration risk)

- glassbar already hides explorer's `Shell_TrayWnd` and **re-asserts every 3s**
  via `shell_taskbar.rs` using `FindWindow("Shell_TrayWnd")`. That code MUST be
  updated to **exclude glassbar's own tray-host HWND** (store it; skip it during
  enumerate/hide), so it keeps hiding *explorer's* window, not ours. Without this,
  glassbar will fight itself and flash the real taskbar (observed in the spike).
- Our tray-host must stay the window `FindWindow` returns (topmost). The spike
  confirmed topmost wins over explorer.
- **On glassbar shutdown / Drop:** `DestroyWindow` + re-broadcast `TaskbarCreated`
  so explorer reclaims the icons. Best-effort on panic/abnormal exit too.
- Only one tray-host instance (glassbar is already singleton-guarded).

## Frontend (`ui/hud/`) — already built in v1, keep as-is

`<section class="block tray-block">` with `.apps-header` toggle, `.tray-grid` of
icon buttons (`<img>` + tooltip), glass right-click menu (Activate / Open app
menu / Quit / Force quit), empty + "Tray unavailable" states. Matches
`ui/shared/glass.css`. listens `tray:changed`, invokes the same commands.

## Error handling / safety

- Never panic across FFI; every fallible call degrades (drop icon + log).
- Malformed/oversized `WM_COPYDATA` buffers: bounds-check before parse.
- **PII:** `szTip` and exe paths can contain the user's real name/home path
  (spike saw a real first name and `%USERPROFILE%\...`). NEVER write those into
  committed code, comments, or test fixtures. `debug.log` is local-only and fine.

## Testing

- **Manual (primary):** build, run, open HUD → tray block shows the same icons as
  the real tray (every icon the stock tray shows), correct
  tooltips; left-click activates; right-click shows the app's own menu; Quit
  closes; Force quit kills. Confirm explorer reclaims icons after glassbar exit.
- **`tray_diagnostics()`** dumps the live map + raw `SHELLTRAYDATA` bytes to
  `debug.log` for offset verification.
- Unit tests for pure parts: `id` formatting, struct-offset parse helpers,
  v4/legacy click-encoding helpers.

## Out of scope (now)

- Balloon/toast notification rendering.
- Drag-reordering tray icons.
- glassbar registering its own tray icon.

## Files

- New: `src-tauri/src/tray_host.rs` (replaces v1 `tray_mirror.rs` core).
- Remove/repurpose: v1 `tray_mirror.rs` toolbar-scraping logic.
- Edit: `src-tauri/src/main.rs` (spawn host thread on setup; keep command
  registration), `src-tauri/src/commands.rs` (commands now read the host's map;
  drop the HUD-visibility gating of the poller), `src-tauri/src/shell_taskbar.rs`
  (exclude our tray-host HWND from taskbar-hide), `src-tauri/src/icons.rs`
  (already has `icon_handle_to_data_url`).
- Keep: `ui/hud/*` (v1 frontend), `Cargo.toml` (`Win32_System_Diagnostics_Debug`
  already added; add `Win32_UI_Shell` notify-icon structs if needed).
