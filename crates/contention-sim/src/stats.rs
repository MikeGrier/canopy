// Copyright (c) Michael Grier.

//! Percentiles within a run, and means with confidence intervals across replications.

/// The normal quantile for a two-sided 95% interval.
const Z95: f64 = 1.96;

/// The `p`-th percentile (0..=100) of `samples` by nearest rank; 0 when there are none.
pub fn percentile(samples: &mut [f64], p: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(f64::total_cmp);
    let rank = (p / 100.0 * (samples.len() - 1) as f64).round() as usize;
    samples[rank.min(samples.len() - 1)]
}

/// The fraction of `samples` above `bound`.
pub fn fraction_above(samples: &[f64], bound: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().filter(|&&s| s > bound).count() as f64 / samples.len() as f64
}

/// One metric's value in each independent replication, summarised as a mean and the half-width
/// of its 95% confidence interval.
#[derive(Clone, Debug, Default)]
pub struct Replicated {
    values: Vec<f64>,
}

impl Replicated {
    pub fn new() -> Replicated {
        Replicated::default()
    }

    pub fn push(&mut self, value: f64) {
        self.values.push(value);
    }

    pub fn mean(&self) -> f64 {
        if self.values.is_empty() {
            return 0.0;
        }
        self.values.iter().sum::<f64>() / self.values.len() as f64
    }

    pub fn half_width(&self) -> f64 {
        let n = self.values.len();
        if n < 2 {
            return 0.0;
        }
        let mean = self.mean();
        let variance = self
            .values
            .iter()
            .map(|v| (v - mean) * (v - mean))
            .sum::<f64>()
            / (n - 1) as f64;
        Z95 * (variance / n as f64).sqrt()
    }

    pub fn max(&self) -> f64 {
        self.values.iter().copied().fold(0.0, f64::max)
    }
}
