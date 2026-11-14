// Copyright (c) Michael Grier.

//! The admission gate and the attachment tiers (canopy's DESIGN-NOTES.md, "The admission gate and
//! the attachment tiers, modelled in `Coordination.tla`").

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use test_timers::mpsc::recv_timeout;
use test_timers::thread::sleep;

use super::{LONG, on_every_implementation, participant};
use crate::coord::file::FileCoordination;
use crate::coord::{
    Access, Admission, Attachment, CoordError, Coordination, HeldTiers, ReaderSafety, TierHold,
    Upgrade, Waiting,
};

/// Long enough for a participant that is not waiting to have finished.
const SETTLE: Duration = Duration::from_millis(50);

fn attach(participant: &FileCoordination, access: Access) -> Box<dyn Attachment> {
    participant.attach(access, Waiting::Never).expect("attach")
}

fn would_wait<T: std::fmt::Debug>(outcome: Result<T, CoordError>) -> bool {
    matches!(outcome, Err(CoordError::WouldWait))
}

/// Whether `observer` could take `span` of `lock` in `mode` without waiting; a take that succeeds
/// is released at once.
fn is_free(
    observer: &FileCoordination,
    lock: &Path,
    mode: crate::LockMode,
    span: crate::LockSpan,
) -> bool {
    match observer.take_lock(lock, mode, span, Waiting::Never) {
        Ok(hold) => {
            drop(observer.release_lock(hold));
            true
        }
        Err(CoordError::WouldWait) => false,
        Err(error) => panic!("{error}"),
    }
}

/// Waits until `condition` holds, failing the case if it has not within [`LONG`].
fn eventually(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + LONG;
    while !condition() {
        assert!(Instant::now() < deadline, "{what}");
        sleep(Duration::from_millis(1));
    }
}

/// Runs `upgrade` on a thread and reports through the receiver once it has returned.
fn upgrading(
    attachment: Box<dyn Attachment>,
    safety: ReaderSafety,
) -> mpsc::Receiver<Result<Box<dyn Upgrade>, CoordError>> {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(attachment.upgrade(safety, Waiting::Wait));
    });
    finished
}

#[test]
fn readers_and_writers_attach_together() {
    on_every_implementation("coord-attach-many", &|implementation, backend| {
        let attachments: Vec<_> = (0..6)
            .map(|i| {
                let access = if i % 2 == 0 {
                    Access::Read
                } else {
                    Access::Write
                };
                attach(&participant(implementation, backend), access)
            })
            .collect();
        for attachment in &attachments {
            let expected = HeldTiers {
                reader: Some(TierHold::Shared),
                writer: (attachment.access() == Access::Write).then_some(TierHold::Shared),
            };
            assert_eq!(attachment.tiers(), expected, "{}", backend.name());
        }
    });
}

#[test]
fn an_attachment_holds_the_gate_only_while_it_attaches() {
    on_every_implementation("coord-attach-gate", &|implementation, backend| {
        let attached = participant(implementation, backend);
        let _attachment = attach(&attached, Access::Read);
        let other = participant(implementation, backend);
        let gate = other.layout().gate_lock();
        other
            .take_lock(
                &gate,
                crate::LockMode::Exclusive,
                crate::LockSpan::File,
                Waiting::Never,
            )
            .unwrap_or_else(|error| panic!("{}: the gate is free: {error}", backend.name()));
    });
}

#[test]
fn only_a_reader_unsafe_upgrade_keeps_the_gate() {
    on_every_implementation("coord-upgrade-gate", &|implementation, backend| {
        for (safety, kept) in [(ReaderSafety::Safe, false), (ReaderSafety::Unsafe, true)] {
            let upgrade = attach(&participant(implementation, backend), Access::Write)
                .upgrade(safety, Waiting::Never)
                .expect("upgrade");
            let other = participant(implementation, backend);
            let gate = other.take_lock(
                &other.layout().gate_lock(),
                crate::LockMode::Shared,
                crate::LockSpan::File,
                Waiting::Never,
            );
            assert_eq!(would_wait(gate), kept, "{}: {safety:?}", backend.name());
            drop(upgrade);
        }
    });
}

#[test]
fn a_reader_safe_upgrade_keeps_readers_and_holds_off_writers() {
    on_every_implementation("coord-upgrade-safe", &|implementation, backend| {
        let _reader = attach(&participant(implementation, backend), Access::Read);
        let upgrader = attach(&participant(implementation, backend), Access::Write);
        let upgrade = upgrader
            .upgrade(ReaderSafety::Safe, Waiting::Never)
            .expect("no other writer is attached");
        assert_eq!(
            upgrade.tiers(),
            HeldTiers {
                reader: None,
                writer: Some(TierHold::Exclusive)
            }
        );
        let _late_reader = attach(&participant(implementation, backend), Access::Read);
        assert!(
            would_wait(participant(implementation, backend).attach(Access::Write, Waiting::Never)),
            "{}",
            backend.name()
        );
        drop(upgrade);
        attach(&participant(implementation, backend), Access::Write);
    });
}

#[test]
fn a_reader_unsafe_upgrade_waits_for_every_reader_and_admits_none() {
    on_every_implementation("coord-upgrade-unsafe", &|implementation, backend| {
        let reader = attach(&participant(implementation, backend), Access::Read);
        let upgrader = participant(implementation, backend);
        assert!(would_wait(
            attach(&upgrader, Access::Write).upgrade(ReaderSafety::Unsafe, Waiting::Never)
        ));
        drop(reader);
        let upgrade = attach(&upgrader, Access::Write)
            .upgrade(ReaderSafety::Unsafe, Waiting::Never)
            .expect("every reader has gone");
        assert_eq!(
            upgrade.tiers(),
            HeldTiers {
                reader: Some(TierHold::Exclusive),
                writer: Some(TierHold::Exclusive)
            }
        );
        for access in [Access::Read, Access::Write] {
            assert!(
                would_wait(participant(implementation, backend).attach(access, Waiting::Never)),
                "{}: {access:?}",
                backend.name()
            );
        }
    });
}

#[test]
fn an_upgrade_that_cannot_proceed_leaves_the_participant_holding_nothing() {
    on_every_implementation("coord-upgrade-detached", &|implementation, backend| {
        let other_writer = attach(&participant(implementation, backend), Access::Write);
        let upgrader = participant(implementation, backend);
        assert!(would_wait(
            attach(&upgrader, Access::Write).upgrade(ReaderSafety::Safe, Waiting::Never)
        ));
        drop(other_writer);
        let second = participant(implementation, backend);
        let upgrade = attach(&second, Access::Write).upgrade(ReaderSafety::Unsafe, Waiting::Never);
        assert!(
            upgrade.is_ok(),
            "{}: the refused upgrader holds nothing",
            backend.name()
        );
    });
}

#[test]
fn an_upgrade_waits_for_the_last_reader_to_leave() {
    on_every_implementation("coord-upgrade-wait", &|implementation, backend| {
        let reader = attach(&participant(implementation, backend), Access::Read);
        let finished = upgrading(
            attach(&participant(implementation, backend), Access::Write),
            ReaderSafety::Unsafe,
        );
        sleep(SETTLE);
        assert!(recv_timeout(&finished, Duration::ZERO).is_err(), "it waits");
        drop(reader);
        recv_timeout(&finished, LONG)
            .expect("let in")
            .unwrap_or_else(|error| panic!("{}: {error}", backend.name()));
    });
}

#[test]
fn two_upgraders_take_turns() {
    on_every_implementation("coord-upgrade-turns", &|implementation, backend| {
        let (done, finished) = mpsc::channel();
        let attachments: Vec<_> = (0..2)
            .map(|_| attach(&participant(implementation, backend), Access::Write))
            .collect();
        for attachment in attachments {
            let done = done.clone();
            thread::spawn(move || {
                let _ = done.send(attachment.upgrade(ReaderSafety::Safe, Waiting::Wait));
            });
        }
        let winner = recv_timeout(&finished, LONG)
            .expect("one upgrader is let in")
            .expect("upgraded");
        sleep(SETTLE);
        assert!(
            recv_timeout(&finished, Duration::ZERO).is_err(),
            "{}: the other waits its turn",
            backend.name()
        );
        drop(winner);
        recv_timeout(&finished, LONG)
            .expect("the other upgrader is let in")
            .expect("upgraded");
    });
}

#[test]
fn a_writer_arriving_while_an_upgrade_waits_for_readers_waits_at_the_gate() {
    on_every_implementation("coord-upgrade-at-gate", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let gate = observer.layout().gate_lock();
        let reader = attach(&participant(implementation, backend), Access::Read);
        let finished = upgrading(
            attach(&participant(implementation, backend), Access::Write),
            ReaderSafety::Unsafe,
        );
        eventually("the upgrader holds the gate", || {
            !is_free(
                &observer,
                &gate,
                crate::LockMode::Shared,
                crate::LockSpan::File,
            )
        });
        let writer = participant(implementation, backend);
        let (attached, attaching) = mpsc::channel();
        thread::spawn(move || {
            let _ = attached.send(writer.attach(Access::Write, Waiting::Wait).map(drop));
        });
        sleep(SETTLE);
        assert!(
            recv_timeout(&attaching, Duration::ZERO).is_err(),
            "it waits"
        );
        drop(reader);
        let upgrade = recv_timeout(&finished, LONG)
            .expect("the upgrade finishes")
            .expect("upgraded");
        sleep(SETTLE);
        assert!(
            recv_timeout(&attaching, Duration::ZERO).is_err(),
            "{}: the unsafe upgrade keeps the gate",
            backend.name()
        );
        drop(upgrade);
        recv_timeout(&attaching, LONG)
            .expect("the writer attaches once the upgrade is gone")
            .expect("attached");
    });
}

/// The case the upgrade's order exists for: a writer holding the reader tier while it waits for the
/// writer tier, and an unsafe upgrader past the gate. Taking the writer tier before the reader tier
/// would leave each waiting for the other once the safe upgrade holding the writer tier goes.
#[test]
fn a_writer_waiting_inside_the_gate_and_an_unsafe_upgrade_do_not_deadlock() {
    on_every_implementation("coord-upgrade-no-deadlock", &|implementation, backend| {
        let observer = participant(implementation, backend);
        let gate = observer.layout().gate_lock();
        let tiers = observer.layout().tiers_lock();
        let safe = attach(&participant(implementation, backend), Access::Write)
            .upgrade(ReaderSafety::Safe, Waiting::Never)
            .expect("upgrade");
        let writer = participant(implementation, backend);
        let (attached, attaching) = mpsc::channel();
        thread::spawn(move || {
            let _ = attached.send(writer.attach(Access::Write, Waiting::Wait));
        });
        eventually("the writer holds the reader tier", || {
            !is_free(
                &observer,
                &tiers,
                crate::LockMode::Exclusive,
                crate::coord::file::layout::Layout::reader_tier(),
            )
        });
        assert!(
            recv_timeout(&attaching, Duration::ZERO).is_err(),
            "it waits"
        );
        let finished = upgrading(
            attach(&participant(implementation, backend), Access::Read),
            ReaderSafety::Unsafe,
        );
        eventually("the upgrader holds the gate", || {
            !is_free(
                &observer,
                &gate,
                crate::LockMode::Shared,
                crate::LockSpan::File,
            )
        });
        drop(safe);
        let writer = recv_timeout(&attaching, LONG)
            .expect("the writer attaches")
            .unwrap_or_else(|error| panic!("{}: {error}", backend.name()));
        sleep(SETTLE);
        assert!(
            recv_timeout(&finished, Duration::ZERO).is_err(),
            "the upgrade waits for the attached writer"
        );
        drop(writer);
        recv_timeout(&finished, LONG)
            .expect("the upgrade is let in")
            .expect("upgraded");
    });
}

#[test]
fn a_dead_attachment_releases_its_tiers() {
    on_every_implementation("coord-attach-death", &|implementation, backend| {
        let dying = participant(implementation, backend);
        let attachment = attach(&dying, Access::Write);
        dying.abandon();
        attach(&participant(implementation, backend), Access::Write)
            .upgrade(ReaderSafety::Unsafe, Waiting::Never)
            .unwrap_or_else(|error| panic!("{}: released by the death: {error}", backend.name()));
        drop(attachment);
        assert!(matches!(
            dying.attach(Access::Read, Waiting::Never),
            Err(CoordError::Abandoned)
        ));
    });
}

#[test]
fn a_dead_upgrader_releases_the_gate_and_its_tiers() {
    on_every_implementation("coord-upgrade-death", &|implementation, backend| {
        let dying = participant(implementation, backend);
        let upgrade = attach(&dying, Access::Write)
            .upgrade(ReaderSafety::Unsafe, Waiting::Never)
            .expect("upgrade");
        dying.abandon();
        attach(&participant(implementation, backend), Access::Read);
        drop(upgrade);
    });
}

#[test]
fn a_released_upgrade_leaves_the_participant_detached_and_free_to_attach() {
    on_every_implementation("coord-upgrade-release", &|implementation, backend| {
        let upgrader = participant(implementation, backend);
        let upgrade = attach(&upgrader, Access::Write)
            .upgrade(ReaderSafety::Unsafe, Waiting::Never)
            .expect("upgrade");
        assert_eq!(upgrade.safety(), ReaderSafety::Unsafe);
        drop(upgrade);
        let again = attach(&upgrader, Access::Write);
        assert_eq!(again.access(), Access::Write);
        attach(&participant(implementation, backend), Access::Read);
    });
}

#[test]
fn admission_is_reached_through_the_interface() {
    on_every_implementation("coord-attach-dyn", &|implementation, backend| {
        use crate::coord::CoordinationFactory;
        let coordination: Box<dyn Coordination> = implementation
            .open(backend.process(), &backend.root())
            .expect("open");
        let attachment = coordination
            .attach(Access::Write, Waiting::Never)
            .expect("attach");
        attachment
            .upgrade(ReaderSafety::Safe, Waiting::Never)
            .expect("upgrade");
    });
}
