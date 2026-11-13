// Copyright (c) Michael Grier.

//! `lock`: the waiting form of `try_lock`, over both backends; a release, and a simulated
//! process's death, wake a waiter. A real process's death waking one is the cross-process
//! suite's.

use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::{LockMode, LockSpan, VfsLock};

const SHARED: LockMode = LockMode::Shared;
const EXCLUSIVE: LockMode = LockMode::Exclusive;

/// Long enough that a waiter which is going to be let in is let in well before it.
const GRANTED: Duration = Duration::from_secs(10);

/// Short enough to keep the suite fast, long enough that a waiter that was not blocked would
/// have reported in.
const STILL_WAITING: Duration = Duration::from_millis(100);

fn range(offset: u64, len: u64) -> LockSpan {
    LockSpan::Range { offset, len }
}

/// Starts a waiter for `mode` on `span` of `path` on its own thread, which reports on the
/// returned channel once it is granted.
fn waiter<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    fs: &'scope dyn Vfs,
    path: &'scope Path,
    mode: LockMode,
    span: LockSpan,
) -> mpsc::Receiver<Box<dyn VfsLock>> {
    let (granted, receive) = mpsc::channel();
    scope.spawn(move || {
        let held = fs.lock(path, mode, span).expect("the lock is granted");
        let _ = granted.send(held);
    });
    receive
}

fn assert_still_waiting(waiting: &mpsc::Receiver<Box<dyn VfsLock>>) {
    assert!(
        test_timers::mpsc::recv_timeout(waiting, STILL_WAITING).is_err(),
        "the waiter was granted a lock another holder has"
    );
}

fn granted(waiting: &mpsc::Receiver<Box<dyn VfsLock>>) -> Box<dyn VfsLock> {
    test_timers::mpsc::recv_timeout(waiting, GRANTED).expect("the waiter was let in")
}

#[test]
fn a_free_lock_is_granted_without_waiting() {
    on_both("wait-free", |fs, root| {
        let path = root.join("free.lock");
        let held = fs.lock(&path, EXCLUSIVE, LockSpan::File).unwrap();
        assert_kind(
            fs.try_lock(&path, SHARED, LockSpan::File),
            VfsErrorKind::Locked,
        );
        drop(held);
    });
}

#[test]
fn a_waiter_is_granted_when_the_holder_drops_its_guard() {
    on_both("wait-drop", |fs, root| {
        let path = root.join("x.lock");
        let held = fs.try_lock(&path, EXCLUSIVE, LockSpan::File).unwrap();
        std::thread::scope(|scope| {
            let waiting = waiter(scope, fs, &path, EXCLUSIVE, LockSpan::File);
            assert_still_waiting(&waiting);
            drop(held);
            let taken = granted(&waiting);
            assert_kind(
                fs.try_lock(&path, SHARED, LockSpan::File),
                VfsErrorKind::Locked,
            );
            drop(taken);
        });
    });
}

#[test]
fn every_shared_waiter_is_granted_by_one_release() {
    on_both("wait-shared", |fs, root| {
        let path = root.join("shared.lock");
        let held = fs.try_lock(&path, EXCLUSIVE, range(0, 8)).unwrap();
        std::thread::scope(|scope| {
            let first = waiter(scope, fs, &path, SHARED, range(0, 8));
            let second = waiter(scope, fs, &path, SHARED, range(4, 8));
            assert_still_waiting(&first);
            assert_still_waiting(&second);
            drop(held);
            drop((granted(&first), granted(&second)));
        });
    });
}

#[test]
fn a_waiter_for_bytes_nobody_holds_does_not_wait() {
    on_both("wait-disjoint", |fs, root| {
        let path = root.join("ranges");
        let held = fs.try_lock(&path, EXCLUSIVE, range(0, 8)).unwrap();
        let beside = fs.lock(&path, EXCLUSIVE, range(8, 8)).unwrap();
        drop((held, beside));
    });
}

#[test]
fn an_invalid_span_is_refused_before_any_wait() {
    on_both("wait-invalid", |fs, root| {
        let path = root.join("invalid.lock");
        let held = fs.try_lock(&path, EXCLUSIVE, LockSpan::File).unwrap();
        assert_kind(
            fs.lock(&path, EXCLUSIVE, range(0, 0)),
            VfsErrorKind::InvalidInput,
        );
        drop(held);
    });
}

#[test]
fn a_dead_holder_lets_a_waiter_in_another_process_in() {
    let survivor = MemFs::new();
    survivor.create_dir_all(Path::new("/d")).unwrap();
    let dying = survivor.another_process();
    let path = Path::new("/d/writer.lock");
    let held = dying.try_lock(path, EXCLUSIVE, LockSpan::File).unwrap();
    std::thread::scope(|scope| {
        let waiting = waiter(scope, &survivor, path, EXCLUSIVE, LockSpan::File);
        assert_still_waiting(&waiting);
        dying.abandon();
        let taken = granted(&waiting);
        drop(held);
        assert_kind(
            dying.try_lock(path, SHARED, LockSpan::File),
            VfsErrorKind::Locked,
        );
        drop(taken);
    });
}

#[test]
fn the_death_of_some_other_process_lets_no_waiter_in() {
    let holder = MemFs::new();
    holder.create_dir_all(Path::new("/d")).unwrap();
    let bystander = holder.another_process();
    let waiting_process = holder.another_process();
    let path = Path::new("/d/writer.lock");
    let held = holder.try_lock(path, EXCLUSIVE, LockSpan::File).unwrap();
    let unrelated = bystander
        .try_lock(
            &Path::new("/d").join("other.lock"),
            EXCLUSIVE,
            LockSpan::File,
        )
        .unwrap();
    std::thread::scope(|scope| {
        let waiting = waiter(scope, &waiting_process, path, EXCLUSIVE, LockSpan::File);
        bystander.abandon();
        assert_still_waiting(&waiting);
        drop(held);
        drop(granted(&waiting));
    });
    drop(unrelated);
}

#[test]
fn a_lock_can_be_made_to_fail_without_waiting() {
    let path = Path::new("/d/x.lock");
    let faulty = FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject(FaultOp::Lock, path, Fault::io_error(), When::Once),
    );
    faulty.create_dir_all(Path::new("/d")).unwrap();
    let held = faulty.try_lock(path, EXCLUSIVE, LockSpan::File).unwrap();
    assert_kind(
        faulty.lock(path, EXCLUSIVE, LockSpan::File),
        VfsErrorKind::Other,
    );
    drop(held);
    drop(faulty.lock(path, EXCLUSIVE, LockSpan::File).unwrap());
}
