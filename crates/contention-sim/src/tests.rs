// Copyright (c) Michael Grier.

use crate::baseline;
use crate::dist::Dist;
use crate::exclusion::{self, Arrivals, Discipline};
use crate::gate;
use crate::report::{Collect, Sink};
use crate::rng::SplitMix64;
use crate::snapshot;
use crate::stats::{Replicated, fraction_above, percentile};

fn sample_mean(dist: Dist, n: usize, seed: u64) -> f64 {
    let mut rng = SplitMix64::new(seed);
    (0..n).map(|_| dist.sample(&mut rng)).sum::<f64>() / n as f64
}

fn poisson_lock(discipline: Discipline, utilization: f64, hold: Dist) -> exclusion::Config {
    exclusion::Config {
        processes: 4,
        threads: 1,
        arrivals: Arrivals::Poisson {
            rate: baseline::rate_for(utilization, &hold),
        },
        hold,
        discipline,
        barge: 0.0,
        handoff: 0.0,
        horizon: 40_000.0,
        warmup: 100.0,
    }
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

#[test]
fn a_seed_fixes_the_sequence() {
    let mut a = SplitMix64::new(7);
    let mut b = SplitMix64::new(7);
    let mut c = SplitMix64::new(8);
    let first: Vec<u64> = (0..16).map(|_| a.next_u64()).collect();
    let again: Vec<u64> = (0..16).map(|_| b.next_u64()).collect();
    let other: Vec<u64> = (0..16).map(|_| c.next_u64()).collect();
    assert_eq!(first, again);
    assert_ne!(first, other);
}

#[test]
fn replication_streams_differ() {
    let mut a = SplitMix64::stream(1, 0);
    let mut b = SplitMix64::stream(1, 1);
    assert_ne!(a.next_u64(), b.next_u64());
}

#[test]
fn uniform_draws_stay_in_range() {
    let mut rng = SplitMix64::new(3);
    for _ in 0..10_000 {
        let u = rng.unit();
        assert!((0.0..1.0).contains(&u));
        assert!(rng.open_unit() > 0.0);
        assert!(rng.below(5) < 5);
    }
    assert_eq!(rng.below(1), 0);
}

#[test]
fn an_exponential_has_its_mean() {
    let m = sample_mean(Dist::Exponential { mean: 2.0 }, 200_000, 11);
    assert!((m - 2.0).abs() < 0.03, "sample mean {m}");
}

#[test]
fn a_log_normal_has_its_median_and_mean() {
    let dist = Dist::LogNormal {
        median: 0.037,
        sigma: 0.2,
    };
    let mut rng = SplitMix64::new(5);
    let mut samples: Vec<f64> = (0..100_001).map(|_| dist.sample(&mut rng)).collect();
    let median = percentile(&mut samples, 50.0);
    assert!((median - 0.037).abs() < 0.0005, "median {median}");
    let m = mean(&samples);
    assert!((m - dist.mean()).abs() / dist.mean() < 0.01, "mean {m}");
}

#[test]
fn a_spiky_distribution_has_its_moments() {
    let dist = Dist::Spiky {
        median: 1.0,
        sigma: 0.1,
        spike_prob: 0.05,
        spike_factor: 10.0,
    };
    let mut rng = SplitMix64::new(9);
    let samples: Vec<f64> = (0..200_000).map(|_| dist.sample(&mut rng)).collect();
    let m = mean(&samples);
    let second = samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64;
    assert!((m - dist.mean()).abs() / dist.mean() < 0.02, "mean {m}");
    assert!(
        (second - dist.second_moment()).abs() / dist.second_moment() < 0.05,
        "second moment {second}"
    );
    assert_eq!(dist.median(), 1.0);
}

#[test]
fn fixed_and_exponential_moments() {
    assert_eq!(Dist::Fixed(3.0).second_moment(), 9.0);
    assert_eq!(Dist::Exponential { mean: 2.0 }.second_moment(), 8.0);
    assert!((Dist::Exponential { mean: 1.0 }.median() - std::f64::consts::LN_2).abs() < 1e-12);
}

#[test]
fn percentiles_and_tails() {
    let mut samples: Vec<f64> = (1..=100).map(f64::from).collect();
    assert_eq!(percentile(&mut samples, 0.0), 1.0);
    assert_eq!(percentile(&mut samples, 100.0), 100.0);
    assert_eq!(percentile(&mut samples, 50.0), 51.0);
    assert_eq!(fraction_above(&samples, 90.0), 0.1);
    assert_eq!(percentile(&mut [], 99.0), 0.0);
    assert_eq!(fraction_above(&[], 1.0), 0.0);
}

#[test]
fn replicated_mean_and_interval() {
    let mut r = Replicated::new();
    assert_eq!(r.half_width(), 0.0);
    for v in [1.0, 2.0, 3.0, 4.0] {
        r.push(v);
    }
    assert_eq!(r.mean(), 2.5);
    assert_eq!(r.max(), 4.0);
    assert!(r.half_width() > 0.0);
}

#[test]
fn the_baseline_is_the_m_m_1_wait_and_refuses_an_unstable_queue() {
    let hold = Dist::Exponential { mean: 1.0 };
    let w = baseline::mean_wait(0.5, &hold).unwrap();
    assert!((w - 1.0).abs() < 1e-12);
    assert!(baseline::mean_wait(1.0, &hold).is_none());
    assert!((baseline::rate_for(0.5, &Dist::Fixed(2.0)) - 0.25).abs() < 1e-12);
}

#[test]
fn first_come_first_served_matches_the_baseline() {
    let hold = Dist::Exponential { mean: 1.0 };
    let cfg = poisson_lock(Discipline::FifoGlobal, 0.5, hold);
    let outcome = exclusion::simulate(&cfg, 21);
    let simulated = mean(&outcome.waits);
    let expected = baseline::mean_wait(0.5, &hold).unwrap();
    assert!(
        (simulated - expected).abs() / expected < 0.1,
        "simulated {simulated}, expected {expected}"
    );
}

#[test]
fn random_order_keeps_the_mean_and_fattens_the_tail() {
    let hold = Dist::Exponential { mean: 1.0 };
    let mut fifo = exclusion::simulate(&poisson_lock(Discipline::FifoGlobal, 0.8, hold), 31);
    let mut random = exclusion::simulate(&poisson_lock(Discipline::Unordered, 0.8, hold), 31);
    let (mf, mr) = (mean(&fifo.waits), mean(&random.waits));
    assert!(
        (mf - mr).abs() / mf < 0.15,
        "fifo mean {mf}, random mean {mr}"
    );
    let tail_f = percentile(&mut fifo.waits, 99.9);
    let tail_r = percentile(&mut random.waits, 99.9);
    assert!(
        tail_r > tail_f,
        "fifo p99.9 {tail_f}, random p99.9 {tail_r}"
    );
}

#[test]
fn first_come_first_served_passes_no_one_over_even_with_barging_allowed() {
    let mut cfg = poisson_lock(Discipline::FifoGlobal, 0.7, Dist::Fixed(1.0));
    cfg.barge = 1.0;
    cfg.handoff = 0.05;
    cfg.horizon = 5_000.0;
    let outcome = exclusion::simulate(&cfg, 41);
    assert!(outcome.passed_over.iter().all(|&p| p == 0.0));
}

#[test]
fn barging_passes_waiters_over() {
    let mut cfg = poisson_lock(Discipline::Unordered, 0.7, Dist::Fixed(1.0));
    cfg.barge = 1.0;
    cfg.handoff = 0.05;
    cfg.horizon = 5_000.0;
    let outcome = exclusion::simulate(&cfg, 43);
    assert!(outcome.passed_over.iter().any(|&p| p > 0.0));
}

#[test]
fn bounded_bypass_bounds_how_often_a_waiter_is_passed_over() {
    let limit = 2;
    let threads = 4;
    let cfg = exclusion::Config {
        processes: 1,
        threads,
        arrivals: Arrivals::Closed {
            think: Dist::Exponential { mean: 0.5 },
        },
        hold: Dist::Exponential { mean: 1.0 },
        discipline: Discipline::BoundedBypass { limit },
        barge: 0.5,
        handoff: 0.01,
        horizon: 5_000.0,
        warmup: 10.0,
    };
    let outcome = exclusion::simulate(&cfg, 47);
    let most = outcome.passed_over.iter().copied().fold(0.0, f64::max);
    assert!(
        most <= f64::from(limit) + threads as f64,
        "most passed over {most}"
    );
    let unordered = exclusion::simulate(
        &exclusion::Config {
            discipline: Discipline::Unordered,
            ..cfg
        },
        47,
    );
    let most_unordered = unordered.passed_over.iter().copied().fold(0.0, f64::max);
    assert!(
        most_unordered > most,
        "unordered {most_unordered}, bounded {most}"
    );
}

#[test]
fn first_come_first_served_within_one_process_is_first_come_first_served() {
    let mut cfg = poisson_lock(Discipline::FifoWithinProcess, 0.7, Dist::Fixed(1.0));
    cfg.processes = 1;
    cfg.horizon = 5_000.0;
    let outcome = exclusion::simulate(&cfg, 53);
    assert!(outcome.passed_over.iter().all(|&p| p == 0.0));
}

#[test]
fn a_burst_makes_every_request_it_promises() {
    let cfg = exclusion::Config {
        processes: 4,
        threads: 1,
        arrivals: Arrivals::Bursts {
            period: 100.0,
            size: 8,
            writes: 3,
            think: Dist::Exponential { mean: 0.1 },
        },
        hold: Dist::Fixed(0.01),
        discipline: Discipline::Unordered,
        barge: 0.0,
        handoff: 0.0,
        horizon: 1_050.0,
        warmup: 0.0,
    };
    let outcome = exclusion::simulate(&cfg, 59);
    assert_eq!(outcome.waits.len(), 11 * 8 * 3);
}

#[test]
fn a_run_is_reproducible_from_its_seed() {
    let cfg = poisson_lock(Discipline::Unordered, 0.6, Dist::Exponential { mean: 1.0 });
    let short = exclusion::Config {
        horizon: 2_000.0,
        ..cfg
    };
    let a = exclusion::simulate(&short, 61);
    let b = exclusion::simulate(&short, 61);
    assert_eq!(a.waits, b.waits);
    assert_eq!(a.passed_over, b.passed_over);
}

#[test]
fn an_idle_lock_makes_no_one_wait() {
    let cfg = exclusion::Config {
        horizon: 2_000.0,
        ..poisson_lock(Discipline::Unordered, 0.000_1, Dist::Fixed(0.001))
    };
    let outcome = exclusion::simulate(&cfg, 67);
    assert!(outcome.waits.iter().all(|&w| w == 0.0));
    assert_eq!(outcome.longest_unserved, 0.0);
}

fn snapshot_cfg(rate: f64, policy: snapshot::Policy) -> snapshot::Config {
    snapshot::Config {
        write_rate: rate,
        hold: Dist::LogNormal {
            median: 0.037,
            sigma: 0.2,
        },
        read: 0.001,
        policy,
        reads: 20_000,
        horizon: 4_000.0,
    }
}

const SPIN: snapshot::Policy = snapshot::Policy::Spin {
    attempts: 16,
    gap: 0.000_02,
};

#[test]
fn a_spinning_snapshot_fails_whenever_a_write_overlaps_it() {
    let cfg = snapshot_cfg(1.0, SPIN);
    let outcome = snapshot::simulate(&cfg, 71);
    let expected = snapshot::spin_failure_approx(1.0, &cfg.hold, cfg.read);
    let observed = outcome.failure_rate();
    assert!(
        (observed - expected).abs() < 0.01,
        "observed {observed}, expected {expected}"
    );
}

#[test]
fn a_snapshot_without_writes_never_fails() {
    let outcome = snapshot::simulate(&snapshot_cfg(1e-9, SPIN), 73);
    assert_eq!(outcome.failures, 0);
    assert!((outcome.mean_latency - 0.001).abs() < 1e-9);
}

#[test]
fn waiting_for_an_even_count_rides_out_a_write() {
    let cfg = snapshot_cfg(1.0, snapshot::Policy::WaitEven { deadline: 1.0 });
    let outcome = snapshot::simulate(&cfg, 79);
    assert_eq!(outcome.failures, 0);
    assert!(outcome.longest_latency < 1.0);
}

fn gate_cfg(rate: f64, policy: gate::Policy) -> gate::Config {
    gate::Config {
        attach_rate: rate,
        gate_hold: 0.002,
        mean_lifetime: 1.0,
        policy,
    }
}

#[test]
fn withholding_shared_admission_bounds_the_gate_wait_by_one_hold() {
    for seed in 0..200 {
        let outcome = gate::simulate(&gate_cfg(500.0, gate::Policy::ExclusiveFirst), seed);
        assert!(outcome.gate_wait <= 0.002 + 1e-12);
        assert!(outcome.body_wait >= outcome.gate_wait);
    }
}

#[test]
fn overlapping_shared_holds_keep_the_upgrader_at_the_gate() {
    let waits = |policy| {
        (0..200)
            .map(|seed| gate::simulate(&gate_cfg(2_000.0, policy), seed).gate_wait)
            .sum::<f64>()
            / 200.0
    };
    let contract = waits(gate::Policy::Contract);
    let first = waits(gate::Policy::ExclusiveFirst);
    assert!(
        contract > 10.0 * first,
        "contract {contract}, exclusive first {first}"
    );
}

#[test]
fn without_attachments_an_upgrader_waits_for_nothing() {
    let cfg = gate::Config {
        attach_rate: 1e-9,
        ..gate_cfg(1.0, gate::Policy::Contract)
    };
    let outcome = gate::simulate(&cfg, 83);
    assert_eq!(outcome.gate_wait, 0.0);
    assert_eq!(outcome.body_wait, 0.0);
}

#[test]
fn an_upgrader_waits_for_the_longest_lived_attachment() {
    let short = gate_cfg(10.0, gate::Policy::ExclusiveFirst);
    let long = gate::Config {
        mean_lifetime: 100.0,
        ..short
    };
    let mean_body = |cfg: &gate::Config| {
        (0..400)
            .map(|seed| gate::simulate(cfg, seed).body_wait)
            .sum::<f64>()
            / 400.0
    };
    let (s, l) = (mean_body(&short), mean_body(&long));
    assert!(l > 50.0 * s, "short {s}, long {l}");
}

#[test]
fn the_shared_busy_period_tends_to_one_hold_when_admissions_are_rare() {
    let period = gate::mean_shared_busy_period(1e-6, 0.002);
    assert!((period - 0.002).abs() < 1e-6);
    assert!(gate::mean_shared_busy_period(1_000.0, 0.01) > 20.0);
}

#[test]
fn a_collecting_sink_keeps_its_lines() {
    let mut sink = Collect::default();
    sink.line("one");
    sink.line("two");
    assert_eq!(sink.lines, vec!["one".to_string(), "two".to_string()]);
}
