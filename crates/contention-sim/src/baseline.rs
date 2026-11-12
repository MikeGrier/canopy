// Copyright (c) Michael Grier.

//! The analytic baseline for one exclusion: the M/G/1 queue.
//!
//! It holds only under its assumptions -- Poisson arrivals, a stable queue (utilization below
//! one), a work-conserving and non-preemptive discipline, and service times independent of the
//! order of service with finite variance -- and then the mean wait is the same for every such
//! discipline, first-come-first-served and random order alike (Kleinrock's conservation law).
//! Only the mean: the tail of the wait depends on the discipline, which is what the simulation
//! is for.

use crate::dist::Dist;

/// The Pollaczek-Khinchine mean wait in queue for Poisson arrivals at `rate` and service times
/// drawn from `hold`, or `None` when the queue is not stable.
pub fn mean_wait(rate: f64, hold: &Dist) -> Option<f64> {
    let utilization = rate * hold.mean();
    if !(0.0..1.0).contains(&utilization) {
        return None;
    }
    Some(rate * hold.second_moment() / (2.0 * (1.0 - utilization)))
}

/// The arrival rate that loads one exclusion to `utilization` with service times from `hold`.
pub fn rate_for(utilization: f64, hold: &Dist) -> f64 {
    utilization / hold.mean()
}
