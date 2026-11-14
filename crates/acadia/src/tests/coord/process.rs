// Copyright (c) Michael Grier.

//! The cross-process cases: real processes over [`OsFs`], on Windows and on Linux.
//!
//! A helper process is this test binary run again on one ignored test, [`helper_process`], told by
//! its environment what to do. That keeps the helpers beside the cases and dependent on nothing but
//! acadia, which a helper crate naming canopy's types could not be.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use test_timers::mpsc::recv_timeout;

use super::super::TempDir;
use super::{LONG, implementations, participant_on};
use crate::coord::file::FileFactory;
use crate::coord::file::count::raising::Modifications;
use crate::coord::file::restart::Standing;
use crate::coord::{
    Access, Admission, CoordError, CoordinationFactory, CountNotification, ModCount,
    ModificationCount, ReaderSafety, Registration, Registry, RestartRecognition, Trust, Waiting,
    WriterExclusion,
};
use crate::{LockMode, LockSpan, OsFs};

/// The variable naming what a helper does; its absence means the test is not running as one.
const ACTION: &str = "ACADIA_COORD_HELPER_ACTION";
/// The variable naming the root a helper acts on.
const ROOT: &str = "ACADIA_COORD_HELPER_ROOT";
/// The variable naming the implementation a helper is a participant of.
const IMPLEMENTATION: &str = "ACADIA_COORD_HELPER_IMPLEMENTATION";
/// What a helper writes once it has done what it was told; libtest's own report of the test may
/// precede it on the line.
const READY: &str = "acadia-coord-helper: ready";

/// What a helper does once started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Holds writer exclusion until it is killed, refusing to wait for it.
    HoldExclusion,
    /// Waits for writer exclusion, then holds it until it is killed.
    AwaitExclusion,
    /// Acquires writer exclusion through the interface, recording itself as its holder, and holds
    /// it until it is killed.
    AcquireHold,
    /// Holds its implementation's creation exclusion until it is killed.
    HoldCreation,
    /// Records the state as vouched for in this boot.
    RecordVouched,
    /// Begins a modification, raising the count, and holds it open until it is killed.
    OpenModification,
    /// Attaches as a reader and stays attached until it is killed.
    AttachReader,
    /// Attaches as a writer, upgrades unsafely for readers, and holds the upgrade until it is
    /// killed.
    UpgradeUnsafe,
    /// Attaches as a reader, registers with the payload [`HELPER_PAYLOAD`], and stays attached
    /// until it is killed.
    RegisterReader,
}

/// What a registered helper's entry carries.
const HELPER_PAYLOAD: &[u8] = b"acadia-coord-helper";

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::HoldExclusion => "hold-exclusion",
            Action::AwaitExclusion => "await-exclusion",
            Action::AcquireHold => "acquire-hold",
            Action::HoldCreation => "hold-creation",
            Action::RecordVouched => "record-vouched",
            Action::OpenModification => "open-modification",
            Action::AttachReader => "attach-reader",
            Action::UpgradeUnsafe => "upgrade-unsafe",
            Action::RegisterReader => "register-reader",
        }
    }

    fn parse(name: &str) -> Action {
        [
            Action::HoldExclusion,
            Action::AwaitExclusion,
            Action::AcquireHold,
            Action::HoldCreation,
            Action::RecordVouched,
            Action::OpenModification,
            Action::AttachReader,
            Action::UpgradeUnsafe,
            Action::RegisterReader,
        ]
        .into_iter()
        .find(|action| action.name() == name)
        .unwrap_or_else(|| panic!("no helper action {name}"))
    }
}

/// A running helper, killed when dropped so that a failing case leaves no process behind.
struct Helper {
    child: Child,
    /// Sent to once the helper reports it has done what it was told.
    ready: mpsc::Receiver<()>,
}

impl Helper {
    /// Starts a helper of the implementation `implementation` doing `action` on `root`, and
    /// returns once it reports that it has.
    fn start(implementation: &str, action: Action, root: &Path) -> Helper {
        let helper = Helper::spawn(implementation, action, root);
        assert!(helper.ready_within(LONG), "the helper reports ready");
        helper
    }

    /// Whether the helper reports, within `within`, that it has done what it was told.
    fn ready_within(&self, within: std::time::Duration) -> bool {
        recv_timeout(&self.ready, within).is_ok()
    }

    /// Starts a helper of the implementation `implementation` doing `action` on `root`, without
    /// waiting for it to report.
    fn spawn(implementation: &str, action: Action, root: &Path) -> Helper {
        let test = format!(
            "{}::helper_process",
            module_path!().split_once("::").expect("a crate path").1
        );
        let mut child = Command::new(std::env::current_exe().expect("this test binary"))
            .args([test.as_str(), "--exact", "--ignored", "--nocapture"])
            .args(["--test-threads", "1"])
            .env(ACTION, action.name())
            .env(ROOT, root)
            .env(IMPLEMENTATION, implementation)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start a helper");
        let stdout = child.stdout.take().expect("the helper's output");
        let (ready, readiness) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.ends_with(READY) {
                    let _ = ready.send(());
                }
            }
        });
        Helper {
            child,
            ready: readiness,
        }
    }

    /// Kills the helper and waits for it to be gone.
    fn kill(mut self) {
        self.child.kill().expect("kill the helper");
        self.child.wait().expect("reap the helper");
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "a helper process the cross-process cases start; it does nothing run on its own"]
fn helper_process() {
    let Ok(action) = std::env::var(ACTION) else {
        return;
    };
    let root = PathBuf::from(std::env::var_os(ROOT).expect("a root"));
    let name = std::env::var(IMPLEMENTATION).expect("an implementation");
    let implementation = implementations()
        .into_iter()
        .find(|implementation| implementation.implementation().as_str() == name)
        .expect("a known implementation");
    let participant = participant_on(&implementation, Arc::new(OsFs::new()), &root);
    let mut exclusion = None;
    let mut hold = None;
    let mut attachment = None;
    let mut upgrade = None;
    let lock = match Action::parse(&action) {
        Action::HoldExclusion | Action::AwaitExclusion => {
            let waiting = if Action::parse(&action) == Action::HoldExclusion {
                Waiting::Never
            } else {
                Waiting::Wait
            };
            exclusion = Some(
                participant
                    .acquire_exclusion(waiting)
                    .expect("the helper holds exclusion"),
            );
            None
        }
        Action::AcquireHold => {
            hold = Some(
                participant
                    .acquire(Waiting::Never)
                    .expect("the helper holds exclusion"),
            );
            None
        }
        Action::HoldCreation => Some(participant.layout().creation_lock()),
        Action::RecordVouched => {
            participant
                .record_boot(Standing::Vouched)
                .expect("the helper vouches");
            None
        }
        Action::OpenModification => {
            participant
                .begin_modification(&mut Modifications::default())
                .expect("the helper raises the count");
            None
        }
        Action::AttachReader => {
            attachment = Some(
                participant
                    .attach(Access::Read, Waiting::Never)
                    .expect("the helper attaches"),
            );
            None
        }
        Action::RegisterReader => {
            let mut attached = participant
                .attach(Access::Read, Waiting::Never)
                .expect("the helper attaches");
            assert!(matches!(
                attached.register(HELPER_PAYLOAD),
                Ok(Registration::Registered(_))
            ));
            attachment = Some(attached);
            None
        }
        Action::UpgradeUnsafe => {
            upgrade = Some(
                participant
                    .attach(Access::Write, Waiting::Never)
                    .and_then(|attached| attached.upgrade(ReaderSafety::Unsafe, Waiting::Never))
                    .expect("the helper upgrades"),
            );
            None
        }
    };
    if let Some(lock) = lock {
        participant
            .take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never)
            .expect("the helper takes the lock");
    }
    println!("{READY}");
    let _held = (exclusion, hold, attachment, upgrade);
    loop {
        thread::park();
    }
}

#[test]
fn a_lock_held_in_another_process_is_released_by_its_death() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-death");
        let participant = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let lock = participant.layout().exclusion_lock();
        let helper = Helper::start(
            implementation.implementation().as_str(),
            Action::HoldExclusion,
            dir.path(),
        );
        assert!(matches!(
            participant.take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        helper.kill();
        participant
            .take_lock(&lock, LockMode::Exclusive, LockSpan::File, Waiting::Wait)
            .expect("let in once the holder is dead");
    }
}

#[test]
fn creation_exclusions_of_two_implementations_in_two_processes_do_not_exclude_each_other() {
    let dir = TempDir::new("coord-process-creators");
    let file = FileFactory::new();
    let alternate = FileFactory::alternate();
    let _helper = Helper::start(
        file.implementation().as_str(),
        Action::HoldCreation,
        dir.path(),
    );
    let mine = participant_on(&alternate, Arc::new(OsFs::new()), dir.path());
    mine.take_lock(
        &mine.layout().creation_lock(),
        LockMode::Exclusive,
        LockSpan::File,
        Waiting::Never,
    )
    .expect("the alternate's creation exclusion is free");
    let same = participant_on(&file, Arc::new(OsFs::new()), dir.path());
    assert!(matches!(
        same.take_lock(
            &same.layout().creation_lock(),
            LockMode::Exclusive,
            LockSpan::File,
            Waiting::Never
        ),
        Err(CoordError::WouldWait)
    ));
}

#[test]
fn state_vouched_for_in_another_process_is_trusted_in_this_one() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-trust");
        let mine = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let helper = Helper::start(
            implementation.implementation().as_str(),
            Action::RecordVouched,
            dir.path(),
        );
        assert_eq!(
            mine.trust().expect("trust"),
            Trust::Vouched,
            "both processes run in the same boot"
        );
        helper.kill();
        assert_eq!(mine.trust().expect("trust"), Trust::Vouched);
    }
}

#[test]
fn a_count_raised_by_a_process_that_is_killed_stays_odd() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-count");
        let mine = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        mine.record_boot(Standing::Vouched).expect("vouch");
        mine.publish_count(4, Standing::Vouched).expect("publish");
        let helper = Helper::start(
            implementation.implementation().as_str(),
            Action::OpenModification,
            dir.path(),
        );
        assert_eq!(mine.sample().expect("sample"), ModCount::new(5));
        helper.kill();
        assert_eq!(
            mine.sample().expect("sample"),
            ModCount::new(5),
            "a dead holder's count stays odd"
        );
    }
}

#[test]
fn exclusion_held_in_another_process_refuses_this_one_until_its_holder_dies() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-exclusion");
        let mine = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let helper = Helper::start(
            implementation.implementation().as_str(),
            Action::HoldExclusion,
            dir.path(),
        );
        assert!(matches!(
            mine.acquire_exclusion(Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        let waiter = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let waiting = thread::spawn(move || {
            drop(waiter.acquire_exclusion(Waiting::Wait).expect("let in"));
        });
        helper.kill();
        recv_ok(waiting);
    }
}

/// Joins `waiting`, failing the case if it has not finished within [`LONG`].
fn recv_ok(waiting: thread::JoinHandle<()>) {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(waiting.join());
    });
    recv_timeout(&finished, LONG)
        .expect("the waiter is let in")
        .expect("the waiter succeeds");
}

#[test]
fn a_process_waiting_for_exclusion_is_let_in_by_this_ones_release() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-handoff");
        let mine = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let held = mine.acquire_exclusion(Waiting::Never).expect("acquire");
        let helper = Helper::spawn(
            implementation.implementation().as_str(),
            Action::AwaitExclusion,
            dir.path(),
        );
        assert!(
            !helper.ready_within(std::time::Duration::from_millis(300)),
            "the other process waits while this one holds exclusion"
        );
        drop(held);
        assert!(helper.ready_within(LONG), "let in by the release");
        assert!(matches!(
            mine.acquire_exclusion(Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        helper.kill();
        mine.acquire_exclusion(Waiting::Wait)
            .expect("released by the death");
    }
}

#[test]
fn a_holder_killed_in_another_process_is_reported_dead_and_fenced() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-holder-death");
        let mine = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let mut recovery = mine.acquire(Waiting::Never).expect("acquire");
        recovery.complete_recovery().expect("recover");
        recovery.release().expect("release");
        let helper = Helper::start(
            implementation.implementation().as_str(),
            Action::AcquireHold,
            dir.path(),
        );
        helper.kill();
        let found = mine.acquire(Waiting::Wait).expect("acquire");
        assert!(found.acquired().holder_died());
        assert!(found.acquired().fenced());
        assert!(found.acquired().recovery_required());
    }
}

#[test]
fn a_reader_attached_in_another_process_holds_off_an_unsafe_upgrade_until_it_dies() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-reader");
        let mine = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let helper = Helper::start(
            implementation.implementation().as_str(),
            Action::AttachReader,
            dir.path(),
        );
        let refused = mine
            .attach(Access::Write, Waiting::Never)
            .expect("a writer attaches beside a reader")
            .upgrade(ReaderSafety::Unsafe, Waiting::Never);
        assert!(matches!(refused, Err(CoordError::WouldWait)));
        mine.attach(Access::Write, Waiting::Never)
            .expect("attach")
            .upgrade(ReaderSafety::Safe, Waiting::Never)
            .expect("a reader-safe upgrade does not wait for the reader");
        let attached = mine.attach(Access::Write, Waiting::Never).expect("attach");
        let waiting = thread::spawn(move || {
            drop(
                attached
                    .upgrade(ReaderSafety::Unsafe, Waiting::Wait)
                    .expect("let in"),
            );
        });
        helper.kill();
        recv_ok(waiting);
    }
}

#[test]
fn an_unsafe_upgrade_in_another_process_holds_off_attachment_until_it_dies() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-upgrade");
        let mine = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let helper = Helper::start(
            implementation.implementation().as_str(),
            Action::UpgradeUnsafe,
            dir.path(),
        );
        assert!(matches!(
            mine.attach(Access::Read, Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        let waiter = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let waiting = thread::spawn(move || {
            drop(waiter.attach(Access::Read, Waiting::Wait).expect("let in"));
        });
        helper.kill();
        recv_ok(waiting);
    }
}

#[test]
fn an_entry_registered_in_another_process_is_listed_until_it_dies_and_is_then_reused() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-registry");
        let mine = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        let helper = Helper::start(
            implementation.implementation().as_str(),
            Action::RegisterReader,
            dir.path(),
        );
        let listing = mine.list().expect("list");
        assert_eq!(listing.live.len(), 1);
        assert_eq!(listing.live[0].payload, HELPER_PAYLOAD);
        let dead = listing.live[0].token;
        helper.kill();
        let listing = mine.list().expect("list");
        assert!(
            listing.live.is_empty(),
            "a dead process's entry is not live"
        );
        assert!(!listing.may_be_incomplete);
        let mut attachment = mine.attach(Access::Read, Waiting::Never).expect("attach");
        let Registration::Registered(token) = attachment.register(b"mine").expect("register")
        else {
            panic!("every entry is held");
        };
        assert_ne!(token, dead);
        let bytes = mine
            .vfs()
            .read(&mine.layout().registry_entry(0))
            .expect("entry 0 is reused");
        assert_eq!(
            crate::coord::file::registry::decode(&bytes).map(|entry| entry.token),
            Some(token)
        );
    }
}

#[test]
fn a_count_raised_in_another_process_rings_a_registration_in_this_one() {
    for implementation in implementations() {
        let dir = TempDir::new("coord-process-notify");
        let mine = participant_on(&implementation, Arc::new(OsFs::new()), dir.path());
        mine.record_boot(Standing::Vouched).expect("vouch");
        mine.publish_count(6, Standing::Vouched).expect("publish");
        let (rang, rings) = mpsc::channel();
        let _notice = mine
            .notify(
                ModCount::new(6),
                std::time::Instant::now() + LONG * 4,
                Box::new(move || {
                    let _ = rang.send(());
                }),
            )
            .expect("notify");
        let _helper = Helper::start(
            implementation.implementation().as_str(),
            Action::OpenModification,
            dir.path(),
        );
        recv_timeout(&rings, LONG).expect("the other process's raise rings it");
        assert_eq!(mine.sample().expect("sample"), ModCount::new(7));
    }
}
