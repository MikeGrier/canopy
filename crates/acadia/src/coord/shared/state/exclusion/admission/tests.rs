// Copyright (c) Michael Grier.

use super::*;
use crate::coord::Registry;

fn participants() -> (Arc<crate::MemFs>, SharedState, SharedState) {
    let fs = Arc::new(crate::MemFs::new());
    fs.create_dir_all(Path::new("/tiers")).unwrap();
    let first = SharedState::open(fs.clone(), Path::new("/tiers")).unwrap();
    let second = SharedState::open(fs.clone(), Path::new("/tiers")).unwrap();
    (fs, first, second)
}

#[test]
fn a_panicking_gate_release_refuses_the_shared_attachment() {
    let (fs, first, _) = participants();
    fs.panic_when_next_lock_drops(Path::new("/tiers/.coord-shared/gate.lock"));

    assert!(matches!(
        first.attach(Access::Read, Waiting::Never),
        Err(CoordError::Abandoned)
    ));
}

#[test]
fn read_and_write_attachments_hold_only_their_declared_tiers() {
    let (_, first, second) = participants();
    let read = first.attach(Access::Read, Waiting::Never).unwrap();
    assert_eq!(read.tiers().reader, Some(TierHold::Shared));
    assert_eq!(read.tiers().writer, None);
    let writer = second.attach(Access::Write, Waiting::Never).unwrap();
    assert_eq!(writer.tiers().writer, Some(TierHold::Shared));
    drop(writer);
    let safe = second
        .attach(Access::Write, Waiting::Never)
        .unwrap()
        .upgrade(ReaderSafety::Safe, Waiting::Never)
        .unwrap();
    assert!(first.attach(Access::Read, Waiting::Never).is_ok());
    assert!(matches!(
        first.attach(Access::Write, Waiting::Never),
        Err(CoordError::WouldWait)
    ));
    drop(safe);
    drop(read);
}

#[test]
fn pending_conversion_retains_gate_and_cancel_restores_admission() {
    let (_, first, second) = participants();
    let writer = second.attach(Access::Write, Waiting::Never).unwrap();
    let mut conversion = first
        .attach(Access::Write, Waiting::Never)
        .unwrap()
        .begin_conversion()
        .unwrap();
    assert!(conversion.poll_gate().unwrap());
    assert!(conversion.poll_tiers(ReaderSafety::Safe).unwrap().is_none());
    assert!(matches!(
        first.attach(Access::Read, Waiting::Never),
        Err(CoordError::WouldWait)
    ));
    drop(conversion);
    assert!(first.attach(Access::Read, Waiting::Never).is_ok());
    drop(writer);
}

#[test]
fn opaque_registration_updates_under_one_token_and_unregistration_precedes_tiers() {
    let (_, first, second) = participants();
    let mut attachment = first.attach(Access::Write, Waiting::Never).unwrap();
    let token = attachment.register(b"first").unwrap();
    assert_eq!(attachment.register(b"second").unwrap(), token);
    let listed = second.list().unwrap();
    assert_eq!(listed.live.len(), 1);
    assert_eq!(listed.live[0].payload, b"second");
    let upgrade = attachment
        .upgrade(ReaderSafety::Unsafe, Waiting::Never)
        .unwrap();
    assert!(second.list().unwrap().live.is_empty());
    drop(upgrade);
    assert!(second.attach(Access::Write, Waiting::Never).is_ok());
}

#[test]
fn unsafe_upgrade_keeps_reader_admission_closed_until_drop() {
    let (_, first, second) = participants();
    let mut upgrade = first
        .attach(Access::Write, Waiting::Never)
        .unwrap()
        .upgrade(ReaderSafety::Unsafe, Waiting::Never)
        .unwrap();
    upgrade.admit_readers().unwrap();
    assert!(matches!(
        second.attach(Access::Read, Waiting::Never),
        Err(CoordError::WouldWait)
    ));
    assert!(matches!(
        second.attach(Access::Write, Waiting::Never),
        Err(CoordError::WouldWait)
    ));
    upgrade.register(b"unsafe upgrade").unwrap();
    let listed = second.list().unwrap();
    assert_eq!(listed.live[0].tiers.reader, Some(TierHold::Exclusive));
    drop(upgrade);
    assert!(second.list().unwrap().live.is_empty());
    assert!(second.attach(Access::Read, Waiting::Never).is_ok());
}

#[test]
fn partial_unsafe_conversion_cancels_and_a_safety_change_releases_its_holds() {
    let (_, first, second) = participants();
    let blocker = second
        .take_lock(
            &second.directory.join("writer-tier.lock"),
            LockMode::Shared,
            Waiting::Never,
        )
        .unwrap();
    let mut conversion = first
        .attach(Access::Read, Waiting::Never)
        .unwrap()
        .begin_conversion()
        .unwrap();
    assert!(conversion.poll_gate().unwrap());
    assert!(
        conversion
            .poll_tiers(ReaderSafety::Unsafe)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        second.attach(Access::Read, Waiting::Never),
        Err(CoordError::WouldWait)
    ));
    assert!(matches!(
        conversion.poll_tiers(ReaderSafety::Safe),
        Err(CoordError::Malformed { .. })
    ));
    assert!(second.attach(Access::Read, Waiting::Never).is_ok());
    drop(blocker);
}

#[test]
fn safe_incremental_upgrade_reopens_readers_only_when_explicitly_admitted() {
    let (_, first, second) = participants();
    let mut conversion = first
        .attach(Access::Write, Waiting::Never)
        .unwrap()
        .begin_conversion()
        .unwrap();
    assert!(conversion.poll_gate().unwrap());
    let mut upgrade = conversion.poll_tiers(ReaderSafety::Safe).unwrap().unwrap();
    assert!(matches!(
        second.attach(Access::Read, Waiting::Never),
        Err(CoordError::WouldWait)
    ));
    upgrade.admit_readers().unwrap();
    assert!(second.attach(Access::Read, Waiting::Never).is_ok());
    assert!(matches!(
        second.attach(Access::Write, Waiting::Never),
        Err(CoordError::WouldWait)
    ));
    drop(upgrade);
    assert!(second.attach(Access::Write, Waiting::Never).is_ok());
}
