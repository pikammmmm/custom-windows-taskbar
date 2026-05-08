//! CPU temperature probe.
//!
//! Windows doesn't expose CPU temp through any single user-mode API. The
//! options are roughly:
//!   • WMI's `MSAcpi_ThermalZoneTemperature` in `root/wmi` — works
//!     without admin on most laptops/desktops, returns ACPI thermal zone
//!     temps in tenths of Kelvin. Reflects the kernel's view of the CPU
//!     temp; a reasonable approximation of "the chip is X hot."
//!   • Vendor-specific APIs (Ryzen Master, Intel XTU, etc.) — accurate
//!     but require either admin or a vendor-specific driver.
//!   • OpenHardwareMonitor / LibreHardwareMonitor — accurate, but the
//!     user must install and run a separate service.
//!
//! We go with the first option — works out of the box, no admin, no
//! third-party deps. On systems that don't expose ACPI thermal zones we
//! report `None` and the HUD shows a placeholder.
//!
//! The probe runs on its own background thread polling every 10s.
//! That's slow enough that the PowerShell startup overhead (~500 ms)
//! doesn't matter, and CPU temp doesn't change fast enough for higher
//! frequency to be useful.

use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Sentinel value stored in the atomic when the probe hasn't yet seen a
/// reading, or the system doesn't expose ACPI thermal zones. Picked so
/// it's distinguishable from any real temperature even after a
/// long-running session.
const NO_READING: i32 = i32::MIN;

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct ThermalState {
    /// CPU temperature in degrees Celsius, rounded. None = no reading
    /// available (probe still warming up, or the system doesn't expose
    /// any thermal source we know how to read).
    pub celsius: Option<i32>,
    /// Where the reading came from. Surfaced in the HUD tooltip so the
    /// user can tell at a glance whether it's the kernel's ACPI sense
    /// or a third-party tool. None when no source is available.
    pub source: Option<&'static str>,
}

pub struct Probe {
    celsius: Arc<AtomicI32>,
    source: Arc<std::sync::Mutex<Option<&'static str>>>,
}

impl Probe {
    pub fn spawn() -> Self {
        let celsius = Arc::new(AtomicI32::new(NO_READING));
        let source: Arc<std::sync::Mutex<Option<&'static str>>> =
            Arc::new(std::sync::Mutex::new(None));
        let c = celsius.clone();
        let src = source.clone();
        std::thread::spawn(move || {
            // Run the first probe immediately so the HUD doesn't sit on
            // an empty value for the full poll interval after launch.
            if let Some((v, s)) = read_once() {
                c.store(v, Ordering::Relaxed);
                *src.lock().unwrap() = Some(s);
            }
            loop {
                std::thread::sleep(POLL_INTERVAL);
                if let Some((v, s)) = read_once() {
                    c.store(v, Ordering::Relaxed);
                    *src.lock().unwrap() = Some(s);
                }
            }
        });
        Self { celsius, source }
    }

    pub fn current(&self) -> ThermalState {
        let v = self.celsius.load(Ordering::Relaxed);
        ThermalState {
            celsius: if v == NO_READING { None } else { Some(v) },
            source: *self.source.lock().unwrap(),
        }
    }
}

/// Try every thermal source we know about, in priority order:
///   1. ACPI thermal zone (kernel-mediated; works on most laptops).
///   2. LibreHardwareMonitor's WMI bridge (if user has LHM running).
///   3. OpenHardwareMonitor's WMI bridge (legacy; still common).
/// Returns the first successful reading along with a label that the
/// frontend uses for the HUD tooltip. None when nothing works — typical
/// on bare-metal desktops without any third-party sensor service.
fn read_once() -> Option<(i32, &'static str)> {
    // 1. ACPI thermal zones, tenths-of-Kelvin.
    let acpi = run_ps(
        "(Get-CimInstance -Namespace 'root/wmi' -ClassName 'MSAcpi_ThermalZoneTemperature' \
         -ErrorAction SilentlyContinue | Select-Object -First 1).CurrentTemperature",
    );
    if let Some(raw) = acpi.and_then(|s| s.parse::<f32>().ok()) {
        if raw > 0.0 {
            let c = (raw / 10.0) - 273.15;
            if (-20.0..=120.0).contains(&c) {
                return Some((c.round() as i32, "ACPI"));
            }
        }
    }

    // 2. LibreHardwareMonitor — exposes Sensor instances under
    // root\LibreHardwareMonitor when the LHM service is running.
    // Filter for SensorType='Temperature' and Name like '*CPU*' to get
    // the package temp; falls back to the highest reading found.
    let lhm = run_ps(
        "$s = Get-CimInstance -Namespace 'root/LibreHardwareMonitor' -ClassName 'Sensor' \
         -ErrorAction SilentlyContinue | Where-Object { $_.SensorType -eq 'Temperature' }; \
         if ($s) { ($s | Where-Object { $_.Name -match 'CPU' } | Select-Object -First 1 -ExpandProperty Value) }",
    );
    if let Some(c) = lhm.and_then(|s| s.parse::<f32>().ok()) {
        if (-20.0..=120.0).contains(&c) {
            return Some((c.round() as i32, "LibreHardwareMonitor"));
        }
    }

    // 3. OpenHardwareMonitor — same shape, different namespace.
    let ohm = run_ps(
        "$s = Get-CimInstance -Namespace 'root/OpenHardwareMonitor' -ClassName 'Sensor' \
         -ErrorAction SilentlyContinue | Where-Object { $_.SensorType -eq 'Temperature' }; \
         if ($s) { ($s | Where-Object { $_.Name -match 'CPU' } | Select-Object -First 1 -ExpandProperty Value) }",
    );
    if let Some(c) = ohm.and_then(|s| s.parse::<f32>().ok()) {
        if (-20.0..=120.0).contains(&c) {
            return Some((c.round() as i32, "OpenHardwareMonitor"));
        }
    }
    None
}

fn run_ps(script: &str) -> Option<String> {
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
