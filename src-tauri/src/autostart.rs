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
