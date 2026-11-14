// Copyright (c) Michael Grier.

//! Writer exclusion: at most one holder across every thread of every process, waiting without
//! order, released when its holder dies (canopy's DESIGN-NOTES.md, "Writer exclusion and its
//! ownership record, modelled in `Coordination.tla`").

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use super::{LONG, on_every_implementation, participant, participant_on, returns_within};
use crate::coord::{CoordError, Coordination, Waiting};

#[test]
fn a_held_exclusion_refuses_every_other_participant_that_will_not_wait() {
    on_every_implementation("coord-excl-refuse", &|implementation, backend| {
        let holder = participant(implementation, backend);
        let _held = holder.acquire_exclusion(Waiting::Never).expect("acquire");
        for _ in 0..3 {
            let other = participant(implementation, backend);
            assert!(
                matches!(
                    other.acquire_exclusion(Waiting::Never),
                    Err(CoordError::WouldWait)
                ),
                "{}",
                backend.name()
            );
        }
    });
}

#[test]
fn exclusion_is_not_reentrant() {
    on_every_implementation("coord-excl-reenter", &|implementation, backend| {
        let holder = participant(implementation, backend);
        let _held = holder.acquire_exclusion(Waiting::Never).expect("acquire");
        assert!(matches!(
            holder.acquire_exclusion(Waiting::Never),
            Err(CoordError::WouldWait)
        ));
    });
}

#[test]
fn a_dropped_exclusion_lets_the_next_participant_in() {
    on_every_implementation("coord-excl-drop", &|implementation, backend| {
        let first = participant(implementation, backend);
        let second = participant(implementation, backend);
        for _ in 0..5 {
            drop(first.acquire_exclusion(Waiting::Never).expect("first"));
            drop(second.acquire_exclusion(Waiting::Never).expect("second"));
        }
    });
}

#[test]
fn a_waiting_participant_is_let_in_by_a_release() {
    on_every_implementation("coord-excl-wait", &|implementation, backend| {
        let holder = participant(implementation, backend);
        let waiter = participant(implementation, backend);
        let held = holder.acquire_exclusion(Waiting::Never).expect("acquire");
        let waiting = thread::spawn(move || {
            drop(waiter.acquire_exclusion(Waiting::Wait).expect("let in"));
        });
        drop(held);
        assert!(
            returns_within(LONG, move || waiting.join().expect("the waiter")),
            "{}",
            backend.name()
        );
    });
}

#[test]
fn many_threads_each_hold_exclusion_alone() {
    const THREADS: usize = 6;
    const ROUNDS: usize = 40;
    on_every_implementation("coord-excl-threads", &|implementation, backend| {
        let inside = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(THREADS));
        let threads: Vec<_> = (0..THREADS)
            .map(|_| {
                let participant = participant(implementation, backend);
                let (inside, entered, start) = (
                    Arc::clone(&inside),
                    Arc::clone(&entered),
                    Arc::clone(&start),
                );
                thread::spawn(move || {
                    start.wait();
                    for _ in 0..ROUNDS {
                        let held = participant
                            .acquire_exclusion(Waiting::Wait)
                            .expect("acquire");
                        assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0, "two holders");
                        entered.fetch_add(1, Ordering::SeqCst);
                        inside.fetch_sub(1, Ordering::SeqCst);
                        drop(held);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("a contender");
        }
        assert_eq!(
            entered.load(Ordering::SeqCst),
            THREADS * ROUNDS,
            "{}",
            backend.name()
        );
    });
}

#[test]
fn releases_overlapping_tries_never_admit_two_holders() {
    const ROUNDS: usize = 200;
    on_every_implementation("coord-excl-overlap", &|implementation, backend| {
        let inside = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(2));
        let contenders: Vec<_> = (0..2)
            .map(|_| {
                let participant = participant(implementation, backend);
                let (inside, start) = (Arc::clone(&inside), Arc::clone(&start));
                thread::spawn(move || {
                    start.wait();
                    let mut held_at_least_once = false;
                    let mut rounds = 0;
                    // A contender may lose every try while the other is trying too; once the other
                    // has finished, its next try succeeds.
                    while rounds < ROUNDS || !held_at_least_once {
                        rounds += 1;
                        match participant.acquire_exclusion(Waiting::Never) {
                            Ok(held) => {
                                assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0);
                                inside.fetch_sub(1, Ordering::SeqCst);
                                drop(held);
                                held_at_least_once = true;
                            }
                            Err(CoordError::WouldWait) => {}
                            Err(error) => panic!("{error}"),
                        }
                    }
                })
            })
            .collect();
        for contender in contenders {
            contender
                .join()
                .unwrap_or_else(|_| panic!("{}: a contender", backend.name()));
        }
    });
}

#[test]
fn abandoning_the_holder_releases_exclusion() {
    on_every_implementation("coord-excl-abandon", &|implementation, backend| {
        let dying = participant(implementation, backend);
        let held = dying.acquire_exclusion(Waiting::Never).expect("acquire");
        dying.abandon();
        let next = participant(implementation, backend);
        let next_held = next
            .acquire_exclusion(Waiting::Never)
            .unwrap_or_else(|error| panic!("{}: released by the death: {error}", backend.name()));
        drop(held);
        let third = participant(implementation, backend);
        assert!(
            matches!(
                third.acquire_exclusion(Waiting::Never),
                Err(CoordError::WouldWait)
            ),
            "dropping a dead holder's hold releases nothing the next holder holds"
        );
        drop(next_held);
        assert!(matches!(
            dying.acquire_exclusion(Waiting::Never),
            Err(CoordError::Abandoned)
        ));
    });
}

#[test]
fn a_hold_outlives_the_borrow_it_was_taken_through() {
    on_every_implementation("coord-excl-outlive", &|implementation, backend| {
        let held = {
            let holder = participant(implementation, backend);
            holder.acquire_exclusion(Waiting::Never).expect("acquire")
        };
        let other = participant(implementation, backend);
        assert!(matches!(
            other.acquire_exclusion(Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        assert_eq!(
            held.participant().layout().exclusion_lock(),
            other.layout().exclusion_lock()
        );
        drop(held);
        other.acquire_exclusion(Waiting::Never).expect("released");
    });
}

#[test]
fn exclusion_of_one_root_does_not_exclude_another() {
    on_every_implementation("coord-excl-roots", &|implementation, backend| {
        let here = participant(implementation, backend);
        let _held = here.acquire_exclusion(Waiting::Never).expect("acquire");
        let vfs = backend.process();
        let elsewhere = backend.root().join("another");
        vfs.create_dir_all(&elsewhere).expect("another root");
        let there = participant_on(implementation, vfs, &elsewhere);
        there
            .acquire_exclusion(Waiting::Never)
            .expect("a second root has an exclusion of its own");
    });
}
