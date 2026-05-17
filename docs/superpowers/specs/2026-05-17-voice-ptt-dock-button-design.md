# Voice PTT Dock Button — Merge Design

**Date:** 2026-05-17
**Status:** Approved, implementing
**Owner:** pikammmmm

## Summary

Replace voice-ptt's global `\` hold-to-talk hotkey with a click-to-toggle mic button in glassbar's dock. glassbar becomes the parent process, spawns voice-ptt as a managed Python child, and drives it over stdin/stdout. Only glassbar autostarts; voice-ptt loses its keyhook and autostart shortcut entirely.

## Goals

- One click in the dock starts/stops recording. No global hotkey.
- voice-ptt model stays warm — no per-click cold-start cost.
- Single autostart entry (glassbar) — voice-ptt is no longer user-facing as a separate app.
- No TCP, no port, no IPC framework — just stdin/stdout pipes between parent and child.

## Non-goals

- Rust port of Whisper (faster-whisper / Python stays).
- Bundling Python into the glassbar installer (config points at existing Python install).
- Push-to-talk via mouse hold (toggle only).
- Per-mic-button visual transcript preview beyond a brief state flash.
- Multiple simultaneous record sessions.

## Architecture

```
┌─ glassbar (Rust + Tauri) ─────────────────────────────┐
│  Dock UI: mic button ──invoke──► voice_toggle command  │
│                                       │                 │
│                                       ▼                 │
│                              VoiceController            │
│                              (spawns + owns pipes)      │
│                                  │      ▲               │
│                          stdin "toggle" │ stdout JSON   │
└──────────────────────────────────┼──────┼───────────────┘
                                   ▼      │
┌─ voice-ptt (Python child) ───────────────┼─────────────┐
│  stdin reader ─► state machine ─► Whisper / type / beep│
│                       events ────────────┘              │
└────────────────────────────────────────────────────────┘
```

- glassbar is parent; voice-ptt is a Python child process.
- stdin carries one-line commands: `toggle\n`, `quit\n`.
- stdout carries one JSON event per line.
- stderr is appended to a glassbar-managed file (replaces voice-ptt's own log).

## Components

### voice-ptt changes (`voice_ptt.py`)

**Delete:**
- `win32_event_filter` and the pynput `keyboard.Listener` for `\`.
- Hold-threshold timer + `handle_down` / `handle_up`.
- Auto-startup Startup-folder shortcut creator (and its run.ps1 silent-launch flow becomes unused).

**Add:**
- `stdin_command_loop()` — blocking thread reading `sys.stdin` line by line; dispatches `toggle` and `quit`.
- `emit(event_dict)` — `print(json.dumps(event_dict), flush=True)`; called on every state change, transcript, and error.

**Keep unchanged:**
- Whisper model warm-up at startup.
- `transcribe_local()` / `transcribe_api()` pipeline + OpenAI fallback.
- `type_text()` (pynput `Controller().type()` — that's just SendInput, not a hook).
- Beeps, config loader, log file rotation.

**Simplified state machine:** `idle ⇄ recording → transcribing → idle`. No tap-vs-hold branching.

**Event schema (stdout, one JSON object per line):**
- `{"type": "state", "state": "loading" | "idle" | "recording" | "transcribing" | "error"}`
- `{"type": "transcript", "text": "..."}`
- `{"type": "error", "message": "..."}`

### glassbar changes (`custom-taskbar/`)

**New file** `src-tauri/src/voice.rs`:
- `pub struct VoiceController` — owns `Mutex<Option<ChildStdin>>`, child handle, respawn state.
- `pub fn spawn(app: &AppHandle, cfg: &VoiceConfig) -> VoiceController` — launches child, kicks off stdout reader thread.
- `pub fn toggle(&self) -> Result<()>` — writes `toggle\n` to stdin.
- `pub fn shutdown(&self)` — writes `quit\n`, kills after 1s.
- Internal: stdout reader thread parses JSON, emits Tauri events `voice:state` and `voice:transcript`.
- Internal: child-death watcher respawns with exponential backoff (1s, 2s, 4s, 8s, 10s). After 5 fails, emits `state: "error"` and stops respawning until next manual toggle.

**Edit `src-tauri/src/commands.rs`:**
```rust
#[tauri::command]
pub fn voice_toggle(state: tauri::State<'_, VoiceController>) -> Result<(), String> {
    state.toggle().map_err(|e| e.to_string())
}
```

**Edit `src-tauri/src/main.rs`:**
- Construct `VoiceController` in `setup`, register as Tauri-managed state.
- Register `voice_toggle` in `invoke_handler`.
- On window-close-requested for the last window, call `shutdown()`.

**Edit `src-tauri/src/config.rs`:**
```rust
#[derive(Deserialize, Default)]
pub struct VoiceCfg {
    pub enabled: bool,           // default true
    pub python_exe: PathBuf,     // e.g. C:\Python313\pythonw.exe
    pub script: PathBuf,         // e.g. C:\Users\<user>\voice-ptt\voice_ptt.py
}
```
Add `pub voice: VoiceCfg` to the root config struct.

**Edit dock frontend** (HTML/CSS/JS for the dock window):
- Add `<button id="voice-mic" class="dock-mic" data-state="loading">🎙</button>` to the dock row.
- CSS: `.dock-mic[data-state="recording"]` pulses red; `loading` is dim; `error` is solid red; `idle` is neutral.
- JS: `button.addEventListener('click', () => invoke('voice_toggle'))`.
- JS: `listen('voice:state', e => button.dataset.state = e.payload)`.

### Autostart changes

- voice-ptt: remove the Startup-folder `.lnk` creator and the creator code path. To prevent the old shortcut from re-launching voice-ptt as a sibling of glassbar's child, glassbar's `autostart.rs` setup deletes the `voice-ptt.lnk` from the user's Startup folder (idempotent — no-op if not present).
- glassbar: no change — its existing `autostart.rs` already covers the unified app.

### Distribution

- Config-pointer approach: glassbar's `config.json` names the Python interpreter and `voice_ptt.py` path. voice-ptt directory stays separate on disk; user maintains it via pip.
- If `voice.enabled = false` or paths invalid: dock shows no mic button. No errors, no spawn attempt.

## Data flow

1. User clicks mic button → dock JS `invoke('voice_toggle')`.
2. Rust `voice_toggle` command → `VoiceController::toggle()` → writes `toggle\n` to child stdin.
3. Python `stdin_command_loop` reads line → toggles state machine.
4. Python emits state events (and transcript event on success) to stdout.
5. Rust stdout reader thread parses JSON, emits Tauri event to dock window.
6. Dock JS listener updates button `data-state`; CSS reflects it.

## Error handling

- **Child dies:** respawn with backoff (1s, 2s, 4s, 8s, 10s). After 5 consecutive deaths, stop respawning and emit `state: "error"`. Next user click resets the counter and tries again.
- **Mic open / Whisper throws:** Python catches, emits `{"type": "error", "message": "..."}`, returns to `idle`. glassbar flashes button red for 2s.
- **Empty transcript:** voice-ptt silently returns to `idle` (matches current intentional-silence behavior).
- **Whisper model loading at startup:** voice-ptt emits `state: "loading"` immediately, then `state: "idle"` once model is warm. Button is disabled (via CSS) while `loading`.
- **glassbar shutdown:** `VoiceController::shutdown()` writes `quit\n`, waits up to 1s, then `child.kill()`.

## Testing (manual)

- Build glassbar, launch → dock shows mic button, button starts in `loading`, transitions to `idle` after ~3s.
- Focus a text field → click mic → button pulses red → speak → click mic → text types into the field within ~1s of click.
- Kill `python.exe` mid-recording → glassbar respawns within 2s, button returns to `idle`.
- Type `\` in any window → literal `\` appears (no special behavior).
- Set `voice.enabled = false` in config → no mic button, no child spawned, glassbar otherwise normal.

## Out of scope for this change

- Mic-button position customization (hardcoded to dock-end, after pinned apps).
- Settings UI for voice config (edit JSON).
- Streaming partial transcripts.
- Multiple language switching from the dock.
- Bundling Python with glassbar installer.
