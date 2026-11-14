// Copyright (c) Michael Grier.

//! Participants: their identity, their separateness, and abandonment.

use std::collections::HashSet;
use std::sync::Arc;

use super::{
    LONG, on_every_backend, on_every_implementation, participant, participant_on, returns_within,
};
use crate::coord::file::FileFactory;
use crate::coord::file::layout::Layout;
use crate::coord::{CoordError, Coordination, CoordinationFactory, Waiting};
use crate::{LockMode, LockSpan};

#[test]
fn a_participant_belongs_to_the_implementation_that_made_it() {
    on_every_implementation("coord-identity", &|implementation, backend| {
        let participant = implementation
            .open(backend.process(), &backend.root())
            .expect("open");
        assert_eq!(
            participant.implementation(),
            implementation.implementation(),
            "{}",
            backend.name()
        );
    });
}

#[test]
fn no_two_implementations_share_an_identity() {
    let identities = super::identities();
    let distinct: HashSet<_> = identities.iter().collect();
    assert_eq!(distinct.len(), identities.len());
}

#[test]
fn making_a_participant_changes_nothing_in_the_root() {
    on_every_implementation("coord-open", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        let _participant = implementation.open(Arc::clone(&vfs), &root).expect("open");
        assert!(
            vfs.read_dir(&root).expect("list the root").is_empty(),
            "{}",
            backend.name()
        );
    });
}

#[test]
fn two_participants_in_one_process_exclude_each_other() {
    on_every_implementation("coord-separate", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        let first = participant_on(implementation, Arc::clone(&vfs), &root);
        let second = participant_on(implementation, vfs, &root);
        let lock = first.layout().exclusion_lock();
        first
            .take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never)
            .expect("the first takes it");
        assert!(
            matches!(
                second.take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never),
                Err(CoordError::WouldWait)
            ),
            "{}",
            backend.name()
        );
    });
}

#[test]
fn a_hold_released_lets_the_next_participant_in() {
    on_every_implementation("coord-release", &|implementation, backend| {
        let first = participant(implementation, backend);
        let second = participant(implementation, backend);
        let lock = first.layout().exclusion_lock();
        let hold = first
            .take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never)
            .expect("the first takes it");
        drop(first.release_lock(hold).expect("the hold is in the table"));
        second
            .take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never)
            .expect("the second takes it once released");
        assert!(
            first.release_lock(hold).is_none(),
            "a hold is released once"
        );
    });
}

#[test]
fn a_waiting_participant_is_let_in_by_a_release() {
    on_every_implementation("coord-wait", &|implementation, backend| {
        let first = participant(implementation, backend);
        let second = participant(implementation, backend);
        let lock = first.layout().exclusion_lock();
        let hold = first
            .take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never)
            .expect("the first takes it");
        let waiter = std::thread::spawn(move || {
            second
                .take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Wait)
                .expect("the second is let in");
        });
        drop(first.release_lock(hold));
        assert!(
            returns_within(LONG, move || waiter.join().expect("the waiter")),
            "{}",
            backend.name()
        );
    });
}

#[test]
fn abandoning_a_participant_releases_every_hold_it_has() {
    on_every_implementation("coord-abandon", &|implementation, backend| {
        let dying = participant(implementation, backend);
        let other = participant(implementation, backend);
        let layout = dying.layout().clone();
        for (path, span) in [
            (layout.exclusion_lock(), LockSpan::File),
            (layout.tiers_lock(), Layout::reader_tier()),
        ] {
            dying
                .take_lock(&path, LockMode::Exclusive, span, Waiting::Never)
                .expect("held before the death");
        }
        dying.abandon();
        other
            .take_lock(
                &layout.exclusion_lock(),
                LockMode::Exclusive,
                LockSpan::File,
                Waiting::Never,
            )
            .expect("exclusion released by the death");
        other
            .take_lock(
                &layout.tiers_lock(),
                LockMode::Exclusive,
                Layout::reader_tier(),
                Waiting::Never,
            )
            .unwrap_or_else(|error| {
                panic!("{}: tier released by the death: {error}", backend.name())
            });
    });
}

#[test]
fn an_abandoned_participant_acts_no_more() {
    on_every_implementation("coord-dead", &|implementation, backend| {
        let participant = participant(implementation, backend);
        let lock = participant.layout().exclusion_lock();
        let hold = participant
            .take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never)
            .expect("taken");
        participant.abandon();
        assert!(matches!(participant.alive(), Err(CoordError::Abandoned)));
        assert!(matches!(
            participant.take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never),
            Err(CoordError::Abandoned)
        ));
        assert!(
            participant.release_lock(hold).is_none(),
            "the death released it, and nothing is left to release"
        );
        participant.abandon();
    });
}

#[test]
fn abandoning_one_participant_leaves_another_in_its_process_holding() {
    on_every_implementation("coord-abandon-one", &|implementation, backend| {
        let vfs = backend.process();
        let root = backend.root();
        let dying = participant_on(implementation, Arc::clone(&vfs), &root);
        let living = participant_on(implementation, vfs, &root);
        let layout = dying.layout().clone();
        living
            .take_lock(
                &layout.gate_lock(),
                LockMode::Exclusive,
                LockSpan::File,
                Waiting::Never,
            )
            .expect("the living holds the gate");
        dying
            .take_lock(
                &layout.exclusion_lock(),
                LockMode::Exclusive,
                LockSpan::File,
                Waiting::Never,
            )
            .expect("the dying holds exclusion");
        dying.abandon();
        let third = participant(implementation, backend);
        assert!(
            matches!(
                third.take_lock(
                    &layout.gate_lock(),
                    LockMode::Shared,
                    LockSpan::File,
                    Waiting::Never
                ),
                Err(CoordError::WouldWait)
            ),
            "{}: the living participant still holds the gate",
            backend.name()
        );
        third
            .take_lock(
                &layout.exclusion_lock(),
                LockMode::Exclusive,
                LockSpan::File,
                Waiting::Never,
            )
            .expect("the dead participant's exclusion is free");
    });
}

#[test]
fn creation_exclusions_of_two_implementations_do_not_exclude_each_other() {
    on_every_backend("coord-two-creators", &|backend| {
        let file = participant(&FileFactory::new(), backend);
        let alternate = participant(&FileFactory::alternate(), backend);
        file.take_lock(
            &file.layout().creation_lock(),
            LockMode::Exclusive,
            LockSpan::File,
            Waiting::Never,
        )
        .expect("the file-based creation exclusion");
        alternate
            .take_lock(
                &alternate.layout().creation_lock(),
                LockMode::Exclusive,
                LockSpan::File,
                Waiting::Never,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "{}: the alternate's creation exclusion: {error}",
                    backend.name()
                )
            });
    });
}

#[test]
fn creation_exclusions_of_one_implementation_exclude_each_other() {
    on_every_implementation("coord-one-creator", &|implementation, backend| {
        let first = participant(implementation, backend);
        let second = participant(implementation, backend);
        let lock = first.layout().creation_lock();
        first
            .take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never)
            .expect("the first creator");
        assert!(matches!(
            second.take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never),
            Err(CoordError::WouldWait)
        ));
    });
}
