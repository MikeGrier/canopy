// Copyright (c) Michael Grier.

use super::*;
use crate::MemFs;
use crate::coord::{
    Access, Admission, ModificationEnd, Purpose, ReaderSafety, RecoveryDiscovery, Registry,
    Waiting, WriterExclusion,
};

fn filesystem() -> Arc<MemFs> {
    let fs = Arc::new(MemFs::new());
    fs.create_dir_all(Path::new("/state")).unwrap();
    fs
}

fn vouch(state: &SharedState) {
    state
        .acquire(Waiting::Wait)
        .unwrap()
        .complete_recovery()
        .unwrap();
}

#[test]
fn live_participants_share_cells_and_last_exit_requires_recovery() {
    for value in 1..=12 {
        let fs = filesystem();
        let first = SharedState::open(fs.clone(), Path::new("/state")).unwrap();
        assert!(first.snapshot().unwrap().fenced);
        vouch(&first);
        first
            .words
            .word(Word::Generation)
            .store(value, Ordering::Release);
        let second = SharedState::open(fs.clone(), Path::new("/state")).unwrap();
        assert_eq!(second.snapshot().unwrap().generation, value);
        assert_eq!(second.snapshot().unwrap().trust, Trust::Vouched);
        drop(first);
        assert_eq!(second.snapshot().unwrap().trust, Trust::Vouched);
        drop(second);
        let recreated = SharedState::open(fs, Path::new("/state")).unwrap();
        assert_eq!(
            recreated.snapshot().unwrap().trust,
            Trust::Untrusted(Untrusted::Created)
        );
        assert!(recreated.snapshot().unwrap().fenced);
    }
}

#[test]
fn every_spelling_of_one_directory_names_one_model() {
    for alias in [
        "/state",
        "state",
        "/./state",
        "/state/.",
        "/state/../state",
        "/other/../state",
        "/state/sub/..",
    ] {
        assert_eq!(
            model_root(Path::new(alias)),
            PathBuf::from("state"),
            "{alias}"
        );
    }
    assert_eq!(model_root(Path::new("/a/b/../c")), PathBuf::from("a/c"));
}

#[test]
fn participants_through_aliased_paths_share_one_state() {
    let fs = filesystem();
    let first = SharedState::open(fs.clone(), Path::new("/state")).unwrap();
    vouch(&first);
    first
        .words
        .word(Word::Generation)
        .store(7, Ordering::Release);
    let aliased = SharedState::open(fs, Path::new("/state/../state")).unwrap();
    assert_eq!(aliased.snapshot().unwrap().generation, 7);
    assert_eq!(aliased.snapshot().unwrap().trust, Trust::Vouched);
}

#[test]
fn host_restart_and_abandonment_are_not_clean_release() {
    let fs = filesystem();
    let first = SharedState::open(fs.clone(), Path::new("/state")).unwrap();
    vouch(&first);
    first.words.word(Word::Owner).store(42, Ordering::Release);
    let second = SharedState::open(fs.clone(), Path::new("/state")).unwrap();
    first.abandon();
    assert!(matches!(first.snapshot(), Err(CoordError::Abandoned)));
    assert_eq!(second.snapshot().unwrap().owner, 42);
    second.abandon();
    fs.simulate_crash();
    let restarted = SharedState::open(fs, Path::new("/state")).unwrap();
    assert_eq!(
        restarted.snapshot().unwrap().trust,
        Trust::Untrusted(Untrusted::HostRestarted)
    );
    assert!(restarted.snapshot().unwrap().fenced);
}

#[test]
fn failed_initialization_is_never_vouched_and_can_be_retried() {
    for operation in [
        crate::FaultOp::PublishNew,
        crate::FaultOp::Write,
        crate::FaultOp::Fsync,
    ] {
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/state")).unwrap();
        let gate = crate::FaultGate::closed();
        let policy = crate::FaultPolicy::new().inject_gated(
            operation,
            crate::PathMatcher::predicate(|_| true),
            crate::Fault::io_error(),
            crate::When::Once,
            gate.clone(),
        );
        let faulty = Arc::new(crate::FaultyFs::new(fs, policy));
        gate.open();
        assert!(SharedState::open(faulty.clone(), Path::new("/state")).is_err());
        gate.close();
        let retried = SharedState::open(faulty, Path::new("/state")).unwrap();
        assert!(retried.snapshot().unwrap().fenced);
        assert_ne!(retried.snapshot().unwrap().trust, Trust::Vouched);
    }
}

#[test]
fn identities_a_retry_issues_survive_a_crash_after_the_first_attempt_died_before_syncing() {
    let fs = MemFs::new();
    let root = Path::new("/state");
    fs.create_dir_all(root).unwrap();
    fs.fsync(root).unwrap();
    let policy = crate::FaultPolicy::new().inject(
        crate::FaultOp::Fsync,
        root,
        crate::Fault::io_error(),
        crate::When::Once,
    );
    let dying = Arc::new(crate::FaultyFs::new(fs.another_process(), policy));
    assert!(SharedState::open(dying, root).is_err());
    let retry = SharedState::open(Arc::new(fs.another_process()), root).unwrap();
    let range = root.join(DIRECTORY).join("generation-range");
    let issued = fs.read(&range).unwrap();
    retry.abandon();
    fs.simulate_crash();
    assert_eq!(
        fs.read(&range).ok(),
        Some(issued),
        "the reservation the retry issued from was lost"
    );
}

#[test]
fn a_participant_joining_as_the_last_one_leaves_opens_the_state_it_found() {
    let fs = filesystem();
    let root = Path::new("/state");
    let last = SharedState::open(fs.clone(), root).unwrap();
    vouch(&last);
    let leaving = Arc::new(Mutex::new(Some(last)));
    let departing = Arc::clone(&leaving);
    let policy = crate::FaultPolicy::new().inject(
        crate::FaultOp::Read,
        root.join(DIRECTORY).join("instance"),
        crate::Fault::observe_with(move |_, _| {
            drop(departing.lock().unwrap().take());
        }),
        crate::When::Once,
    );
    let joining = Arc::new(crate::FaultyFs::new(fs.another_process(), policy));
    let joined = SharedState::open(joining, root).expect("the joiner opens");
    assert!(leaving.lock().unwrap().is_none(), "the last one left");
    assert!(joined.snapshot().is_ok());
}

#[test]
fn a_first_registration_abandoned_part_way_records_no_incomplete_membership() {
    let fs = filesystem();
    let root = Path::new("/state");
    let witness = SharedState::open(fs.clone(), root).unwrap();
    let target: Arc<std::sync::OnceLock<Arc<SharedState>>> = Arc::default();
    let abandoning = Arc::clone(&target);
    let policy = crate::FaultPolicy::new().inject_matching(
        crate::FaultOp::TryLock,
        crate::PathMatcher::file_name_prefix("registry-claims"),
        crate::Fault::observe_with(move |_, _| {
            abandoning.get().expect("installed").abandon();
        }),
        crate::When::Once,
    );
    let vfs = Arc::new(crate::FaultyFs::new(fs.another_process(), policy));
    let participant = Arc::new(SharedState::open(vfs, root).unwrap());
    let _ = target.set(Arc::clone(&participant));
    let mut attachment = participant.attach(Access::Read, Waiting::Never).unwrap();
    assert!(matches!(
        attachment.register(b"payload"),
        Err(CoordError::Abandoned)
    ));
    assert!(
        !fs.exists(&root.join(DIRECTORY).join("registry-overflow"))
            .unwrap(),
        "an abandoned participant recorded incomplete membership"
    );
    assert!(!witness.list().unwrap().may_be_incomplete);
}

#[test]
fn an_epoch_write_cut_short_leaves_a_root_the_next_participant_opens() {
    let fs = filesystem();
    let root = Path::new("/state");
    drop(SharedState::open(fs.clone(), root).unwrap());
    let epoch = root.join(DIRECTORY).join("epoch");
    let policy = crate::FaultPolicy::new()
        .inject(
            crate::FaultOp::Write,
            &epoch,
            crate::Fault::ShortWrite { persist: 5 },
            crate::When::Once,
        )
        .inject_matching(
            crate::FaultOp::Write,
            crate::PathMatcher::file_name_prefix("epoch"),
            crate::Fault::ShortWrite { persist: 5 },
            crate::When::Once,
        );
    let torn = Arc::new(crate::FaultyFs::new(fs.another_process(), policy));
    assert!(SharedState::open(torn, root).is_err());
    let reopened = SharedState::open(fs.clone(), root).expect("the next participant opens");
    assert!(reopened.snapshot().unwrap().fenced);
}

#[test]
fn damaged_inactive_instance_and_mapping_are_recreated_fenced() {
    for name in ["instance", "cells"] {
        let fs = filesystem();
        drop(SharedState::open(fs.clone(), Path::new("/state")).unwrap());
        fs.write(&Path::new("/state").join(DIRECTORY).join(name), b"bad")
            .unwrap();
        let recreated = SharedState::open(fs, Path::new("/state")).unwrap();
        assert_eq!(
            recreated.snapshot().unwrap().trust,
            Trust::Untrusted(Untrusted::CannotTell)
        );
        assert!(recreated.snapshot().unwrap().fenced);
    }
}

#[test]
fn an_unknown_boot_record_recreates_only_as_untrusted() {
    let fs = filesystem();
    let first = SharedState::open(fs.clone(), Path::new("/state")).unwrap();
    vouch(&first);
    drop(first);
    fs.write(&Path::new("/state").join(DIRECTORY).join("boot"), b"bad")
        .unwrap();
    let unknown = SharedState::open(fs, Path::new("/state")).unwrap();
    assert_eq!(
        unknown.snapshot().unwrap().trust,
        Trust::Untrusted(Untrusted::CannotTell)
    );
    assert!(unknown.snapshot().unwrap().fenced);
}

#[test]
fn participant_abandonment_preserves_the_surviving_epoch() {
    let fs = filesystem();
    let first = SharedState::open(fs.clone(), Path::new("/state")).unwrap();
    let second = SharedState::open(fs.clone(), Path::new("/state")).unwrap();
    vouch(&first);
    first.abandon();
    let third = SharedState::open(fs, Path::new("/state")).unwrap();
    assert_eq!(third.snapshot().unwrap().trust, Trust::Vouched);
    assert_eq!(second.snapshot().unwrap().trust, Trust::Vouched);
}

#[test]
fn native_mappings_share_atomic_state_and_last_close_invalidates_trust() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.scratch")
        .join(format!("shared-state-{}", std::process::id()));
    let fs = Arc::new(crate::OsFs::new());
    fs.create_dir_all(&root).unwrap();
    let first = SharedState::open(fs.clone(), &root).unwrap();
    vouch(&first);
    first
        .words
        .word(Word::Generation)
        .store(123, Ordering::Release);
    let second = SharedState::open(fs.clone(), &root).unwrap();
    assert_eq!(second.snapshot().unwrap().trust, Trust::Vouched);
    assert_eq!(second.snapshot().unwrap().generation, 123);
    drop(first);
    assert_eq!(second.snapshot().unwrap().trust, Trust::Vouched);
    drop(second);
    let last = SharedState::open(fs, &root).unwrap();
    assert!(last.snapshot().unwrap().fenced);
    assert_eq!(
        last.snapshot().unwrap().trust,
        Trust::Untrusted(Untrusted::Created)
    );
    drop(last);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn child_state_session() {
    use std::io::{Read, Write};
    let Some(root) = std::env::var_os("CANOPY_STATE_CHILD_ROOT") else {
        return;
    };
    let fs = Arc::new(crate::OsFs::new());
    let state = SharedState::open(fs, Path::new(&root)).unwrap();
    assert_eq!(state.snapshot().unwrap().trust, Trust::Vouched);
    let _attachment = if matches!(
        std::env::var("CANOPY_STATE_CHILD_MODE").as_deref(),
        Ok("attachment")
    ) {
        let mut attachment = state.attach(Access::Read, Waiting::Wait).unwrap();
        attachment.register(b"native child reader").unwrap();
        Some(attachment)
    } else {
        None
    };
    let _hold = if matches!(
        std::env::var("CANOPY_STATE_CHILD_MODE").as_deref(),
        Ok("writer")
    ) {
        let mut hold = state.acquire(Waiting::Wait).unwrap();
        hold.begin_modification(Purpose::Ordinary).unwrap();
        Some(hold)
    } else {
        None
    };
    let mut output = std::io::stdout().lock();
    output.write_all(b"STATE-READY\n").unwrap();
    output.flush().unwrap();
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
}

struct ChildSession(std::process::Child);

impl Drop for ChildSession {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn last_process_death_requires_a_new_untrusted_epoch() {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.scratch")
        .join(format!("shared-process-{}", std::process::id()));
    let fs = Arc::new(crate::OsFs::new());
    fs.create_dir_all(&root).unwrap();
    let first = SharedState::open(fs.clone(), &root).unwrap();
    vouch(&first);
    let mut child = ChildSession(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "coord::shared::state::tests::child_state_session",
                "--nocapture",
            ])
            .env("CANOPY_STATE_CHILD_ROOT", &root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut output = std::io::BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "child exited before retaining state"
        );
        if line.contains("STATE-READY") {
            break;
        }
        line.clear();
    }
    drop(first);
    let surviving = SharedState::open(fs.clone(), &root).unwrap();
    assert_eq!(surviving.snapshot().unwrap().trust, Trust::Vouched);
    drop(surviving);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match fs.try_lock(
            &root.join(DIRECTORY).join("live.lock"),
            LockMode::Exclusive,
            LockSpan::File,
        ) {
            Ok(probe) => {
                drop(probe);
                break;
            }
            Err(error)
                if error.kind() == VfsErrorKind::Locked && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            result => panic!("last process did not release liveness: {result:?}"),
        }
    }
    let recreated = SharedState::open(fs, &root).unwrap();
    assert_eq!(
        recreated.snapshot().unwrap().trust,
        Trust::Untrusted(Untrusted::Created)
    );
    assert!(recreated.snapshot().unwrap().fenced);
    drop(recreated);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_dead_native_writer_leaves_ownership_and_odd_count_for_recovery() {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.scratch")
        .join(format!("shared-writer-process-{}", std::process::id()));
    let fs = Arc::new(crate::OsFs::new());
    fs.create_dir_all(&root).unwrap();
    let parent = SharedState::open(fs, &root).unwrap();
    vouch(&parent);
    let mut child = ChildSession(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "coord::shared::state::tests::child_state_session",
                "--nocapture",
            ])
            .env("CANOPY_STATE_CHILD_ROOT", &root)
            .env("CANOPY_STATE_CHILD_MODE", "writer")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut output = std::io::BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "writer exited before its modification"
        );
        if line.contains("STATE-READY") {
            break;
        }
        line.clear();
    }
    assert!(!parent.discover_recovery().unwrap().holder_died());
    assert!(
        !parent
            .snapshot()
            .unwrap()
            .modification_count
            .is_multiple_of(2)
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let mut recovery = parent.acquire(Waiting::Wait).unwrap();
    assert!(recovery.acquired().holder_died());
    assert!(recovery.acquired().fenced());
    assert!(matches!(
        recovery.begin_modification(Purpose::Ordinary),
        Err(CoordError::RecoveryRequired)
    ));
    recovery.begin_modification(Purpose::Recovery).unwrap();
    recovery.end_modification(ModificationEnd::Settled).unwrap();
    recovery.complete_recovery().unwrap();
    assert!(!parent.snapshot().unwrap().fenced);
    drop(recovery);
    drop(parent);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_native_count_publication_rings_a_notice_in_another_process() {
    use crate::coord::{CountNotification, ModificationCount};
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.scratch")
        .join(format!("shared-notice-process-{}", std::process::id()));
    let fs = Arc::new(crate::OsFs::new());
    fs.create_dir_all(&root).unwrap();
    let parent = SharedState::open(fs, &root).unwrap();
    vouch(&parent);
    let before = parent.sample().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let notice = parent
        .notify(
            before,
            std::time::Instant::now() + std::time::Duration::from_secs(3),
            Box::new(move || {
                let _ = sender.send(());
            }),
        )
        .unwrap();
    let mut child = ChildSession(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "coord::shared::state::tests::child_state_session",
                "--nocapture",
            ])
            .env("CANOPY_STATE_CHILD_ROOT", &root)
            .env("CANOPY_STATE_CHILD_MODE", "writer")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut output = std::io::BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "writer exited before count publication"
        );
        if line.contains("STATE-READY") {
            break;
        }
        line.clear();
    }
    receiver.recv().unwrap();
    assert_ne!(parent.sample().unwrap(), before);
    assert!(!parent.sample().unwrap().is_settled());
    drop(notice);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    parent
        .acquire(Waiting::Wait)
        .unwrap()
        .complete_recovery()
        .unwrap();
    drop(child);
    drop(parent);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_reader_death_releases_tiers_and_is_not_listed_as_live() {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.scratch")
        .join(format!("shared-reader-process-{}", std::process::id()));
    let fs = Arc::new(crate::OsFs::new());
    fs.create_dir_all(&root).unwrap();
    let parent = SharedState::open(fs, &root).unwrap();
    vouch(&parent);
    let mut child = ChildSession(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "coord::shared::state::tests::child_state_session",
                "--nocapture",
            ])
            .env("CANOPY_STATE_CHILD_ROOT", &root)
            .env("CANOPY_STATE_CHILD_MODE", "attachment")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut output = std::io::BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "reader exited before registering"
        );
        if line.contains("STATE-READY") {
            break;
        }
        line.clear();
    }
    assert_eq!(
        parent.list().unwrap().live[0].payload,
        b"native child reader"
    );
    let mut conversion = parent
        .attach(Access::Write, Waiting::Never)
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
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let upgrade = loop {
        if let Some(upgrade) = conversion.poll_tiers(ReaderSafety::Unsafe).unwrap() {
            break upgrade;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "dead reader still holds its tier"
        );
        std::thread::yield_now();
    };
    assert!(parent.list().unwrap().live.is_empty());
    drop(upgrade);
    drop(conversion);
    drop(parent);
    std::fs::remove_dir_all(root).unwrap();
}
