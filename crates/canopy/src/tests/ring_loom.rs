// Copyright (c) Michael Grier.

//! `ring::Doorbell` under loom, a checker of the Rust memory model, which runs below the spec
//! barrier: `spec/Doorbell.tla`'s steps are sequentially consistent, and the doorbell realizes
//! them with acquire and release alone, which only a checker of the memory model can show is
//! enough, and that each of those orderings is needed. Built and run with
//! `RUSTFLAGS="--cfg canopy_loom"`.
//!
//! A lost wake is the consumer deciding to wait, with work published that it has not taken, while
//! the signal stays lowered after every producer has rung for the last time. The consumer polls
//! the signal where it would wait, so a lost wake is a failed assertion rather than a deadlock,
//! which loom reports but cannot unwind from cleanly.

use std::time::Duration;

use acadia::LoomSignal;
use loom::model::Builder;
use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loom::thread;

use crate::ring::Doorbell;
use crate::ring::sync::Weakening;

/// How many times loom lets one thread be preempted in a run, bounding the search so that it
/// ends; every lost wake these checks look for needs fewer.
const PREEMPTION_BOUND: usize = 3;

fn check(model: impl Fn() + Sync + Send + 'static) {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(PREEMPTION_BOUND);
    builder.check(model);
}

/// The doorbell's shape, bare: one side publishes and then sets a flag, the other clears the flag
/// and then reads what was published. Either the setter finds the flag cleared, or the clearer
/// finds the publish -- if the two meet with read-modify-writes, and not if the clear is a store.
fn meeting(clear_is_rmw: bool) {
    check(move || {
        let published = Arc::new(AtomicUsize::new(0));
        let flag = Arc::new(AtomicBool::new(true));
        let setter = {
            let (published, flag) = (Arc::clone(&published), Arc::clone(&flag));
            thread::spawn(move || {
                published.store(1, Ordering::Release);
                flag.swap(true, Ordering::AcqRel)
            })
        };
        if clear_is_rmw {
            flag.swap(false, Ordering::AcqRel);
        } else {
            flag.store(false, Ordering::Release);
        }
        let saw_publish = published.load(Ordering::Acquire) == 1;
        let setter_found_set = setter.join().unwrap();
        assert!(
            saw_publish || !setter_found_set,
            "each side missed the other"
        );
    });
}

/// Loom can be trusted with the doorbell's question only if it tells these two apart.
#[test]
fn read_modify_writes_on_one_flag_meet() {
    meeting(true);
}

#[test]
#[should_panic(expected = "each side missed the other")]
fn a_clear_that_is_a_store_can_miss_the_publish() {
    meeting(false);
}

/// `producers` threads each publish `each` items, releasing each into a count the consumer
/// acquires, and ring after each; the consumer runs the doorbell's rounds until it has taken them
/// all.
fn doorbell(producers: usize, each: usize, weakened: Option<Weakening>) {
    check(move || {
        let doorbell = Arc::new(
            match weakened {
                Some(weakening) => Doorbell::<LoomSignal>::weakened(weakening),
                None => Doorbell::<LoomSignal>::new(),
            }
            .unwrap(),
        );
        let published = Arc::new(AtomicUsize::new(0));
        // How many producers have rung for the last time; read only once the consumer has decided
        // to wait, so it orders nothing the doorbell must.
        let finished = Arc::new(AtomicUsize::new(0));
        let ringers: Vec<_> = (0..producers)
            .map(|_| {
                let (doorbell, published, finished) = (
                    Arc::clone(&doorbell),
                    Arc::clone(&published),
                    Arc::clone(&finished),
                );
                thread::spawn(move || {
                    for _ in 0..each {
                        published.fetch_add(1, Ordering::Release);
                        doorbell.ring().unwrap();
                    }
                    finished.fetch_add(1, Ordering::Release);
                })
            })
            .collect();
        let mut taken = 0;
        while taken < producers * each {
            // A round that takes, rearms and takes again, without waiting.
            let took = doorbell
                .pump(Duration::ZERO, || {
                    let now = published.load(Ordering::Acquire);
                    let found = now > taken;
                    taken = now;
                    found
                })
                .unwrap();
            if took {
                continue;
            }
            // `Wake`: the consumer waits for the signal, which must be raised by the time no
            // producer is left to raise it.
            while !doorbell.wait(Duration::ZERO).unwrap() {
                if finished.load(Ordering::Acquire) == producers {
                    // Every last ring happened before this, so a raise it made is seen here.
                    assert!(
                        doorbell.wait(Duration::ZERO).unwrap(),
                        "a wake was lost: {taken} taken of {} published, and the signal is low",
                        producers * each
                    );
                    break;
                }
                thread::yield_now();
            }
        }
        for ringer in ringers {
            ringer.join().unwrap();
        }
    });
}

#[test]
fn one_producer_ringing_twice_never_loses_a_wake() {
    doorbell(1, 2, None);
}

#[test]
fn two_producers_never_lose_a_wake() {
    doorbell(2, 1, None);
}
#[test]
#[should_panic(expected = "a wake was lost")]
fn a_test_that_does_not_release_loses_a_wake() {
    doorbell(1, 2, Some(Weakening::TestWithoutRelease));
}

#[test]
#[should_panic(expected = "a wake was lost")]
fn a_clear_that_does_not_acquire_loses_a_wake() {
    doorbell(1, 2, Some(Weakening::ClearWithoutAcquire));
}

#[test]
#[should_panic(expected = "a wake was lost")]
fn a_clear_that_is_a_store_loses_a_wake() {
    doorbell(1, 2, Some(Weakening::ClearAsStore));
}
