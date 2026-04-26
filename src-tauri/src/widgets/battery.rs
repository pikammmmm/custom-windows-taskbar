use serde::Serialize;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct BatteryState {
    /// `true` when this machine has a battery — false on desktops, in which
    /// case the HUD hides the chip entirely.
    pub present: bool,
    pub charging: bool,
    /// 0–100. `None` only when the OS reports `BATTERY_LIFE_PERCENT_UNKNOWN`.
    pub percent: Option<u8>,
}

pub fn current() -> BatteryState {
    let mut status = SYSTEM_POWER_STATUS::default();
    let ok = unsafe { GetSystemPowerStatus(&mut status) }.is_ok();
    if !ok {
        return BatteryState::default();
    }
    // BatteryFlag bit 7 (0x80) = "no system battery". On laptops without a
    // battery currently inserted, BatteryLifePercent is also 255 (unknown).
    let no_battery = status.BatteryFlag & 0x80 != 0;
    if no_battery {
        return BatteryState::default();
    }
    let percent = if status.BatteryLifePercent == 255 {
        None
    } else {
        Some(status.BatteryLifePercent.min(100))
    };
    // ACLineStatus: 0 = offline, 1 = online (charging), 255 = unknown.
    let charging = status.ACLineStatus == 1;
    BatteryState { present: true, charging, percent }
}
