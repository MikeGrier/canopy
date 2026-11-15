// Copyright (c) Michael Grier.

use super::*;

fn measured(count: usize, window: f64) -> ResultRow {
    let samples = (0..count)
        .map(|index| metrics::Sample {
            worker: 1,
            request: index as u64 + 1,
            acquired: index as u64 + 2,
            generation: index as u64 + 1,
            wait_seconds: 0.1,
            hold_seconds: 0.05,
            lock_wait_seconds: 0.1,
            released: true,
            opening_seconds: 0.002,
            closing_seconds: 0.003,
        })
        .collect();
    row(
        "service-test".into(),
        0,
        true,
        window,
        count,
        vec![Event::Done { samples }],
    )
    .unwrap()
}

#[test]
fn normal_replication_summaries_preserve_measurements_without_claiming_tail_proof() {
    for count in 1..=12 {
        let report = summarize(&[measured(count, 1.0), measured(count, 1.0)]);
        assert_eq!(report["completed_samples"], count * 2);
        assert_eq!(report["completed_requests"], count * 2);
        assert_eq!(report["unfinished_requests"], 0);
        assert_eq!(report["p99_mean_seconds"], 0.1);
        assert_eq!(report["p99_halfwidth_95"], 0.0);
        assert_eq!(report["stationary_tail_budget_established"], false);
        assert_eq!(report["reopen_fairness_decision"], false);
    }
}

#[test]
fn absent_samples_and_one_replication_do_not_produce_a_confidence_interval() {
    let empty = summarize(&[]);
    assert!(empty["p99_mean_seconds"].is_null());
    assert!(empty["p99_halfwidth_95"].is_null());
    assert!(empty["snapshot_zero_failure_upper_95_iid"].is_null());
    assert!(summarize(&[measured(1, 1.0)])["p99_halfwidth_95"].is_null());
}

#[test]
fn censoring_and_correlated_acquisitions_are_not_extra_completed_requests() {
    let mut measured = measured(3, 1.0);
    measured.samples[1].request = measured.samples[0].request;
    measured.samples[1].worker = measured.samples[0].worker;
    let mut measured = row(
        "upgrade-safe".into(),
        0,
        true,
        1.0,
        3,
        vec![Event::Done {
            samples: measured.samples,
        }],
    )
    .unwrap();
    measured.upgrade_wait = Some(1.0);
    measured.upgrade_censored = true;
    let report = summarize(&[measured]);
    assert_eq!(report["completed_requests"], 2);
    assert_eq!(report["unfinished_requests"], 1);
    assert_eq!(report["upgrade_censored_replications"], 1);
    assert_eq!(report["upgrade_observed_or_lower_bound_mean_seconds"], 1.0);
}

#[test]
fn worker_errors_bad_releases_clock_inversions_and_spurious_notices_fail_measurement() {
    for event in [
        Event::Error {
            message: "worker failure".into(),
        },
        Event::Notice {
            id: 0,
            elapsed: 2.0,
            changed: false,
        },
    ] {
        assert!(row("bad".into(), 0, true, 1.0, 1, vec![event]).is_err());
    }
    for invalid_release in [false, true] {
        let mut measured = measured(1, 1.0);
        if invalid_release {
            measured.samples[0].released = false;
        } else {
            measured.samples[0].acquired = 0;
        }
        assert!(
            row(
                "bad".into(),
                0,
                true,
                1.0,
                1,
                vec![Event::Done {
                    samples: measured.samples
                }]
            )
            .is_err()
        );
    }
}

#[test]
fn paired_poisson_schedules_replay_exactly_and_stay_inside_the_window() {
    for seed in 0..12 {
        let first = poisson(100.0, 1.0, &mut SplitMix64::new(seed));
        let second = poisson(100.0, 1.0, &mut SplitMix64::new(seed));
        assert_eq!(first, second);
        assert!(first.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(first.iter().all(|time| *time > 0.0 && *time < 1.0));
    }
}
