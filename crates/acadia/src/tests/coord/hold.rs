// Copyright (c) Michael Grier.

//! The hold of writer exclusion: the ownership record, the fence, the generation, the boot record
//! kept under it, and raising the count through it (canopy's DESIGN-NOTES.md, "Writer exclusion and
//! its ownership record, modelled in `Coordination.tla`").

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use super::{Backend, implementations, on_every_implementation, participant, participant_on};
use crate::coord::file::{FileCoordination, FileFactory};
use crate::coord::{
    CoordError, Coordination, CoordinationFactory, ExclusionHold, Generation, ModCount,
    ModificationCount, ModificationEnd, Purpose, RestartRecognition, Trust, Untrusted, Waiting,
    WriterExclusion,
};
use crate::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, Vfs, When};

fn acquire(participant: &FileCoordination) -> Box<dyn ExclusionHold> {
    participant.acquire(Waiting::Never).expect("acquire")
}

/// Acquires and completes a recovery, leaving the state vouched for and the count settled.
fn recovered(participant: &FileCoordination) {
    let mut hold = acquire(participant);
    hold.complete_recovery().expect("recover");
    hold.release().expect("release");
}

fn owner_recorded(participant: &FileCoordination) -> bool {
    participant
        .vfs()
        .exists(&participant.layout().owner())
        .expect("exists")
}

#[test]
fn the_first_acquisition_of_new_state_requires_recovery_and_makes_the_boot_record() {
    on_every_implementation("coord-hold-first", &|implementation, backend| {
        let participant = participant(implementation, backend);
        let hold = acquire(&participant);
        let acquired = hold.acquired();
        assert_eq!(acquired.found(), Generation::new(0));
        assert_eq!(acquired.advanced_to(), Generation::new(1));
        assert!(!acquired.holder_died());
        assert!(!acquired.fenced());
        assert_eq!(acquired.trust(), Trust::Untrusted(Untrusted::Created));
        assert!(acquired.recovery_required(), "{}", backend.name());
        assert_eq!(
            participant.trust().expect("trust"),
            Trust::Untrusted(Untrusted::Created)
        );
    });
}

#[test]
fn each_acquisition_reports_the_generation_it_found_and_advances_it() {
    on_every_implementation("coord-hold-generation", &|implementation, backend| {
        let mine = participant(implementation, backend);
        let other = participant(implementation, backend);
        let first = acquire(&mine).acquired().advanced_to();
        let again = acquire(&mine);
        assert_eq!(again.acquired().found(), first, "nobody came between");
        let mine_last = again.acquired().advanced_to();
        drop(again);
        drop(acquire(&other));
        let after_other = acquire(&mine);
        assert_ne!(
            after_other.acquired().found(),
            mine_last,
            "another holder came between"
        );
        assert!(after_other.acquired().advanced_to() > after_other.acquired().found());
    });
}

#[test]
fn a_clean_release_clears_the_ownership_record_before_dropping_the_lock() {
    on_every_implementation("coord-hold-release", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        let hold = acquire(&mine);
        assert!(owner_recorded(&mine));
        hold.release().expect("release");
        assert!(!owner_recorded(&mine));
        let next = acquire(&participant(implementation, backend));
        assert!(!next.acquired().holder_died());
        assert!(!next.acquired().recovery_required(), "{}", backend.name());
    });
}

#[test]
fn a_dropped_hold_releases_as_cleanly_as_a_released_one() {
    on_every_implementation("coord-hold-drop", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        drop(acquire(&mine));
        assert!(!owner_recorded(&mine));
        assert!(!acquire(&mine).acquired().holder_died());
    });
}

#[test]
fn a_holder_that_dies_is_reported_to_the_next_acquirer_and_fenced() {
    on_every_implementation("coord-hold-death", &|implementation, backend| {
        let dying = participant(implementation, backend);
        recovered(&dying);
        let hold = acquire(&dying);
        dying.abandon();
        drop(hold);
        let next = participant(implementation, backend);
        let found = acquire(&next);
        assert!(found.acquired().holder_died(), "{}", backend.name());
        assert!(found.acquired().fenced());
        assert!(found.acquired().recovery_required());
    });
}

#[test]
fn the_fence_outlives_the_clean_release_of_the_holder_that_found_a_death() {
    on_every_implementation("coord-hold-fence-survives", &|implementation, backend| {
        let dying = participant(implementation, backend);
        recovered(&dying);
        let _dead = acquire(&dying);
        dying.abandon();
        let finder = participant(implementation, backend);
        let found = acquire(&finder);
        assert!(found.acquired().holder_died());
        found.release().expect("a clean release");
        let third = acquire(&participant(implementation, backend));
        assert!(!third.acquired().holder_died(), "the record was cleared");
        assert!(
            third.acquired().fenced(),
            "{}: the fence was not",
            backend.name()
        );
        assert!(third.acquired().recovery_required());
    });
}

#[test]
fn a_live_holder_releasing_is_never_reported_dead() {
    const ROUNDS: usize = 30;
    on_every_implementation("coord-hold-no-false-death", &|implementation, backend| {
        recovered(&participant(implementation, backend));
        let reported_dead = Arc::new(AtomicBool::new(false));
        let start = Arc::new(Barrier::new(2));
        let contenders: Vec<_> = (0..2)
            .map(|_| {
                let participant = participant(implementation, backend);
                let (reported_dead, start) = (Arc::clone(&reported_dead), Arc::clone(&start));
                thread::spawn(move || {
                    start.wait();
                    for _ in 0..ROUNDS {
                        let hold = participant.acquire(Waiting::Wait).expect("acquire");
                        if hold.acquired().holder_died() {
                            reported_dead.store(true, Ordering::SeqCst);
                        }
                        hold.release().expect("release");
                    }
                })
            })
            .collect();
        for contender in contenders {
            contender.join().expect("a contender");
        }
        assert!(!reported_dead.load(Ordering::SeqCst), "{}", backend.name());
    });
}

#[test]
fn an_ordinary_modification_waits_for_recovery_and_a_recovery_one_does_not() {
    on_every_implementation("coord-hold-recovery-gate", &|implementation, backend| {
        let mine = participant(implementation, backend);
        let mut hold = acquire(&mine);
        assert!(hold.acquired().recovery_required());
        assert!(matches!(
            hold.begin_modification(Purpose::Ordinary),
            Err(CoordError::RecoveryRequired)
        ));
        hold.begin_modification(Purpose::Recovery)
            .expect("recovery's own replay");
        assert!(matches!(
            hold.complete_recovery(),
            Err(CoordError::ModificationOpen)
        ));
        hold.end_modification(ModificationEnd::Settled)
            .expect("end");
        assert!(
            !mine.sample().expect("sample").is_settled(),
            "untrusted until recovery"
        );
        hold.complete_recovery().expect("recover");
        assert!(mine.sample().expect("sample").is_settled());
        assert_eq!(mine.trust().expect("trust"), Trust::Vouched);
        hold.begin_modification(Purpose::Ordinary)
            .expect("ordinary once recovered");
        hold.end_modification(ModificationEnd::Settled)
            .expect("end");
    });
}

#[test]
fn a_completed_recovery_clears_the_fence_and_settles_the_count() {
    on_every_implementation("coord-hold-recover", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        let mut hold = acquire(&mine);
        hold.begin_modification(Purpose::Ordinary).expect("begin");
        hold.end_modification(ModificationEnd::Unresolved)
            .expect("unresolved");
        assert!(matches!(
            hold.begin_modification(Purpose::Ordinary),
            Err(CoordError::RecoveryRequired)
        ));
        let odd = mine.sample().expect("sample");
        assert!(!odd.is_settled());
        hold.complete_recovery().expect("recover");
        assert_eq!(mine.sample().expect("sample"), ModCount::new(odd.get() + 1));
        hold.release().expect("release");
        let next = acquire(&participant(implementation, backend));
        assert!(!next.acquired().fenced(), "{}", backend.name());
        assert!(!next.acquired().recovery_required());
    });
}

#[test]
fn modifications_through_the_hold_raise_the_count_once() {
    on_every_implementation("coord-hold-raise", &|implementation, backend| {
        let mine = participant(implementation, backend);
        let sampler = participant(implementation, backend);
        recovered(&mine);
        let before = sampler.sample().expect("sample");
        let mut hold = acquire(&mine);
        hold.begin_modification(Purpose::Ordinary).expect("outer");
        hold.begin_modification(Purpose::Ordinary).expect("inner");
        hold.end_modification(ModificationEnd::Settled)
            .expect("inner");
        assert_eq!(
            sampler.sample().expect("sample"),
            ModCount::new(before.get() + 1)
        );
        hold.end_modification(ModificationEnd::Settled)
            .expect("outer");
        assert_eq!(
            sampler.sample().expect("sample"),
            ModCount::new(before.get() + 2)
        );
        assert!(matches!(
            hold.end_modification(ModificationEnd::Settled),
            Err(CoordError::NoModificationOpen)
        ));
    });
}

#[test]
fn a_holder_told_to_wait_is_refused_while_another_holds() {
    on_every_implementation("coord-hold-never", &|implementation, backend| {
        let _held = acquire(&participant(implementation, backend));
        assert!(matches!(
            participant(implementation, backend).acquire(Waiting::Never),
            Err(CoordError::WouldWait)
        ));
    });
}

#[test]
fn an_abandoned_holder_does_no_bookkeeping() {
    on_every_implementation("coord-hold-abandoned", &|implementation, backend| {
        let dying = participant(implementation, backend);
        let mut hold = acquire(&dying);
        dying.abandon();
        assert!(matches!(
            hold.begin_modification(Purpose::Recovery),
            Err(CoordError::Abandoned)
        ));
        hold.release()
            .expect("a dead holder's release reports nothing");
        assert!(owner_recorded(&dying), "a death leaves the record");
        assert!(matches!(
            dying.acquire(Waiting::Never),
            Err(CoordError::Abandoned)
        ));
    });
}

#[test]
fn a_restart_requires_recovery_and_remakes_the_boot_record() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        let mine = participant_on(&implementation, Arc::new(mem.another_process()), &root);
        recovered(&mine);
        let layout = mine.layout();
        for path in [
            layout.boot(),
            layout.count(),
            layout.state_dir(),
            root.clone(),
        ] {
            mem.fsync(&path).expect("sync");
        }
        mem.simulate_crash();
        let after = participant_on(&implementation, Arc::new(mem.another_process()), &root);
        let hold = acquire(&after);
        assert_eq!(
            hold.acquired().trust(),
            Trust::Untrusted(Untrusted::HostRestarted)
        );
        assert!(hold.acquired().recovery_required());
        assert_eq!(
            after.trust().expect("trust"),
            Trust::Untrusted(Untrusted::Created),
            "remade in this boot, untrusted"
        );
    }
}

/// A participant whose `op` on the state files named with `prefix` fails once `gate` opens.
fn failing(
    implementation: &FileFactory,
    mem: &crate::MemFs,
    op: FaultOp,
    prefix: &str,
    gate: &FaultGate,
) -> FileCoordination {
    let policy = FaultPolicy::new().inject_gated(
        op,
        PathMatcher::FileNamePrefix(prefix.into()),
        Fault::io_error(),
        When::Always,
        gate.clone(),
    );
    participant_on(
        implementation,
        Arc::new(FaultyFs::new(mem.another_process(), policy)),
        &PathBuf::from(Backend::MEM_ROOT),
    )
}

#[test]
fn an_acquirer_that_fails_before_writing_its_record_leaves_the_dead_holders_for_the_next() {
    for implementation in implementations() {
        for (op, prefix) in [
            (FaultOp::Write, "fence"),
            (FaultOp::Write, "generation.staging."),
            (FaultOp::Write, "owner.staging."),
        ] {
            let mem = Backend::mem();
            let root = PathBuf::from(Backend::MEM_ROOT);
            let dying = participant_on(&implementation, Arc::new(mem.another_process()), &root);
            recovered(&dying);
            let _dead = acquire(&dying);
            dying.abandon();
            let gate = FaultGate::opened();
            let unlucky = failing(&implementation, &mem, op, prefix, &gate);
            assert!(unlucky.acquire(Waiting::Never).is_err(), "{prefix}");
            gate.close();
            let next = participant_on(&implementation, Arc::new(mem.another_process()), &root);
            let found = acquire(&next);
            assert!(found.acquired().holder_died(), "{prefix}");
            assert!(found.acquired().recovery_required(), "{prefix}");
        }
    }
}

#[test]
fn a_release_that_cannot_clear_its_record_costs_the_next_acquirer_a_recovery() {
    for implementation in implementations() {
        let mem = Backend::mem();
        let root = PathBuf::from(Backend::MEM_ROOT);
        recovered(&participant_on(
            &implementation,
            Arc::new(mem.another_process()),
            &root,
        ));
        let gate = FaultGate::closed();
        let holder = failing(&implementation, &mem, FaultOp::RemoveFile, "owner", &gate);
        let hold = acquire(&holder);
        gate.open();
        assert!(hold.release().is_err());
        let next = participant_on(&implementation, Arc::new(mem.another_process()), &root);
        let found = next
            .acquire(Waiting::Never)
            .expect("the lock was released whatever the outcome");
        assert!(found.acquired().holder_died());
        assert!(found.acquired().recovery_required());
    }
}

#[test]
fn an_unreadable_generation_in_this_boot_is_refused() {
    on_every_implementation("coord-hold-garbled", &|implementation, backend| {
        let mine = participant(implementation, backend);
        recovered(&mine);
        mine.vfs()
            .write(&mine.layout().generation(), b"garbage")
            .expect("garble");
        assert!(matches!(
            mine.acquire(Waiting::Never),
            Err(CoordError::Malformed { what: "generation" })
        ));
        assert!(
            !owner_recorded(&mine),
            "a refused acquisition leaves no record"
        );
    });
}

#[test]
fn exclusion_is_reached_through_the_interface() {
    for implementation in implementations() {
        let backend = Backend::Mem(Backend::mem());
        let participant: Box<dyn Coordination> = implementation
            .open(backend.process(), &backend.root())
            .expect("open");
        let mut hold = participant.acquire(Waiting::Never).expect("acquire");
        hold.complete_recovery().expect("recover");
        hold.begin_modification(Purpose::Ordinary).expect("begin");
        hold.end_modification(ModificationEnd::Settled)
            .expect("end");
        hold.release().expect("release");
        assert!(participant.sample().expect("sample").is_settled());
    }
}
