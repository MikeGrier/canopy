// Copyright (c) Michael Grier.

use super::*;

#[test]
fn valid_and_invalid_writer_sequences_are_discriminated() {
    for count in 1..=12 {
        let mut turns: Vec<_> = (0..count)
            .map(|index| Turn {
                worker: index % 4,
                request: index as u64,
                acquired: index as u64 + 1,
                released: index as u64 + 2,
                generation: index as u64 + 11,
                count: index as u64 * 2 + 22,
            })
            .collect();
        turns.reverse();
        validate_turns(&turns, count, 10, 20).unwrap();
        turns[0].count += 1;
        assert!(validate_turns(&turns, count, 10, 20).is_err());
    }
    assert!(validate_turns(&[], 1, 0, 0).is_err());
    validate_turns(&[], 0, 0, 0).unwrap();
}
