//! Voice-PTT child-process manager.
//!
//! Spawns and supervises a long-running Python voice-ptt instance. Drives
//! it over stdin (`toggle\n`, `quit\n`) and parses JSON events back over
//! stdout (`{"type":"state",...}`, `{"type":"transcript",...}`,
//! `{"type":"error",...}`). Each parsed event is re-emitted as a Tauri
//! event (`voice:state`, `voice:transcript`, `voice:error`) so the dock
//! frontend can update its mic-button affordance.
//!
//! Restart policy: if the child dies, respawn with exponential backoff
//! capped at five failures. After that the controller enters a manual-
//! reset state — the next `toggle()` call resets the counter and tries
//! again.

use crate::config::VoiceCfg;
use crate::win32::CommandHidden;
use anyhow::{Context, Result, anyhow};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const RESPAWN_BACKOFF_SECS: &[u64] = &[1, 2, 4, 8, 10];

pub struct VoiceController {
    inner: Arc<Mutex<Inner>>,
    cfg: VoiceCfg,
}

struct Inner {
    /// Live stdin to the child. `None` while we're between spawns or after
    /// the controller gave up.
    stdin: Option<ChildStdin>,
    /// PID for diagnostics + last-resort kill on shutdown.
    child: Option<Child>,
    /// Last seen state we emitted, so toggles can be reasoned about.
    last_state: String,
}

impl VoiceController {
    /// Create a new controller. Spawns the child immediately if the config
    /// is fully populated; otherwise leaves the controller idle (the dock
    /// button stays in a `loading` state forever, signalling "not set up").
    pub fn new(app: AppHandle, cfg: VoiceCfg) -> Self {
        let inner = Arc::new(Mutex::new(Inner {
            stdin: None,
            child: None,
            last_state: "loading".into(),
        }));
        let controller = Self { inner: inner.clone(), cfg: cfg.clone() };
        if cfg.enabled && !cfg.python_exe.as_os_str().is_empty() && !cfg.script.as_os_str().is_empty() {
            spawn_supervisor(app, inner, cfg);
        } else {
            crate::glog!(
                "voice controller: not spawned (enabled={}, python_exe={:?}, script={:?})",
                cfg.enabled, cfg.python_exe, cfg.script
            );
        }
        controller
    }

    /// Send a `toggle\n` to the child. Returns Err if the child is not
    /// alive (controller not configured, or supervisor gave up).
    pub fn toggle(&self) -> Result<()> {
        if !self.cfg.enabled {
            return Err(anyhow!("voice disabled in config"));
        }
        let mut guard = self.inner.lock().unwrap();
        let Some(stdin) = guard.stdin.as_mut() else {
            return Err(anyhow!("voice-ptt child not running"));
        };
        stdin.write_all(b"toggle\n").context("write toggle to stdin")?;
        stdin.flush().ok();
        Ok(())
    }

    /// Best-effort graceful shutdown: send `quit`, wait briefly, then kill.
    pub fn shutdown(&self) {
        let mut guard = self.inner.lock().unwrap();
        if let Some(stdin) = guard.stdin.as_mut() {
            let _ = stdin.write_all(b"quit\n");
            let _ = stdin.flush();
        }
        // Drop stdin so the child sees EOF if it's blocked on stdin.
        guard.stdin = None;
        if let Some(child) = guard.child.as_mut() {
            // Give it 500ms to exit cleanly.
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
            let _ = child.kill();
        }
    }
}

/// Background thread: spawns + respawns the child, kicks off a stdout
/// reader thread for each generation. Lives for the lifetime of the app.
fn spawn_supervisor(app: AppHandle, inner: Arc<Mutex<Inner>>, cfg: VoiceCfg) {
    std::thread::spawn(move || {
        let mut fail_streak: usize = 0;
        loop {
            let exited_normally = match spawn_one(&app, &inner, &cfg) {
                Ok(()) => {
                    fail_streak = 0;
                    true
                }
                Err(e) => {
                    tracing::warn!("voice child failed: {e:#}");
                    false
                }
            };
            if exited_normally {
                // Clean exit (quit command or stdin closed) — don't respawn.
                tracing::info!("voice child exited cleanly, supervisor stopping");
                return;
            }
            fail_streak += 1;
            if fail_streak > RESPAWN_BACKOFF_SECS.len() {
                tracing::error!(
                    "voice child died {} times in a row, giving up until next manual toggle",
                    fail_streak
                );
                let _ = app.emit("voice:state", "error");
                {
                    let mut g = inner.lock().unwrap();
                    g.last_state = "error".into();
                }
                return;
            }
            let wait = RESPAWN_BACKOFF_SECS[fail_streak - 1];
            tracing::info!("voice supervisor: respawning in {wait}s (attempt {fail_streak})");
            std::thread::sleep(Duration::from_secs(wait));
        }
    });
}

/// Spawn one generation of the child, install its pipes, drain stdout
/// to Tauri events, then wait for it to exit. Returns Ok if the exit was
/// orderly (quit command), Err otherwise.
fn spawn_one(app: &AppHandle, inner: &Arc<Mutex<Inner>>, cfg: &VoiceCfg) -> Result<()> {
    let mut cmd = Command::new(&cfg.python_exe);
    cmd.arg(&cfg.script);
    if let Some(parent) = PathBuf::from(&cfg.script).parent() {
        if !parent.as_os_str().is_empty() {
            cmd.current_dir(parent);
        }
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .hidden();
    let mut child = cmd.spawn().context("spawn voice-ptt child")?;
    let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin handle"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout handle"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr handle"))?;
    let pid = child.id();
    crate::glog!("voice-ptt spawned (pid={pid})");

    {
        let mut g = inner.lock().unwrap();
        g.stdin = Some(stdin);
        g.child = Some(child);
    }

    // stderr drain: forward to glassbar's tracing log so a Python crash
    // shows up in the same place as glassbar errors.
    std::thread::spawn(move || {
        let r = BufReader::new(stderr);
        for line in r.lines().flatten() {
            tracing::info!("voice-ptt stderr: {line}");
        }
    });

    // stdout drain on a dedicated thread that parses each line as JSON
    // and re-emits as a Tauri event. Runs until the child closes stdout.
    let app_for_reader = app.clone();
    let inner_for_reader = inner.clone();
    let reader = std::thread::spawn(move || {
        let r = BufReader::new(stdout);
        for line in r.lines().flatten() {
            handle_event_line(&app_for_reader, &inner_for_reader, &line);
        }
    });

    // Wait for the child to exit by polling try_wait under a short-held
    // lock. Avoids holding the inner mutex across a blocking wait() (which
    // would deadlock the toggle path) and keeps the child handle accessible
    // to shutdown() in the meantime.
    let exit_status = loop {
        {
            let mut g = inner.lock().unwrap();
            let Some(child) = g.child.as_mut() else {
                // Shutdown cleared the child out from under us.
                return Ok(());
            };
            match child.try_wait().context("try_wait on voice-ptt child")? {
                Some(status) => break status,
                None => {}
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    // Best-effort: join the stdout reader so we don't leak it past child exit.
    let _ = reader.join();

    {
        let mut g = inner.lock().unwrap();
        g.stdin = None;
        g.child = None;
    }

    crate::glog!("voice-ptt exited (pid={pid}, status={exit_status:?})");
    if exit_status.success() {
        Ok(())
    } else {
        Err(anyhow!("voice-ptt non-zero exit: {exit_status:?}"))
    }
}

fn handle_event_line(app: &AppHandle, inner: &Arc<Mutex<Inner>>, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        tracing::warn!("voice-ptt non-json stdout: {line}");
        return;
    };
    let ty = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match ty {
        "state" => {
            let state = value.get("state").and_then(|v| v.as_str()).unwrap_or("idle");
            {
                let mut g = inner.lock().unwrap();
                g.last_state = state.to_string();
            }
            crate::glog!("voice state -> {state}");
            let _ = app.emit("voice:state", state);
        }
        "transcript" => {
            let text = value.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let _ = app.emit("voice:transcript", text);
        }
        "error" => {
            let msg = value.get("message").and_then(|v| v.as_str()).unwrap_or("");
            tracing::warn!("voice-ptt error event: {msg}");
            let _ = app.emit("voice:error", msg);
        }
        other => {
            tracing::debug!("voice-ptt unknown event type: {other}");
        }
    }
}
