use anyhow::Result;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "glassbar";

/// This executable's absolute path, wrapped in quotes. Windows parses a Run
/// value as a command line, so an unquoted path that contains spaces (a future
/// `%LOCALAPPDATA%\Glass Bar\` or `Program Files\glassbar\` install) would be
/// split at the first space and fail to launch. Quoting makes the whole path
/// the program to run regardless of where the binary lives.
fn quoted_exe() -> Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\"", exe.to_string_lossy()))
}

/// Register glassbar to launch at user login. Idempotent and self-healing: it
/// always (over)writes the value to *this* binary's current location, so a
/// binary that was moved, renamed, or reinstalled repairs its own stale
/// Run-key entry on the next launch instead of silently failing to start.
pub fn enable() -> Result<()> {
    // create_subkey opens-or-creates with write access; the Run key always
    // exists for HKCU but this is bulletproof against a wiped hive.
    let (run, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
    run.set_value(VALUE, &quoted_exe()?)?;
    Ok(())
}

pub fn disable() -> Result<()> {
    let run = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE)?;
    let _ = run.delete_value(VALUE);
    Ok(())
}

pub fn is_enabled() -> bool {
    let Ok(run) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(RUN_KEY) else { return false; };
    run.get_value::<String, _>(VALUE).is_ok()
}
