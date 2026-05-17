use anyhow::Result;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "glassbar";

pub fn enable() -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy().to_string();
    let run = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE)?;
    run.set_value(VALUE, &exe_str)?;
    Ok(())
}

pub fn disable() -> Result<()> {
    let run = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE)?;
    let _ = run.delete_value(VALUE);
    Ok(())
}

pub fn is_enabled() -> bool {
    let Ok(run) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(RUN_KEY) else { return false; };
    run.get_value::<String, _>(VALUE).is_ok()
}

/// Delete any leftover `Voice PTT.lnk` from the user's Startup folder.
/// Old voice-ptt installs added themselves to autostart directly; now that
/// glassbar owns voice-ptt as a child process, that shortcut would launch
/// a second, hotkey-driven instance alongside glassbar's managed one.
/// Idempotent: no-op if the shortcut isn't there.
pub fn cleanup_legacy_voice_ptt_autostart() {
    let Some(appdata) = std::env::var_os("APPDATA") else { return };
    let lnk = std::path::PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("Voice PTT.lnk");
    if lnk.exists() {
        if let Err(e) = std::fs::remove_file(&lnk) {
            tracing::warn!("failed to remove legacy Voice PTT.lnk: {e}");
        } else {
            tracing::info!("removed legacy Voice PTT.lnk autostart");
        }
    }
}
