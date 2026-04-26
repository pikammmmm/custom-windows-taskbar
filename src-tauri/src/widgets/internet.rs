use serde::Serialize;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InternetState {
    pub online: bool,
    /// Round-trip TCP-handshake to a public DNS, in milliseconds. `None` while
    /// offline or before the first probe finishes.
    pub ping_ms: Option<u32>,
}

const PROBE_INTERVAL: Duration = Duration::from_secs(8);
const PROBE_TIMEOUT: Duration = Duration::from_millis(900);
const PROBE_TARGETS: &[&str] = &["1.1.1.1:53", "8.8.8.8:53"];

pub struct Probe {
    online: Arc<AtomicBool>,
    ping_ms: Arc<AtomicU32>, // u32::MAX sentinel = unknown
}

impl Probe {
    pub fn spawn() -> Self {
        let online = Arc::new(AtomicBool::new(true));
        let ping_ms = Arc::new(AtomicU32::new(u32::MAX));
        let o = online.clone();
        let p = ping_ms.clone();
        std::thread::spawn(move || {
            // Run the first probe immediately so the HUD doesn't show stale
            // "online" until the first interval elapses.
            run_probe(&o, &p);
            loop {
                std::thread::sleep(PROBE_INTERVAL);
                run_probe(&o, &p);
            }
        });
        Self { online, ping_ms }
    }

    pub fn current(&self) -> InternetState {
        let raw = self.ping_ms.load(Ordering::Relaxed);
        InternetState {
            online: self.online.load(Ordering::Relaxed),
            ping_ms: if raw == u32::MAX { None } else { Some(raw) },
        }
    }
}

fn run_probe(online: &AtomicBool, ping_ms: &AtomicU32) {
    for target in PROBE_TARGETS {
        let Ok(addr) = target.parse::<SocketAddr>() else { continue };
        let started = Instant::now();
        match TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
            Ok(_) => {
                let elapsed = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
                online.store(true, Ordering::Relaxed);
                ping_ms.store(elapsed, Ordering::Relaxed);
                return;
            }
            Err(_) => continue,
        }
    }
    online.store(false, Ordering::Relaxed);
    ping_ms.store(u32::MAX, Ordering::Relaxed);
}
