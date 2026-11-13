// Copyright (c) Michael Grier.

//! A snapshot read while writes go on.
//!
//! Writes arrive as a Poisson stream and are served one at a time, each holding the modification
//! count odd for its hold. A read samples the count, reads for `read`, and samples it again; it is
//! disturbed when the count was odd at the start or a write began during it. What it does then is
//! its policy: the store's today retries at once, after yielding, a fixed number of times; the
//! alternative waits for the count to be even again, up to a deadline.

use crate::dist::Dist;
use crate::rng::SplitMix64;

#[derive(Clone, Copy, Debug)]
pub enum Policy {
    /// Up to `attempts` tries, each after a pause of `gap` -- a yield -- when disturbed.
    Spin { attempts: usize, gap: f64 },
    /// Wait for the count to be even before each try, and give up once `deadline` has passed
    /// since the read began.
    WaitEven { deadline: f64 },
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub write_rate: f64,
    pub hold: Dist,
    pub read: f64,
    pub policy: Policy,
    pub reads: usize,
    pub horizon: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Outcome {
    pub reads: usize,
    pub failures: usize,
    /// The mean time from a read's start to its success, over the reads that succeeded.
    pub mean_latency: f64,
    /// The longest a successful read took.
    pub longest_latency: f64,
}

impl Outcome {
    pub fn failure_rate(&self) -> f64 {
        if self.reads == 0 {
            0.0
        } else {
            self.failures as f64 / self.reads as f64
        }
    }
}

/// The intervals during which the count is odd: the writes, served one at a time, in order.
struct Writes {
    starts: Vec<f64>,
    ends: Vec<f64>,
}

impl Writes {
    fn generate(cfg: &Config, rng: &mut SplitMix64) -> Writes {
        let gap = Dist::Exponential {
            mean: 1.0 / cfg.write_rate,
        };
        let (mut starts, mut ends) = (Vec::new(), Vec::new());
        let (mut arrival, mut free) = (0.0_f64, 0.0_f64);
        loop {
            arrival += gap.sample(rng);
            if arrival > cfg.horizon {
                break;
            }
            let start = arrival.max(free);
            let end = start + cfg.hold.sample(rng);
            starts.push(start);
            ends.push(end);
            free = end;
        }
        Writes { starts, ends }
    }

    /// The end of the write in progress at `t`, if one is.
    fn odd_until(&self, t: f64) -> Option<f64> {
        let index = self.starts.partition_point(|&s| s <= t);
        if index > 0 && self.ends[index - 1] > t {
            Some(self.ends[index - 1])
        } else {
            None
        }
    }

    /// Whether a write begins in `(from, to]`.
    fn begins_within(&self, from: f64, to: f64) -> bool {
        let index = self.starts.partition_point(|&s| s <= from);
        index < self.starts.len() && self.starts[index] <= to
    }
}

/// Runs `cfg.reads` reads at times uniform over the horizon, against one replication of the
/// writes from `seed`.
pub fn simulate(cfg: &Config, seed: u64) -> Outcome {
    let mut rng = SplitMix64::new(seed);
    let writes = Writes::generate(cfg, &mut rng);
    let mut outcome = Outcome {
        reads: cfg.reads,
        ..Outcome::default()
    };
    let mut total_latency = 0.0;
    for _ in 0..cfg.reads {
        let start = rng.unit() * cfg.horizon;
        match attempt(cfg, &writes, start) {
            Some(latency) => {
                total_latency += latency;
                outcome.longest_latency = outcome.longest_latency.max(latency);
            }
            None => outcome.failures += 1,
        }
    }
    let succeeded = outcome.reads - outcome.failures;
    if succeeded > 0 {
        outcome.mean_latency = total_latency / succeeded as f64;
    }
    outcome
}

/// The time one read takes to succeed, or `None` when it gives up.
fn attempt(cfg: &Config, writes: &Writes, start: f64) -> Option<f64> {
    let mut t = start;
    match cfg.policy {
        Policy::Spin { attempts, gap } => {
            for _ in 0..attempts {
                if writes.odd_until(t).is_some() {
                    t += gap;
                    continue;
                }
                if writes.begins_within(t, t + cfg.read) {
                    t += cfg.read + gap;
                    continue;
                }
                return Some(t + cfg.read - start);
            }
            None
        }
        Policy::WaitEven { deadline } => loop {
            if let Some(end) = writes.odd_until(t) {
                t = end;
            }
            if t - start > deadline {
                return None;
            }
            if writes.begins_within(t, t + cfg.read) {
                t += cfg.read;
                continue;
            }
            return Some(t + cfg.read - start);
        },
    }
}

/// The failure probability of a spinning snapshot when its tries all fit inside one write's
/// hold: a read fails if it begins during a write or a write begins while it reads. Valid when
/// writes rarely overlap (`write_rate * hold.mean()` small).
pub fn spin_failure_approx(write_rate: f64, hold: &Dist, read: f64) -> f64 {
    1.0 - (-write_rate * (hold.mean() + read)).exp()
}
