// Copyright (c) Michael Grier.

use super::*;

#[test]
fn tokens_are_refused_once_the_count_is_spent_rather_than_repeated() {
    let next = AtomicU32::new(u32::MAX - 2);
    let made: Vec<_> = (0..2)
        .map(|_| token_from(&next, 7).expect("a token"))
        .collect();
    assert_ne!(made[0], made[1]);
    for _ in 0..3 {
        assert!(matches!(
            token_from(&next, 7),
            Err(CoordError::Exhausted { .. })
        ));
    }
}

#[test]
fn tokens_differ_by_count_and_by_start() {
    let next = AtomicU32::new(0);
    let first = token_from(&next, 1).unwrap();
    let second = token_from(&next, 1).unwrap();
    let other_start = token_from(&AtomicU32::new(0), 2).unwrap();
    assert_ne!(first, second);
    assert_ne!(first, other_start);
}
