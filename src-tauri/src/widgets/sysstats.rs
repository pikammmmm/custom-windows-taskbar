use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SysStats {
    /// 0.0..=100.0. Kept as f32 (not u8) so the snapshot equality check
    /// in widget_state catches sub-1% fluctuations and the HUD CPU
    /// readout actually animates instead of showing the same integer
    /// value frozen for seconds at a time. Frontend rounds for display.
    pub cpu_percent: f32,
    pub mem_percent: f32,
    pub mem_used_gb: f32,
    pub mem_total_gb: f32,
}

/// Long-lived sysinfo handle. The first `cpu_usage()` after construction
/// always returns 0 — it needs two samples to compute a delta — so we keep
/// the System around between ticks and let sysinfo accumulate.
fn shared() -> &'static Mutex<System> {
    static SYS: std::sync::OnceLock<Mutex<System>> = std::sync::OnceLock::new();
    SYS.get_or_init(|| {
        let s = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::new().with_cpu_usage())
                .with_memory(MemoryRefreshKind::new().with_ram()),
        );
        Mutex::new(s)
    })
}

pub fn current() -> SysStats {
    let mut s = shared().lock().unwrap();
    s.refresh_cpu_usage();
    s.refresh_memory();
    // sysinfo's per-CPU usage requires a small gap between refreshes for an
    // accurate sample. The HUD poller already runs at 1 Hz so the gap is
    // satisfied across calls — we don't sleep here.
    let cpus = s.cpus();
    let cpu_percent = if cpus.is_empty() {
        0.0
    } else {
        let avg = cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32;
        avg.clamp(0.0, 100.0)
    };
    let total = s.total_memory().max(1);
    let used = s.used_memory();
    let mem_percent = ((used as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32;
    let mem_used_gb = used as f32 / 1024.0 / 1024.0 / 1024.0;
    let mem_total_gb = total as f32 / 1024.0 / 1024.0 / 1024.0;
    SysStats { cpu_percent, mem_percent, mem_used_gb, mem_total_gb }
}

/// Warm up the CPU usage sampler so the first HUD tick has a real value.
/// Call once at app start, then wait at least ~200ms before the first
/// `current()` call.
pub fn prime() {
    let mut s = shared().lock().unwrap();
    s.refresh_cpu_usage();
    drop(s);
    std::thread::sleep(Duration::from_millis(50));
}
