// Copyright (c) Michael Grier.

//! Discovery of whether recovery is required: it reports abandoned work and never work in
//! progress, and it changes nothing (canopy's DESIGN-NOTES.md, "Recovery and its discovery,
//! modelled in `Coordination.tla`").

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::{
    Backend, implementations, on_every_implementation, participant, participant_on, returns_within,
};
use crate::Vfs;
use crate::coord::file::FileCoordination;
use crate::coord::file::restart::Standing;
use crate::coord::{
    CoordError, Coordination, ExclusionHold, ModificationEnd, Purpose, RecoveryDiscovery,
    RecoveryNeed, Untrusted, Waiting, WriterExclusion,
};

fn acquire(participant: &FileCoordination) -> Box<dyn ExclusionHold> {
    participant.acquire(Waiting::Never).expect("acquire")
}

/// Acquires and completes a recovery, leaving the state vouched for and the count settled.
fn recovered(participant: &FileCoordination) {
    let mut hold = acquire(participant);
    hold.complete_recovery().expect("recover");
    hold.release().expect("release");
}

#[test]
fn inspection_runs_only_while_a_probe_excludes_writers_and_changes_no_state() {
    on_every_implementation("coord-inspection", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        let other = participant(implementation, backend);
        for _ in 0..12 {
            let before = state_of(&mine);
            let mut inspected = false;
            mine.discover_recovery_with(&mut || {
                inspected = true;
                assert!(matches!(
                    other.acquire(Waiting::Never),
                    Err(CoordError::WouldWait)
                ));
            })
            .unwrap();
            assert!(inspected);
            assert_eq!(state_of(&mine), before);
            let hold = acquire(&other);
            inspected = false;
            mine.discover_recovery_with(&mut || inspected = true)
                .unwrap();
            assert!(!inspected);
            drop(hold);
        }
    });
}

#[test]
fn an_inspection_that_unwinds_releases_its_probe() {
    on_every_implementation("coord-inspection-unwind", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = mine.discover_recovery_with(&mut || panic!("inspection failed"));
            }))
            .is_err()
        );
        drop(acquire(&participant(implementation, backend)));
    });
}

#[test]
fn a_first_writer_that_appears_while_discovery_looks_is_not_reported_dead() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let writer = implementation.participant(Arc::new(mem.another_process()), &root);
        let fence = writer.layout().fence();
        let held: Arc<std::sync::Mutex<Option<Box<dyn ExclusionHold>>>> = Arc::default();
        let holding = Arc::clone(&held);
        let policy = crate::FaultPolicy::new().inject(
            crate::FaultOp::Metadata,
            &fence,
            crate::Fault::observe_with(move |_, _| {
                *holding.lock().unwrap() = Some(writer.acquire(Waiting::Never).expect("acquire"));
            }),
            crate::When::Once,
        );
        let vfs: Arc<dyn Vfs> = Arc::new(crate::FaultyFs::new(mem.another_process(), policy));
        let discovering = implementation.participant(vfs, &root);
        let need = discovering.discover_recovery().expect("discover");
        assert!(!need.holder_died(), "a live first writer was reported dead");
        assert!(!need.fenced());
    }
}

fn discover(participant: &FileCoordination) -> RecoveryNeed {
    participant.discover_recovery().expect("discover")
}

/// Every file of the state, with its contents, so a discovery can be shown to have changed none.
fn state_of(participant: &FileCoordination) -> Vec<(String, Vec<u8>)> {
    let dir = participant.layout().state_dir();
    let mut files: Vec<_> = participant
        .vfs()
        .read_dir(&dir)
        .expect("list the state")
        .into_iter()
        .map(|entry| {
            let bytes = participant
                .vfs()
                .read(&dir.join(&entry.file_name))
                .unwrap_or_default();
            (entry.file_name, bytes)
        })
        .collect();
    files.sort();
    files
}

#[test]
fn state_never_made_is_reported_untrusted_and_discovery_makes_none() {
    on_every_implementation("coord-discover-new", &|implementation, backend| {
        let participant = implementation.participant(backend.process(), &backend.root());
        let need = discover(&participant);
        assert!(need.required(), "{}", backend.name());
        assert_eq!(need.untrusted(), Some(Untrusted::Created));
        assert!(!need.fenced());
        assert!(!need.holder_died());
        assert!(
            !participant
                .vfs()
                .exists(&participant.layout().state_dir())
                .expect("exists"),
            "asking made no state directory"
        );
    });
}

#[test]
fn a_store_nobody_holds_and_a_recovery_vouched_for_requires_nothing() {
    on_every_implementation("coord-discover-healthy", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        for asker in [&mine, &participant(implementation, backend)] {
            let need = discover(asker);
            assert!(!need.required(), "{}: {need:?}", backend.name());
            assert_eq!(need.untrusted(), None);
        }
    });
}

#[test]
fn a_live_holders_transaction_is_not_reported_as_abandoned() {
    on_every_implementation("coord-discover-live", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        let mut hold = acquire(&mine);
        hold.begin_modification(Purpose::Ordinary).expect("begin");
        let owner_before = state_of(&mine);
        let other = participant(implementation, backend);
        let need = discover(&other);
        assert!(!need.required(), "{}: {need:?}", backend.name());
        assert!(!need.holder_died());
        assert_eq!(state_of(&mine), owner_before, "discovery changed the state");
        hold.end_modification(ModificationEnd::Settled)
            .expect("end");
        assert!(!discover(&other).required());
    });
}

#[test]
fn a_dead_holders_record_beside_a_free_lock_is_reported() {
    on_every_implementation("coord-discover-death", &|implementation, backend| {
        let dying = participant(implementation, backend);
        recovered(&dying);
        let mut hold = acquire(&dying);
        hold.begin_modification(Purpose::Ordinary).expect("begin");
        dying.abandon();
        drop(hold);
        let other = participant(implementation, backend);
        let need = discover(&other);
        assert!(need.required(), "{}", backend.name());
        assert!(need.holder_died());
        assert!(!need.fenced(), "only an acquirer sets the fence");
    });
}

#[test]
fn discovery_leaves_a_dead_holders_work_for_the_next_acquirer_to_be_told() {
    on_every_implementation("coord-discover-untouched", &|implementation, backend| {
        let dying = participant(implementation, backend);
        recovered(&dying);
        let hold = acquire(&dying);
        dying.abandon();
        drop(hold);
        let asker = participant(implementation, backend);
        let before = state_of(&asker);
        for _ in 0..3 {
            assert!(discover(&asker).holder_died());
        }
        assert_eq!(state_of(&asker), before, "{}", backend.name());

        let found = acquire(&participant(implementation, backend));
        assert!(found.acquired().holder_died());
        assert!(found.acquired().fenced());
        assert!(found.acquired().recovery_required());
    });
}

#[test]
fn a_fence_is_reported_with_exclusion_held_and_free_and_is_not_cleared_by_asking() {
    on_every_implementation("coord-discover-fence", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        let mut hold = acquire(&mine);
        hold.begin_modification(Purpose::Ordinary).expect("begin");
        hold.end_modification(ModificationEnd::Unresolved)
            .expect("end unresolved");
        let other = participant(implementation, backend);
        assert!(discover(&other).fenced(), "{}: held", backend.name());
        assert!(discover(&mine).fenced(), "the holder asking about itself");
        drop(hold);
        let need = discover(&other);
        assert!(need.fenced(), "{}: released", backend.name());
        assert!(need.required());
        assert!(!need.holder_died(), "a clean release cleared the record");
        assert!(discover(&other).fenced(), "asking did not clear it");
    });
}

#[test]
fn a_recovery_in_progress_is_reported_until_it_completes() {
    on_every_implementation("coord-discover-recovering", &|implementation, backend| {
        let dying = participant(implementation, backend);
        recovered(&dying);
        let hold = acquire(&dying);
        dying.abandon();
        drop(hold);

        let next = participant(implementation, backend);
        let mut hold = acquire(&next);
        let asker = participant(implementation, backend);
        let need = discover(&asker);
        assert!(
            need.fenced(),
            "{}: recovery has not finished",
            backend.name()
        );
        assert!(!need.holder_died(), "the lock is held by the recoverer");
        hold.begin_modification(Purpose::Recovery).expect("replay");
        hold.end_modification(ModificationEnd::Settled)
            .expect("end");
        assert!(discover(&asker).required(), "still reported while it runs");
        hold.complete_recovery().expect("complete");
        assert!(!discover(&asker).required(), "{}", backend.name());
        drop(hold);
        assert!(!discover(&asker).required());
    });
}

#[test]
fn a_holder_asking_about_its_own_record_is_a_live_holder_to_itself() {
    on_every_implementation("coord-discover-self", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        let hold = acquire(&mine);
        let need = discover(&mine);
        assert!(!need.required(), "{}: {need:?}", backend.name());
        assert!(!need.holder_died());
        drop(hold);
    });
}

#[test]
fn state_a_recovery_has_not_vouched_for_is_reported_even_when_nobody_died() {
    on_every_implementation("coord-discover-created", &|implementation, backend| {
        let mine = participant(implementation, backend);
        mine.record_boot(Standing::Untrusted).expect("made");
        let need = discover(&mine);
        assert_eq!(need.untrusted(), Some(Untrusted::Created));
        assert!(need.required());
        assert!(!need.fenced());
        assert!(!need.holder_died());
        mine.record_boot(Standing::Vouched).expect("vouched");
        assert!(!discover(&mine).required());
    });
}

#[test]
fn a_host_restart_is_reported() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let participant = participant_on(&implementation, Arc::new(mem.another_process()), &root);
        participant.record_boot(Standing::Vouched).expect("vouch");
        for path in [
            participant.layout().boot(),
            participant.layout().state_dir(),
            root.clone(),
        ] {
            mem.fsync(&path).expect("sync");
        }
        assert!(!discover(&participant).required());
        let mem = mem.simulate_restart();
        let after = participant_on(&implementation, Arc::new(mem.another_process()), &root);
        let need = discover(&after);
        assert_eq!(need.untrusted(), Some(Untrusted::HostRestarted));
        assert!(need.required());
    }
}

#[test]
fn asking_does_not_hold_up_an_acquirer_or_advance_the_generation() {
    on_every_implementation("coord-discover-free", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        let first = acquire(&mine);
        let generation = first.acquired().advanced_to();
        drop(first);
        let asker = participant(implementation, backend);
        for _ in 0..5 {
            let _ = discover(&asker);
        }
        let next = acquire(&participant(implementation, backend));
        assert_eq!(next.acquired().found(), generation, "{}", backend.name());
    });
}

#[test]
fn a_discovery_that_finds_exclusion_held_does_not_wait_for_it() {
    on_every_implementation("coord-discover-nowait", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        let hold = acquire(&mine);
        let asker = participant(implementation, backend);
        assert!(
            returns_within(Duration::from_secs(10), move || {
                let _ = asker.discover_recovery();
            }),
            "{}",
            backend.name()
        );
        drop(hold);
    });
}

#[test]
fn an_abandoned_participant_cannot_ask() {
    on_every_implementation("coord-discover-abandoned", &|implementation, backend| {
        let mine = participant(implementation, backend);
        mine.abandon();
        assert!(matches!(
            mine.discover_recovery(),
            Err(CoordError::Abandoned)
        ));
    });
}

#[test]
fn a_filesystem_failure_while_asking_is_an_error() {
    use crate::{Fault, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let policy = FaultPolicy::new().inject_matching(
            FaultOp::TryLock,
            PathMatcher::predicate(|_| true),
            Fault::io_error(),
            When::Always,
        );
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
        let participant = participant_on(&implementation, vfs, &root);
        assert!(matches!(
            participant.discover_recovery(),
            Err(CoordError::Vfs(_))
        ));
    }
}
