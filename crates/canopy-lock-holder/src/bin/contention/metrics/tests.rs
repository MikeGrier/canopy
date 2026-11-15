// Copyright (c) Michael Grier.

use super::*;

fn sample(request: u64, generation: u64) -> Sample {
    Sample {
        worker: 0,
        request,
        acquired: request + 1,
        generation,
        wait_seconds: 0.0,
        hold_seconds: 0.0,
        lock_wait_seconds: 0.0,
        released: true,
        opening_seconds: 0.0,
        closing_seconds: 0.0,
    }
}

#[test]
fn bypass_uses_request_order_and_the_contracts_acquisition_generation() {
    for count in 1..=12 {
        let samples: Vec<_> = (0..count)
            .map(|index| sample(index, count - index))
            .collect();
        assert_eq!(
            passed_over(&samples),
            (0..count)
                .map(|index| (count - index - 1) as f64)
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(passed_over(&[sample(1, 1), sample(2, 2)]), [0.0, 0.0]);
    assert_eq!(passed_over(&[sample(1, 2), sample(1, 1)]), [0.0, 0.0]);
}

#[test]
fn empty_and_singleton_statistics_are_defined_and_failure_bounds_need_samples() {
    assert_eq!(percentile(&[], 99.9), 0.0);
    assert_eq!(percentile(&[2.0], 99.9), 2.0);
    assert_eq!(zero_failure_bound(0), None);
    for samples in [1, 2, 3, 4, 5, 10, 20, 50, 100, 1000, 10000, 3000000] {
        let bound = zero_failure_bound(samples).unwrap();
        assert!(bound > 0.0 && bound < 1.0);
    }
}
