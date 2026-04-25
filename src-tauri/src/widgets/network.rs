use serde::Serialize;
use std::collections::VecDeque;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NetState {
    pub down_bps: f64,
    pub up_bps: f64,
}

#[derive(Default)]
pub struct Smoother {
    samples: VecDeque<(u64, u64)>,
    capacity: usize,
}

impl Smoother {
    pub fn new(window: usize) -> Self {
        Self { samples: VecDeque::with_capacity(window), capacity: window }
    }

    pub fn push(&mut self, down: u64, up: u64) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back((down, up));
    }

    pub fn averaged(&self, tick: Duration) -> NetState {
        if self.samples.is_empty() {
            return NetState { down_bps: 0.0, up_bps: 0.0 };
        }
        let n = self.samples.len() as f64;
        let secs = tick.as_secs_f64();
        let (sum_down, sum_up) = self.samples.iter().fold((0u64, 0u64), |(d, u), (sd, su)| (d + sd, u + su));
        NetState {
            down_bps: sum_down as f64 / n / secs,
            up_bps: sum_up as f64 / n / secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_smoother_returns_zero() {
        let s = Smoother::new(3);
        assert_eq!(s.averaged(Duration::from_secs(1)), NetState { down_bps: 0.0, up_bps: 0.0 });
    }

    #[test]
    fn single_sample_averages_to_itself() {
        let mut s = Smoother::new(3);
        s.push(1000, 500);
        let avg = s.averaged(Duration::from_secs(1));
        assert_eq!(avg, NetState { down_bps: 1000.0, up_bps: 500.0 });
    }

    #[test]
    fn averages_three_samples() {
        let mut s = Smoother::new(3);
        s.push(1000, 100);
        s.push(2000, 200);
        s.push(3000, 300);
        let avg = s.averaged(Duration::from_secs(1));
        assert_eq!(avg, NetState { down_bps: 2000.0, up_bps: 200.0 });
    }

    #[test]
    fn evicts_oldest_at_capacity() {
        let mut s = Smoother::new(2);
        s.push(1000, 0);
        s.push(2000, 0);
        s.push(3000, 0);
        let avg = s.averaged(Duration::from_secs(1));
        assert_eq!(avg.down_bps, 2500.0);
    }

    #[test]
    fn divides_by_tick_seconds() {
        let mut s = Smoother::new(1);
        s.push(2000, 0);
        let avg = s.averaged(Duration::from_secs(2));
        assert_eq!(avg.down_bps, 1000.0);
    }
}

use sysinfo::Networks;

pub struct Sampler {
    networks: Networks,
    smoother: Smoother,
    tick: Duration,
}

impl Sampler {
    pub fn new(tick: Duration, smoothing_window: usize) -> Self {
        Self {
            networks: Networks::new_with_refreshed_list(),
            smoother: Smoother::new(smoothing_window),
            tick,
        }
    }

    /// Call once per tick. Returns smoothed throughput.
    pub fn tick(&mut self) -> NetState {
        self.networks.refresh();
        let (down, up) = self.networks.iter().fold((0u64, 0u64), |(d, u), (_, n)| {
            (d + n.received(), u + n.transmitted())
        });
        self.smoother.push(down, up);
        self.smoother.averaged(self.tick)
    }
}
