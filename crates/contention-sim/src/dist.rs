// Copyright (c) Michael Grier.

//! The distributions of hold times, think times and lifetimes.

use crate::rng::SplitMix64;

#[derive(Clone, Copy, Debug)]
pub enum Dist {
    Fixed(f64),
    Exponential {
        mean: f64,
    },
    /// Log-normal, given by its median and the standard deviation of its logarithm.
    LogNormal {
        median: f64,
        sigma: f64,
    },
    /// Log-normal, except that with probability `spike_prob` a sample is `spike_factor` times as
    /// long: the occasional slow sync a measured hold time shows.
    Spiky {
        median: f64,
        sigma: f64,
        spike_prob: f64,
        spike_factor: f64,
    },
}

impl Dist {
    pub fn sample(&self, rng: &mut SplitMix64) -> f64 {
        match *self {
            Dist::Fixed(value) => value,
            Dist::Exponential { mean } => -mean * rng.open_unit().ln(),
            Dist::LogNormal { median, sigma } => median * (sigma * standard_normal(rng)).exp(),
            Dist::Spiky {
                median,
                sigma,
                spike_prob,
                spike_factor,
            } => {
                let base = median * (sigma * standard_normal(rng)).exp();
                if rng.chance(spike_prob) {
                    base * spike_factor
                } else {
                    base
                }
            }
        }
    }

    pub fn mean(&self) -> f64 {
        match *self {
            Dist::Fixed(value) => value,
            Dist::Exponential { mean } => mean,
            Dist::LogNormal { median, sigma } => median * (sigma * sigma / 2.0).exp(),
            Dist::Spiky {
                median,
                sigma,
                spike_prob,
                spike_factor,
            } => {
                median
                    * (sigma * sigma / 2.0).exp()
                    * (1.0 - spike_prob + spike_prob * spike_factor)
            }
        }
    }

    /// `E[X^2]`, which the queueing baseline needs.
    pub fn second_moment(&self) -> f64 {
        match *self {
            Dist::Fixed(value) => value * value,
            Dist::Exponential { mean } => 2.0 * mean * mean,
            Dist::LogNormal { median, sigma } => median * median * (2.0 * sigma * sigma).exp(),
            Dist::Spiky {
                median,
                sigma,
                spike_prob,
                spike_factor,
            } => {
                median
                    * median
                    * (2.0 * sigma * sigma).exp()
                    * (1.0 - spike_prob + spike_prob * spike_factor * spike_factor)
            }
        }
    }

    /// The median: what the budget's relative bounds are multiples of.
    pub fn median(&self) -> f64 {
        match *self {
            Dist::Fixed(value) => value,
            Dist::Exponential { mean } => mean * std::f64::consts::LN_2,
            Dist::LogNormal { median, .. } | Dist::Spiky { median, .. } => median,
        }
    }
}

/// A standard normal sample, by the Box-Muller transform.
pub(crate) fn standard_normal(rng: &mut SplitMix64) -> f64 {
    let radius = (-2.0 * rng.open_unit().ln()).sqrt();
    let angle = 2.0 * std::f64::consts::PI * rng.unit();
    radius * angle.cos()
}
