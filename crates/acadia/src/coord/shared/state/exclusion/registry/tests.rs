// Copyright (c) Michael Grier.

use super::*;
use crate::coord::{Access, Admission};

fn participants(fs: Arc<dyn Vfs>) -> (SharedState, SharedState) {
    fs.create_dir_all(Path::new("/registry")).unwrap();
    let first = SharedState::open(fs.clone(), Path::new("/registry")).unwrap();
    let second = SharedState::open(fs, Path::new("/registry")).unwrap();
    (first, second)
}

#[test]
fn opaque_payloads_and_tiers_round_trip_for_normal_registrations() {
    let (first, second) = participants(Arc::new(crate::MemFs::new()));
    for value in 0..12 {
        let mut attachment = first
            .attach(
                if value % 2 == 0 {
                    Access::Read
                } else {
                    Access::Write
                },
                Waiting::Never,
            )
            .unwrap();
        let payload = vec![value as u8; value];
        let token = attachment.register(&payload).unwrap();
        let listed = second.list().unwrap();
        assert_eq!(listed.live.len(), 1);
        assert_eq!(listed.live[0].payload, payload);
        assert_eq!(listed.live[0].tiers, attachment.tiers());
        assert_eq!(Registration::Registered(listed.live[0].token), token);
        assert!(!listed.may_be_incomplete);
        drop(attachment);
        assert!(second.list().unwrap().live.is_empty());
    }
}

#[test]
fn claim_contention_reports_unknown_and_preserves_an_updates_previous_payload() {
    let (first, second) = participants(Arc::new(crate::MemFs::new()));
    let mut attachment = first.attach(Access::Read, Waiting::Never).unwrap();
    attachment.register(b"before").unwrap();
    let claim = second.registry_lock("registry-claims.lock", 0).unwrap();
    assert!(matches!(
        attachment.register(b"after"),
        Err(CoordError::WouldWait)
    ));
    let listing = first.list().unwrap();
    assert_eq!(listing.unknown, 1);
    assert!(listing.may_be_incomplete);
    drop(claim);
    assert_eq!(second.list().unwrap().live[0].payload, b"before");
}

#[test]
fn failed_first_registration_marks_incomplete_and_failed_update_keeps_old_payload() {
    let gate = crate::FaultGate::closed();
    let policy = crate::FaultPolicy::new().inject_gated(
        crate::FaultOp::PublishNew,
        crate::PathMatcher::predicate(|path| {
            path.extension()
                .is_some_and(|extension| extension == "payload")
        }),
        crate::Fault::io_error(),
        crate::When::Always,
        gate.clone(),
    );
    let fs = Arc::new(crate::FaultyFs::new(crate::MemFs::new(), policy));
    let (first, second) = participants(fs);
    let mut attachment = first.attach(Access::Write, Waiting::Never).unwrap();
    gate.open();
    assert!(attachment.register(b"failed").is_err());
    assert!(second.list().unwrap().may_be_incomplete);
    assert!(second.list().unwrap().live.is_empty());
    gate.close();
    let token = attachment.register(b"good").unwrap();
    gate.open();
    assert!(attachment.register(b"bad update").is_err());
    gate.close();
    assert_eq!(second.list().unwrap().live[0].payload, b"good");
    assert_eq!(
        Registration::Registered(second.list().unwrap().live[0].token),
        token
    );
}

#[test]
fn capacity_overflow_is_best_effort_and_sticky_for_the_boot() {
    let fs = Arc::new(crate::MemFs::new());
    let (first, second) = participants(fs.clone());
    let mut held = Vec::new();
    for _ in 0..ENTRIES {
        let mut attachment = first.attach(Access::Read, Waiting::Never).unwrap();
        assert!(matches!(
            attachment.register(b"member").unwrap(),
            Registration::Registered(_)
        ));
        held.push(attachment);
    }
    let mut overflow = second.attach(Access::Read, Waiting::Never).unwrap();
    assert_eq!(
        overflow.register(b"overflow").unwrap(),
        Registration::Unregistered
    );
    assert_eq!(second.list().unwrap().live.len(), ENTRIES);
    assert!(second.list().unwrap().may_be_incomplete);
    drop(overflow);
    drop(held);
    drop(first);
    drop(second);
    let reopened = SharedState::open(fs, Path::new("/registry")).unwrap();
    assert!(reopened.list().unwrap().may_be_incomplete);
}

#[test]
fn an_abandoned_entry_is_not_live_and_an_old_drop_cannot_erase_its_reuser() {
    let (first, second) = participants(Arc::new(crate::MemFs::new()));
    let mut old = first.attach(Access::Read, Waiting::Never).unwrap();
    let token = old.register(b"old").unwrap();
    first.abandon();
    assert!(second.list().unwrap().live.is_empty());
    let mut new = second.attach(Access::Read, Waiting::Never).unwrap();
    assert_ne!(new.register(b"new").unwrap(), token);
    assert!(matches!(old.register(b"stale"), Err(CoordError::Abandoned)));
    drop(old);
    assert_eq!(second.list().unwrap().live[0].payload, b"new");
}

#[test]
fn a_corrupt_live_token_is_unknown_and_update_cannot_reuse_it() {
    let (first, second) = participants(Arc::new(crate::MemFs::new()));
    let mut attachment = first.attach(Access::Read, Waiting::Never).unwrap();
    attachment.register(b"before").unwrap();
    {
        let claim = second.registry_lock("registry-claims.lock", 0).unwrap();
        let _held = second.hold_guard(claim.id).unwrap();
        cell(&second, 0, format::TOKEN).store(format::NONE, Ordering::Release);
    }
    assert!(second.list().unwrap().may_be_incomplete);
    assert!(second.list().unwrap().live.is_empty());
    assert!(matches!(
        attachment.register(b"after"),
        Err(CoordError::Malformed { .. })
    ));
}

#[test]
fn tokens_do_not_repeat_after_a_last_participant_exit() {
    let fs = Arc::new(crate::MemFs::new());
    let (first, second) = participants(fs.clone());
    let mut attachment = first.attach(Access::Read, Waiting::Never).unwrap();
    let old = attachment.register(b"before").unwrap();
    drop(attachment);
    drop(first);
    drop(second);
    let next = SharedState::open(fs, Path::new("/registry")).unwrap();
    let mut attachment = next.attach(Access::Read, Waiting::Never).unwrap();
    assert_ne!(attachment.register(b"after").unwrap(), old);
}

#[test]
fn copied_files_in_an_independent_model_do_not_alias_live_registry_words() {
    let source = Arc::new(crate::MemFs::new());
    let (first, second) = participants(source.clone());
    let mut original = first.attach(Access::Read, Waiting::Never).unwrap();
    original.register(b"source").unwrap();
    let target = Arc::new(crate::MemFs::new());
    let directory = Path::new("/registry").join(DIRECTORY);
    target.create_dir_all(&directory).unwrap();
    for name in ["instance", "epoch", "boot", "cells"] {
        target
            .write(
                &directory.join(name),
                &source.read(&directory.join(name)).unwrap(),
            )
            .unwrap();
    }
    let copy = SharedState::open(target, Path::new("/registry")).unwrap();
    assert!(copy.list().unwrap().live.is_empty());
    assert_eq!(second.list().unwrap().live[0].payload, b"source");
    let mut copied = copy.attach(Access::Read, Waiting::Never).unwrap();
    copied.register(b"target").unwrap();
    assert_eq!(copy.list().unwrap().live[0].payload, b"target");
    assert_eq!(second.list().unwrap().live[0].payload, b"source");
}
