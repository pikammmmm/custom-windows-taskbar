use crate::widgets::{audio, battery, clock, internet, media, network, sysstats, thermal, warp, weather};
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HudSnapshot {
    pub clock: clock::ClockState,
    pub network: network::NetState,
    pub media: media::MediaState,
    pub audio: audio::AudioState,
    pub internet: internet::InternetState,
    pub sysstats: sysstats::SysStats,
    pub thermal: thermal::ThermalState,
    pub battery: battery::BatteryState,
    pub weather: weather::WeatherState,
    pub warp: warp::WarpState,
}

pub fn spawn(app: AppHandle, tick: Duration) {
    std::thread::spawn(move || {
        let mut net = network::Sampler::new(tick, 3);
        let inet = internet::Probe::spawn();
        let wx = weather::Probe::spawn();
        let warp_probe = warp::Probe::spawn();
        let thermal_probe = thermal::Probe::spawn();
        sysstats::prime();
        let mut prev_snapshot: Option<HudSnapshot> = None;
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let min_gap = Duration::from_millis(200);

        loop {
            std::thread::sleep(tick);
            let snapshot = HudSnapshot {
                clock: clock::current(),
                network: net.tick(),
                media: media::current().unwrap_or_default(),
                audio: audio::current(),
                internet: inet.current(),
                sysstats: sysstats::current(),
                thermal: thermal_probe.current(),
                battery: battery::current(),
                weather: wx.current(),
                warp: warp_probe.current(),
            };
            let changed_substantively = prev_snapshot.as_ref()
                .map(|p| !snapshot_equivalent(p, &snapshot))
                .unwrap_or(true);
            if !changed_substantively { continue; }
            if last_emit.elapsed() < min_gap { continue; }

            let _ = app.emit("hud:update", &snapshot);
            last_emit = Instant::now();
            prev_snapshot = Some(snapshot);
        }
    });
}

/// Comparison that ignores second-level clock changes — frontend ticks seconds itself.
/// sysstats deliberately uses f32 percent fields so this PartialEq picks
/// up sub-1% fluctuations and the HUD CPU readout actually animates;
/// without that the integer-rounded values stayed equal across many
/// ticks and the panel looked frozen.
fn snapshot_equivalent(a: &HudSnapshot, b: &HudSnapshot) -> bool {
    a.network == b.network
        && a.media == b.media
        && a.audio == b.audio
        && a.internet == b.internet
        && a.sysstats == b.sysstats
        && a.thermal == b.thermal
        && a.battery == b.battery
        && a.weather == b.weather
        && a.warp == b.warp
        && a.clock.now_local[..16] == b.clock.now_local[..16]
}
