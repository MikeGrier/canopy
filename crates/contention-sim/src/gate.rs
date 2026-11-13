// Copyright (c) Michael Grier.

//! An upgrader at the admission gate while handles go on attaching.
//!
//! Handles arrive as a Poisson stream. Each holds the gate shared for `gate_hold` -- long enough
//! to take the reader tier -- and then stays attached, holding the tiers, for a lifetime drawn
//! from an exponential distribution. The upgrader needs the gate exclusively, and then every
//! attachment gone from the tiers before its body begins. Under the contract a handle may still
//! pass the gate shared while the upgrader waits, so the gate frees only when the shared holds
//! stop overlapping; with shared admission withheld while the upgrader waits, only the holds
//! already begun stand in its way. Either way, once it holds the gate no new handle attaches,
//! and it waits for the attachments already there.
//!
//! The upgrader arrives into the stream's steady state, which is drawn directly rather than
//! simulated from the start: the handles holding the gate are Poisson in number with residual
//! holds uniform over `gate_hold`, and the attached handles are Poisson in number with
//! exponential residual lifetimes, since the exponential is memoryless.

use crate::dist::{Dist, standard_normal};
use crate::rng::SplitMix64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Policy {
    /// Shared admission goes on while the upgrader waits for the gate: the contract.
    Contract,
    /// No handle passes the gate shared while the upgrader waits for it.
    ExclusiveFirst,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub attach_rate: f64,
    pub gate_hold: f64,
    pub mean_lifetime: f64,
    pub policy: Policy,
}

/// How long one upgrader waited for the gate, and for its body.
#[derive(Clone, Copy, Debug)]
pub struct Outcome {
    pub gate_wait: f64,
    pub body_wait: f64,
}

/// Above this mean a Poisson count is drawn from its normal approximation.
const POISSON_NORMAL_ABOVE: f64 = 64.0;

/// A Poisson count with the given mean.
fn poisson(mean: f64, rng: &mut SplitMix64) -> u64 {
    if mean <= 0.0 {
        return 0;
    }
    if mean > POISSON_NORMAL_ABOVE {
        let z = standard_normal(rng);
        return (mean + mean.sqrt() * z).round().max(0.0) as u64;
    }
    let limit = (-mean).exp();
    let mut product = rng.open_unit();
    let mut count = 0;
    while product > limit {
        product *= rng.open_unit();
        count += 1;
    }
    count
}

/// The largest of `n` independent draws whose distribution function is `u -> u^(1/n)` mapped by
/// `quantile`; zero when `n` is zero.
fn largest_of(n: u64, rng: &mut SplitMix64, quantile: impl Fn(f64) -> f64) -> f64 {
    if n == 0 {
        return 0.0;
    }
    quantile(rng.unit().powf(1.0 / n as f64))
}

/// One upgrader, arriving at time zero into the steady state, from `seed`.
pub fn simulate(cfg: &Config, seed: u64) -> Outcome {
    let mut rng = SplitMix64::new(seed);
    let lifetime = Dist::Exponential {
        mean: cfg.mean_lifetime,
    };
    let holding_gate = poisson(cfg.attach_rate * cfg.gate_hold, &mut rng);
    let mut gate_free = largest_of(holding_gate, &mut rng, |u| u * cfg.gate_hold);
    let attached = poisson(cfg.attach_rate * cfg.mean_lifetime, &mut rng);
    let mean_lifetime = cfg.mean_lifetime;
    let mut tiers_free = largest_of(attached, &mut rng, |u| -mean_lifetime * (1.0 - u).ln());
    // A handle still holding the gate attaches when it drops it.
    for _ in 0..holding_gate {
        tiers_free = tiers_free.max(gate_free + lifetime.sample(&mut rng));
    }
    if cfg.policy == Policy::Contract {
        let gap = Dist::Exponential {
            mean: 1.0 / cfg.attach_rate,
        };
        let mut t = gap.sample(&mut rng);
        while t < gate_free {
            let released = t + cfg.gate_hold;
            gate_free = gate_free.max(released);
            tiers_free = tiers_free.max(released + lifetime.sample(&mut rng));
            t += gap.sample(&mut rng);
        }
    }
    Outcome {
        gate_wait: gate_free,
        body_wait: tiers_free.max(gate_free),
    }
}

/// The mean length of a period in which the gate is held shared without a gap, for Poisson
/// admissions at `rate` each holding it for a fixed `hold`: the busy period of an infinite-server
/// queue, `(exp(rate * hold) - 1) / rate`. An upgrader that arrives inside one waits out its rest.
pub fn mean_shared_busy_period(rate: f64, hold: f64) -> f64 {
    ((rate * hold).exp() - 1.0) / rate
}
