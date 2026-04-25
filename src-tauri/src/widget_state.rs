use crate::widgets::{clock, network, media};
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HudSnapshot {
    pub clock: clock::ClockState,
    pub network: network::NetState,
    pub media: media::MediaState,
}

pub fn spawn(app: AppHandle, tick: Duration) {
    std::thread::spawn(move || {
        let mut net = network::Sampler::new(tick, 3);
        let mut prev_snapshot: Option<HudSnapshot> = None;
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let min_gap = Duration::from_millis(200);

        loop {
            std::thread::sleep(tick);
            let snapshot = HudSnapshot {
                clock: clock::current(),
                network: net.tick(),
                media: media::current().unwrap_or_default(),
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
fn snapshot_equivalent(a: &HudSnapshot, b: &HudSnapshot) -> bool {
    a.network == b.network
        && a.media == b.media
        && a.clock.now_local[..16] == b.clock.now_local[..16] // YYYY-MM-DDTHH:MM
}
