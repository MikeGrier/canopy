// Copyright (c) Michael Grier.

//! Recording the implementation of a root whose store the caller makes or changes itself
//! (canopy's DESIGN-NOTES.md, "Creating a store root, modelled in `Creation.tla`").

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use test_timers::mpsc::recv_timeout;
use test_timers::thread::sleep;

use super::creation::{nonce, reservation};
use super::{Backend, LONG, on_every_implementation};
use crate::coord::Waiting;
use crate::coord::file::FileFactory;
use crate::coord::file::reservation::Reservation;
use crate::coord::{CoordError, Coordination, CoordinationFactory, Created};
use crate::{Fault, FaultOp, FaultPolicy, FaultyFs, LockMode, LockSpan, Vfs, When};

const INPUTS: &[u8] = b"migration: store identity unchanged";
const OTHER_INPUTS: &[u8] = b"migration: another identity";
const MARKER: &str = "marker";

fn participant(
    implementation: &FileFactory,
    vfs: Arc<dyn Vfs>,
    root: &std::path::Path,
) -> Box<dyn Coordination> {
    implementation.open(vfs, root).expect("open")
}

#[test]
fn a_reservation_records_the_implementation_its_inputs_and_its_nonce() {
    on_every_implementation("coord-reserve", &|implementation, backend| {
        let (vfs, root) = (backend.process(), backend.root());
        let reserved = participant(implementation, Arc::clone(&vfs), &root)
            .reserve(INPUTS, nonce(4))
            .expect("reserve");
        assert_eq!(reserved, Created::Created, "{}", backend.name());
        let record = reservation(&*vfs, &root).expect("a reservation");
        assert_eq!(&record.implementation, implementation.implementation());
        assert_eq!(record.inputs, INPUTS);
        assert_eq!(record.nonce, nonce(4));
    });
}

#[test]
fn reserving_again_with_the_same_inputs_resumes_and_keeps_the_first_nonce() {
    on_every_implementation("coord-reserve-again", &|implementation, backend| {
        let (vfs, root) = (backend.process(), backend.root());
        for round in 0..3 {
            let reserved = participant(implementation, Arc::clone(&vfs), &root)
                .reserve(INPUTS, nonce(10 + round))
                .expect("reserve");
            let expected = if round == 0 {
                Created::Created
            } else {
                Created::Resumed
            };
            assert_eq!(reserved, expected, "{} round {round}", backend.name());
            assert_eq!(
                reservation(&*vfs, &root).expect("a reservation").nonce,
                nonce(10)
            );
        }
    });
}

#[test]
fn a_reservation_with_other_inputs_refuses_and_changes_nothing() {
    on_every_implementation("coord-reserve-inputs", &|implementation, backend| {
        let (vfs, root) = (backend.process(), backend.root());
        participant(implementation, Arc::clone(&vfs), &root)
            .reserve(INPUTS, nonce(1))
            .expect("reserve");
        let before = vfs
            .read(&reservation_path(&root))
            .expect("the reservation file");
        let refused =
            participant(implementation, Arc::clone(&vfs), &root).reserve(OTHER_INPUTS, nonce(2));
        assert!(
            matches!(refused, Err(CoordError::Reserved)),
            "{}",
            backend.name()
        );
        assert_eq!(vfs.read(&reservation_path(&root)).expect("kept"), before);
    });
}

#[test]
fn a_reservation_of_another_implementation_refuses_the_same_inputs() {
    on_every_implementation("coord-reserve-other", &|implementation, backend| {
        let (vfs, root) = (backend.process(), backend.root());
        participant(implementation, Arc::clone(&vfs), &root)
            .reserve(INPUTS, nonce(1))
            .expect("reserve");
        for other in super::implementations() {
            if other.implementation() == implementation.implementation() {
                continue;
            }
            let refused = participant(&other, Arc::clone(&vfs), &root).reserve(INPUTS, nonce(2));
            assert!(
                matches!(refused, Err(CoordError::Reserved)),
                "{}: {} against {}",
                backend.name(),
                other.implementation(),
                implementation.implementation()
            );
        }
        let record = reservation(&*vfs, &root).expect("a reservation");
        assert_eq!(&record.implementation, implementation.implementation());
    });
}

#[test]
fn reserving_reads_nothing_of_what_the_root_holds() {
    on_every_implementation("coord-reserve-holds", &|implementation, backend| {
        let (vfs, root) = (backend.process(), backend.root());
        for name in [MARKER, "files", "anything-at-all"] {
            vfs.write(&root.join(name), b"already here").expect("write");
        }
        assert_eq!(
            participant(implementation, Arc::clone(&vfs), &root)
                .reserve(INPUTS, nonce(1))
                .expect("reserve"),
            Created::Created,
            "{}",
            backend.name()
        );
        assert_eq!(vfs.read(&root.join(MARKER)).expect("kept"), b"already here");
    });
}

#[test]
fn a_reservation_is_what_confirming_a_created_root_looks_for() {
    on_every_implementation("coord-reserve-confirm", &|implementation, backend| {
        let (vfs, root) = (backend.process(), backend.root());
        let owner = participant(implementation, Arc::clone(&vfs), &root);
        assert!(
            matches!(
                owner.confirm_created(OsStr::new(MARKER)),
                Err(CoordError::NotCreated)
            ),
            "{}: nothing is reserved",
            backend.name()
        );
        owner.reserve(INPUTS, nonce(1)).expect("reserve");
        assert!(
            matches!(
                owner.confirm_created(OsStr::new(MARKER)),
                Err(CoordError::NotCreated)
            ),
            "reserved, but no marker"
        );
        vfs.write(&root.join(MARKER), b"marker").expect("write");
        owner
            .confirm_created(OsStr::new(MARKER))
            .expect("reserved, and the marker is there");
        for other in super::implementations() {
            if other.implementation() == implementation.implementation() {
                continue;
            }
            let mismatch =
                participant(&other, Arc::clone(&vfs), &root).confirm_created(OsStr::new(MARKER));
            assert!(
                matches!(mismatch, Err(CoordError::ImplementationMismatch { .. })),
                "{}",
                backend.name()
            );
        }
    });
}

#[test]
fn simultaneous_reservations_leave_exactly_one_owner() {
    const RACERS: usize = 6;
    for backend in [
        Backend::Mem(Backend::mem()),
        Backend::Os(super::TempDir::new("coord-reserve-race")),
    ] {
        let root = backend.root();
        let implementations = super::implementations();
        let start = Arc::new(Barrier::new(RACERS));
        let racers: Vec<_> = (0..RACERS)
            .map(|i| {
                let implementation = implementations[i % implementations.len()].clone();
                let inputs: &'static [u8] = if i % 3 == 0 { OTHER_INPUTS } else { INPUTS };
                let (vfs, root, start) = (backend.process(), root.clone(), Arc::clone(&start));
                thread::spawn(move || {
                    let participant = implementation.open(vfs, &root).expect("open");
                    start.wait();
                    participant
                        .reserve(inputs, nonce(u8::try_from(i).expect("small")))
                        .map(|created| (implementation.implementation().clone(), inputs, created))
                })
            })
            .collect();
        let outcomes: Vec<_> = racers
            .into_iter()
            .map(|racer| racer.join().expect("a racer"))
            .collect();
        let created: Vec<_> = outcomes
            .iter()
            .filter_map(|outcome| outcome.as_ref().ok())
            .filter(|(_, _, created)| *created == Created::Created)
            .collect();
        assert_eq!(created.len(), 1, "{}: {outcomes:?}", backend.name());
        let (owner, inputs, _) = created[0];
        for outcome in &outcomes {
            match outcome {
                Ok((implementation, racer_inputs, _)) => {
                    assert_eq!(implementation, owner, "{}", backend.name());
                    assert_eq!(racer_inputs, inputs);
                }
                Err(error) => assert!(matches!(error, CoordError::Reserved), "{error}"),
            }
        }
    }
}

#[test]
fn a_reservation_cut_short_by_a_failure_is_finished_by_the_next_call() {
    for implementation in super::implementations() {
        for failing_sync in 1..=2 {
            let mem = Backend::mem();
            let root = PathBuf::from(Backend::MEM_ROOT);
            let policy = FaultPolicy::new().inject(
                FaultOp::Fsync,
                &root,
                Fault::io_error(),
                When::OnCall(failing_sync),
            );
            let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
            let cut = participant(&implementation, vfs, &root);
            let first = cut.reserve(INPUTS, nonce(1));
            cut.abandon();
            let mem = mem.simulate_restart();
            let again = participant(&implementation, Arc::new(mem.another_process()), &root)
                .reserve(INPUTS, nonce(2))
                .unwrap_or_else(|error| panic!("{failing_sync}: {first:?} then {error}"));
            assert!(matches!(again, Created::Created | Created::Resumed));
            let mem = mem.simulate_restart();
            let kept = reservation(&mem, &root).expect("the reservation survives a crash");
            assert_eq!(kept.inputs, INPUTS, "{failing_sync}");
        }
    }
}

#[test]
fn a_reservation_that_returned_survives_a_crash() {
    for implementation in super::implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        participant(&implementation, Arc::new(mem.another_process()), &root)
            .reserve(INPUTS, nonce(1))
            .expect("reserve");
        let mem = mem.simulate_restart();
        let kept = reservation(&mem, &root).expect("the reservation is durable");
        assert_eq!(&kept.implementation, implementation.implementation());
        assert_eq!(kept.inputs, INPUTS);
    }
}

/// A reserver that read no reservation, then waited for its implementation's creation exclusion,
/// reads again under it: another implementation's reservation published meanwhile refuses it.
#[test]
fn a_reservation_published_while_a_reserver_waits_refuses_it() {
    let implementations = super::implementations();
    for (waiting, interloping) in [(0, 1), (1, 0)] {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let holder = implementations[waiting].participant(Arc::new(mem.another_process()), &root);
        let hold = holder
            .take_lock(
                &holder.layout().creation_lock(),
                LockMode::Exclusive,
                LockSpan::File,
                Waiting::Never,
            )
            .expect("the exclusion is free");
        let (done, finished) = mpsc::channel();
        let (implementation, vfs, waiting_root) = (
            implementations[waiting].clone(),
            Arc::new(mem.another_process()),
            root.clone(),
        );
        thread::spawn(move || {
            let reserver = implementation.open(vfs, &waiting_root).expect("open");
            let _ = done.send(reserver.reserve(INPUTS, nonce(2)));
        });
        sleep(Duration::from_millis(50));
        assert!(recv_timeout(&finished, Duration::ZERO).is_err(), "it waits");
        assert_eq!(
            participant(
                &implementations[interloping],
                Arc::new(mem.another_process()),
                &root
            )
            .reserve(INPUTS, nonce(3))
            .expect("the other implementation's exclusion is free"),
            Created::Created
        );
        drop(holder.release_lock(hold));
        let refused = recv_timeout(&finished, LONG).expect("let in");
        assert!(matches!(refused, Err(CoordError::Reserved)), "{refused:?}");
        let kept = reservation(&mem, &root).expect("a reservation");
        assert_eq!(
            &kept.implementation,
            implementations[interloping].implementation()
        );
    }
}

/// The exclusion's holder publishes this implementation's reservation and dies, so a reserver that
/// read none before waiting resumes the one it finds under the exclusion rather than refusing it.
#[test]
fn a_reserver_that_waited_resumes_the_reservation_the_holder_made() {
    for implementation in super::implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let holder = implementation.participant(Arc::new(mem.another_process()), &root);
        let hold = holder
            .take_lock(
                &holder.layout().creation_lock(),
                LockMode::Exclusive,
                LockSpan::File,
                Waiting::Never,
            )
            .expect("the exclusion is free");
        let (done, finished) = mpsc::channel();
        let (waiter, vfs, waiting_root) = (
            implementation.clone(),
            Arc::new(mem.another_process()),
            root.clone(),
        );
        thread::spawn(move || {
            let reserver = waiter.open(vfs, &waiting_root).expect("open");
            let _ = done.send(reserver.reserve(INPUTS, nonce(2)));
        });
        sleep(Duration::from_millis(50));
        assert!(recv_timeout(&finished, Duration::ZERO).is_err(), "it waits");
        let made = Reservation {
            implementation: implementation.implementation().clone(),
            nonce: nonce(1),
            inputs: INPUTS.to_vec(),
        };
        mem.publish_new(&holder.layout().reservation(), &made.encode())
            .expect("the holder publishes");
        drop(holder.release_lock(hold));
        assert_eq!(
            recv_timeout(&finished, LONG)
                .expect("let in")
                .expect("resumed"),
            Created::Resumed
        );
        assert_eq!(
            reservation(&mem, &root).expect("a reservation").nonce,
            nonce(1)
        );
    }
}

#[test]
fn an_abandoned_participant_reserves_nothing() {
    on_every_implementation("coord-reserve-abandoned", &|implementation, backend| {
        let (vfs, root) = (backend.process(), backend.root());
        let dying = participant(implementation, Arc::clone(&vfs), &root);
        dying.abandon();
        assert!(matches!(
            dying.reserve(INPUTS, nonce(1)),
            Err(CoordError::Abandoned)
        ));
        assert!(reservation(&*vfs, &root).is_none(), "{}", backend.name());
    });
}

fn reservation_path(root: &std::path::Path) -> PathBuf {
    FileFactory::new()
        .participant(Arc::new(crate::MemFs::new()), root)
        .layout()
        .reservation()
}
