// Copyright (c) Michael Grier.

//! Notification when the modification count moves (canopy's DESIGN-NOTES.md, "A change to the
//! coordination state is delivered as a notification").

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use test_timers::mpsc::recv_timeout;
use test_timers::thread::sleep;

use super::{Backend, LONG, participant, participant_on};
use crate::coord::file::restart::Standing;
use crate::coord::file::{FileCoordination, FileFactory};
use crate::coord::{
    CoordError, Coordination, CoordinationFactory, CountNotification, ModCount, ModificationCount,
    Notice,
};
use crate::{Fault, FaultOp, FaultPolicy, FaultyFs, Vfs, When};

/// A deadline no case reaches.
fn far() -> Instant {
    Instant::now() + LONG * 4
}

/// Runs `case` for every implementation both as the platform notifies it and as the poller alone
/// does, over a fresh root on every backend.
fn on_every_implementation(tag: &str, case: &dyn Fn(&FileFactory, &Backend)) {
    for implementation in super::implementations() {
        for factory in [implementation.clone(), implementation.polled()] {
            super::on_every_backend(tag, &|backend| case(&factory, backend));
        }
    }
}

/// Publishes `value` as a vouched count of this boot, so it samples as itself.
fn publish(participant: &FileCoordination, value: u64) {
    participant.record_boot(Standing::Vouched).expect("vouch");
    participant
        .publish_count(value, Standing::Vouched)
        .expect("publish");
}

/// Registers through `participant` and returns the notice and what the ring sends.
fn registered(
    participant: &FileCoordination,
    sampled: ModCount,
    deadline: Instant,
) -> (Box<dyn Notice>, mpsc::Receiver<Instant>) {
    let (rang, rings) = mpsc::channel();
    let notice = participant
        .notify(
            sampled,
            deadline,
            Box::new(move || {
                let _ = rang.send(Instant::now());
            }),
        )
        .expect("notify");
    (notice, rings)
}

fn rings_within(rings: &mpsc::Receiver<Instant>, within: Duration) -> bool {
    recv_timeout(rings, within).is_ok()
}

#[test]
fn a_registration_rings_when_the_count_moves() {
    on_every_implementation("coord-notify-moves", &|implementation, backend| {
        let writer = participant(implementation, backend);
        publish(&writer, 4);
        let waiter = participant(implementation, backend);
        let sampled = waiter.sample().expect("sample");
        assert_eq!(sampled, ModCount::new(4));
        let (_notice, rings) = registered(&waiter, sampled, far());
        sleep(Duration::from_millis(20));
        assert!(!rings_within(&rings, Duration::ZERO), "nothing moved");
        writer.publish_count(5, Standing::Vouched).expect("publish");
        assert!(rings_within(&rings, LONG), "{}", backend.name());
    });
}

#[test]
fn a_registration_whose_count_already_differs_rings_at_once() {
    on_every_implementation("coord-notify-differs", &|implementation, backend| {
        let waiter = participant(implementation, backend);
        publish(&waiter, 8);
        let (_notice, rings) = registered(&waiter, ModCount::new(6), far());
        assert!(rings_within(&rings, LONG), "{}", backend.name());
    });
}

#[test]
fn a_registration_rings_when_its_deadline_passes_and_not_before() {
    on_every_implementation("coord-notify-deadline", &|implementation, backend| {
        let waiter = participant(implementation, backend);
        publish(&waiter, 2);
        let deadline = Instant::now() + Duration::from_millis(60);
        let (_notice, rings) = registered(&waiter, ModCount::new(2), deadline);
        let rang = recv_timeout(&rings, LONG).expect("the deadline rings it");
        assert!(rang >= deadline, "{}: rang early", backend.name());
    });
}

#[test]
fn a_registration_rings_once() {
    on_every_implementation("coord-notify-once", &|implementation, backend| {
        let waiter = participant(implementation, backend);
        publish(&waiter, 2);
        let (_notice, rings) = registered(&waiter, ModCount::new(0), far());
        assert!(rings_within(&rings, LONG));
        waiter.publish_count(4, Standing::Vouched).expect("publish");
        assert!(
            !rings_within(&rings, Duration::from_millis(100)),
            "{}: rang twice",
            backend.name()
        );
    });
}

#[test]
fn a_dropped_notice_never_rings() {
    on_every_implementation("coord-notify-dropped", &|implementation, backend| {
        let waiter = participant(implementation, backend);
        publish(&waiter, 2);
        let (notice, rings) = registered(
            &waiter,
            ModCount::new(2),
            Instant::now() + Duration::from_millis(40),
        );
        drop(notice);
        waiter.publish_count(4, Standing::Vouched).expect("publish");
        assert!(
            !rings_within(&rings, Duration::from_millis(150)),
            "{}",
            backend.name()
        );
    });
}

#[test]
fn many_registrations_on_many_participants_each_ring_for_their_own_reason() {
    on_every_implementation("coord-notify-many", &|implementation, backend| {
        let polled = implementation.clone().polled();
        let implementation = &polled;
        let writer = participant(implementation, backend);
        publish(&writer, 10);
        let waiters: Vec<_> = (0..6)
            .map(|_| participant(implementation, backend))
            .collect();
        let mut by_count = Vec::new();
        let mut by_deadline = Vec::new();
        for (i, waiter) in waiters.iter().enumerate() {
            for _ in 0..3 {
                if i % 2 == 0 {
                    by_count.push(registered(waiter, ModCount::new(10), far()));
                } else {
                    let deadline = Instant::now() + Duration::from_millis(30);
                    by_deadline.push((registered(waiter, ModCount::new(10), deadline), deadline));
                }
            }
        }
        for ((_notice, rings), deadline) in &by_deadline {
            let rang = recv_timeout(rings, LONG).expect("the deadline rings it");
            assert!(rang >= *deadline);
        }
        for (_notice, rings) in &by_count {
            assert!(!rings_within(rings, Duration::ZERO), "{}", backend.name());
        }
        writer
            .publish_count(11, Standing::Vouched)
            .expect("publish");
        for (_notice, rings) in &by_count {
            assert!(rings_within(rings, LONG), "{}", backend.name());
        }
    });
}

#[test]
fn a_participant_samples_nothing_while_no_registration_is_outstanding() {
    for implementation in super::implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let writer = participant_on(&implementation, Arc::new(mem.another_process()), &root);
        publish(&writer, 2);
        let reads = Arc::new(AtomicUsize::new(0));
        let policy = {
            let reads = Arc::clone(&reads);
            FaultPolicy::new().inject(
                FaultOp::Read,
                writer.layout().count(),
                Fault::observe_with(move |_, _| {
                    reads.fetch_add(1, Ordering::Relaxed);
                }),
                When::Always,
            )
        };
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
        let waiter = participant_on(&implementation, vfs, &root);
        sleep(Duration::from_millis(50));
        assert_eq!(reads.load(Ordering::Relaxed), 0, "nothing registered");
        let started = Instant::now();
        let deadline = started + Duration::from_millis(300);
        let (_notice, rings) = registered(&waiter, ModCount::new(2), deadline);
        recv_timeout(&rings, LONG).expect("the deadline rings it");
        let elapsed = started.elapsed();
        let sampled = reads.load(Ordering::Relaxed);
        assert!(sampled >= 1, "it was sampled while outstanding");
        let bound = 8 + usize::try_from(elapsed.as_millis() / 32).expect("small");
        assert!(
            sampled <= bound,
            "{sampled} samples in {elapsed:?}: the poller backs off"
        );
        sleep(Duration::from_millis(100));
        assert_eq!(
            reads.load(Ordering::Relaxed),
            sampled,
            "none once no registration is outstanding"
        );
        let (_notice, rings) = registered(&waiter, ModCount::new(0), far());
        recv_timeout(&rings, LONG).expect("the count differs");
        let sampled = reads.load(Ordering::Relaxed);
        sleep(Duration::from_millis(100));
        assert_eq!(
            reads.load(Ordering::Relaxed),
            sampled,
            "none once a registration has rung, though its notice is held"
        );
    }
}

#[test]
fn a_ring_that_panics_does_not_stop_later_notifications() {
    on_every_implementation("coord-notify-panic", &|implementation, backend| {
        let waiter = participant(implementation, backend);
        publish(&waiter, 2);
        let panicking = waiter
            .notify(
                ModCount::new(0),
                far(),
                Box::new(|| panic!("a ring that panics")),
            )
            .expect("notify");
        let (_notice, rings) = registered(&waiter, ModCount::new(0), far());
        assert!(rings_within(&rings, LONG), "{}", backend.name());
        drop(panicking);
        let (_notice, rings) = registered(&waiter, ModCount::new(0), far());
        assert!(rings_within(&rings, LONG), "{}", backend.name());
    });
}

#[test]
fn an_abandoned_participant_refuses_to_register_and_its_registrations_ring() {
    on_every_implementation("coord-notify-abandoned", &|implementation, backend| {
        let waiter = participant(implementation, backend);
        publish(&waiter, 2);
        let (_notice, rings) = registered(&waiter, ModCount::new(2), far());
        sleep(Duration::from_millis(20));
        assert!(!rings_within(&rings, Duration::ZERO));
        waiter.abandon();
        assert!(
            rings_within(&rings, LONG),
            "{}: a sample that fails rings, so the caller meets the failure",
            backend.name()
        );
        assert!(matches!(
            waiter.notify(ModCount::new(2), far(), Box::new(|| {})),
            Err(CoordError::Abandoned)
        ));
    });
}

#[test]
fn notification_is_reached_through_the_interface() {
    on_every_implementation("coord-notify-dyn", &|implementation, backend| {
        let writer = participant(implementation, backend);
        publish(&writer, 2);
        let coordination: Box<dyn Coordination> = implementation
            .open(backend.process(), &backend.root())
            .expect("open");
        let sampled = coordination.sample().expect("sample");
        let (rang, rings) = mpsc::channel();
        let _notice = coordination
            .notify(
                sampled,
                far(),
                Box::new(move || {
                    let _ = rang.send(Instant::now());
                }),
            )
            .expect("notify");
        writer.publish_count(4, Standing::Vouched).expect("publish");
        assert!(rings_within(&rings, LONG), "{}", backend.name());
    });
}

/// The notification the operating system gives on Windows: a lock broken by the rename of a fresh
/// count over the one published.
#[cfg(windows)]
mod os {
    use std::sync::Mutex;
    use std::thread;

    use super::*;
    use crate::OsFs;
    use crate::tests::TempDir;

    /// A deadline the cases reach only when the notification was lost.
    const LOST: Duration = Duration::from_secs(2);

    fn on_the_os(tag: &str) -> (TempDir, FileFactory) {
        (TempDir::new(tag), FileFactory::new())
    }

    fn os_participant(implementation: &FileFactory, dir: &TempDir) -> FileCoordination {
        participant_on(implementation, Arc::new(OsFs::new()), dir.path())
    }

    /// A participant over the operating system's filesystem that counts the reads of the count.
    fn counting(
        implementation: &FileFactory,
        dir: &TempDir,
        writer: &FileCoordination,
    ) -> (FileCoordination, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let policy = {
            let reads = Arc::clone(&reads);
            FaultPolicy::new().inject(
                FaultOp::Read,
                writer.layout().count(),
                Fault::observe_with(move |_, _| {
                    reads.fetch_add(1, Ordering::Relaxed);
                }),
                When::Always,
            )
        };
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(OsFs::new(), policy));
        (participant_on(implementation, vfs, dir.path()), reads)
    }

    #[test]
    fn a_break_rings_a_registration_that_nothing_polls_and_no_thread_of_ours_runs() {
        let (dir, implementation) = on_the_os("coord-notify-oplock-break");
        let writer = os_participant(&implementation, &dir);
        publish(&writer, 4);
        let (waiter, reads) = counting(&implementation, &dir, &writer);
        let sampled = waiter.sample().expect("sample");
        let (rang, rings) = mpsc::channel();
        let _notice = waiter
            .notify(
                sampled,
                far(),
                Box::new(move || {
                    let _ = rang.send((Instant::now(), thread::current().name().map(String::from)));
                }),
            )
            .expect("notify");
        let registered = reads.load(Ordering::Relaxed);
        sleep(Duration::from_millis(300));
        assert_eq!(
            reads.load(Ordering::Relaxed),
            registered,
            "a registration the system notifies samples nothing while it waits"
        );
        assert!(rings.try_recv().is_err(), "nothing moved");

        let published = Instant::now();
        writer.publish_count(5, Standing::Vouched).expect("publish");
        let (at, name) = recv_timeout(&rings, LONG).expect("the break rings it");
        assert!(at.duration_since(published) < LOST, "rung by the break");
        assert_ne!(
            name.as_deref(),
            Some("acadia-coord-poller"),
            "not rung by the poller"
        );
        assert_ne!(name, thread::current().name().map(String::from));
    }

    #[test]
    fn a_registration_before_any_count_is_published_is_rung_by_the_poller() {
        let (dir, implementation) = on_the_os("coord-notify-oplock-fallback");
        let writer = os_participant(&implementation, &dir);
        let waiter = os_participant(&implementation, &dir);
        let sampled = waiter.sample().expect("sample");
        let (_notice, rings) = registered(&waiter, sampled, far());
        sleep(Duration::from_millis(20));
        assert!(!rings_within(&rings, Duration::ZERO), "nothing moved");
        publish(&writer, 2);
        assert!(rings_within(&rings, LONG), "the poller notices it");
    }

    #[test]
    fn a_writer_is_not_held_up_by_registrations_the_system_notifies() {
        let (dir, implementation) = on_the_os("coord-notify-oplock-writer");
        let writer = os_participant(&implementation, &dir);
        publish(&writer, 2);
        let waiters: Vec<_> = (0..6)
            .map(|_| os_participant(&implementation, &dir))
            .collect();
        for value in (4..44).step_by(2) {
            let held: Vec<_> = waiters
                .iter()
                .map(|waiter| registered(waiter, waiter.sample().expect("sample"), far()))
                .collect();
            let started = Instant::now();
            writer
                .publish_count(value, Standing::Vouched)
                .expect("publish");
            assert!(
                started.elapsed() < LOST,
                "{value}: a publication waited for a registration to answer its break"
            );
            for (_notice, rings) in &held {
                assert!(rings_within(rings, LONG), "{value}");
            }
        }
    }

    #[test]
    fn no_wake_is_lost_while_a_writer_publishes_without_pause() {
        let (dir, implementation) = on_the_os("coord-notify-oplock-race");
        let writer = os_participant(&implementation, &dir);
        publish(&writer, 2);
        let waiter = os_participant(&implementation, &dir);
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let publisher = {
            let running = Arc::clone(&running);
            thread::spawn(move || {
                let mut value = 4;
                while running.load(Ordering::Acquire) {
                    writer
                        .publish_count(value, Standing::Vouched)
                        .expect("publish");
                    value += 2;
                }
            })
        };
        let mut slowest = Duration::ZERO;
        for _ in 0..40 {
            let sampled = waiter.sample().expect("sample");
            let started = Instant::now();
            let (_notice, rings) = registered(&waiter, sampled, started + LOST);
            recv_timeout(&rings, LONG).expect("every registration rings");
            slowest = slowest.max(started.elapsed());
        }
        running.store(false, Ordering::Release);
        publisher.join().expect("the publisher ran");
        assert!(
            slowest < LOST,
            "a registration waited for its deadline although the count kept moving: {slowest:?}"
        );
    }

    #[test]
    fn a_notice_dropped_by_its_own_ring_does_not_wait_for_itself() {
        let (dir, implementation) = on_the_os("coord-notify-oplock-self");
        let writer = os_participant(&implementation, &dir);
        publish(&writer, 2);
        let waiter = os_participant(&implementation, &dir);
        let slot: Arc<Mutex<Option<Box<dyn Notice>>>> = Arc::default();
        let (done, finished) = mpsc::channel();
        let notice = {
            let slot = Arc::clone(&slot);
            waiter
                .notify(
                    waiter.sample().expect("sample"),
                    far(),
                    Box::new(move || {
                        drop(slot.lock().unwrap().take());
                        let _ = done.send(Instant::now());
                    }),
                )
                .expect("notify")
        };
        *slot.lock().unwrap() = Some(notice);
        writer.publish_count(4, Standing::Vouched).expect("publish");
        assert!(rings_within(&finished, LONG), "the ring returned");
    }
}

#[test]
fn a_registration_whose_retirement_withdraws_another_notice_does_not_stall_the_poller() {
    use std::sync::Mutex;

    let mem = Backend::mem();
    let root = PathBuf::from(Backend::MEM_ROOT);
    let other = participant_on(&FileFactory::new(), Arc::new(mem.another_process()), &root);
    let slot: Arc<Mutex<Option<Box<dyn Notice>>>> = Arc::default();
    // The backend's only owner is the registration below, so retiring it drops this observer,
    // and with it the notice in `slot`, whose drop withdraws from the poller.
    let held = Arc::clone(&slot);
    let policy = FaultPolicy::new().inject(
        FaultOp::Read,
        "/never-read",
        Fault::observe_with(move |_, _| {
            let _ = &held;
        }),
        When::Always,
    );
    let owner = participant_on(
        &FileFactory::new(),
        Arc::new(FaultyFs::new(mem.another_process(), policy)),
        &root,
    );
    let (inner, _) = registered(&other, other.sample().expect("sample"), far());
    *slot.lock().unwrap() = Some(inner);
    drop(slot);
    let (_retiring, rings) = registered(&owner, owner.sample().expect("sample"), Instant::now());
    drop(owner);
    assert!(
        rings_within(&rings, LONG),
        "the retiring registration rings"
    );
    let (_after, rings) = registered(&other, other.sample().expect("sample"), Instant::now());
    assert!(
        rings_within(&rings, LONG),
        "the poller keeps serving registrations"
    );
}
