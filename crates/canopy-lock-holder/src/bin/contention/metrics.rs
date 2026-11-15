// Copyright (c) Michael Grier.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub worker: usize,
    pub request: u64,
    pub acquired: u64,
    pub generation: u64,
    pub wait_seconds: f64,
    pub hold_seconds: f64,
    pub lock_wait_seconds: f64,
    pub released: bool,
    pub opening_seconds: f64,
    pub closing_seconds: f64,
}

pub fn passed_over(samples: &[Sample]) -> Vec<f64> {
    samples
        .iter()
        .map(|sample| {
            samples
                .iter()
                .filter(|other| {
                    other.request > sample.request && other.generation < sample.generation
                })
                .count() as f64
        })
        .collect()
}

pub fn percentile(values: &[f64], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut values = values.to_vec();
    contention_sim::stats::percentile(&mut values, percentile)
}

pub fn zero_failure_bound(samples: usize) -> Option<f64> {
    (samples > 0).then(|| 1.0 - 0.05_f64.powf(1.0 / samples as f64))
}
