// Copyright (c) Michael Grier.

//! Unit tests for the seam, its backends and its fault decorator.
//!
//! Three concerns are exercised here:
//!
//! - A **shared conformance suite** ([`on_both`]) runs the same behavioral
//!   assertions against the real backend ([`OsFs`]) and the in-memory mock
//!   ([`MemFs`]), so both are proven to satisfy one contract for the operations
//!   the store relies on (write/read/append/truncate/rename/read_dir/remove/
//!   metadata/fsync). Only platform-independent *successful* semantics are
//!   asserted on both; exact error categories -- which the OS maps
//!   best-effort and can differ per platform -- are pinned against the mock,
//!   which is the taxonomy's reference implementation.
//! - **Durability** tests drive [`MemFs::simulate_crash`] to check the
//!   write-vs-fsync ordering the mock exists to model.
//! - **Fault** tests drive [`FaultyFs`] over the mock and assert every fault
//!   mode surfaces as the expected typed [`VfsError`] -- never a panic.

#[derive(Debug)]
struct PausedRelease {
    entered: std::sync::mpsc::Sender<()>,
    resume: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl crate::VfsLock for PausedRelease {}

impl Drop for PausedRelease {
    fn drop(&mut self) {
        self.entered.send(()).unwrap();
        let _ = self.resume.get_mut().unwrap().recv();
    }
}

pub(crate) fn assert_abandon_waits_for_release<Release: FnOnce() + Send>(
    install: impl FnOnce(Box<dyn crate::VfsLock>, std::sync::mpsc::Sender<()>) -> Release,
    abandon: impl Fn() + Sync,
) {
    use std::{sync::mpsc, time::Duration};
    let (entered, dropping) = mpsc::channel();
    let (resume, paused) = mpsc::channel();
    let (waiting, arrived) = mpsc::channel();
    let release = install(
        Box::new(PausedRelease {
            entered,
            resume: paused.into(),
        }),
        waiting,
    );
    std::thread::scope(|scope| {
        let resume = resume;
        let timeout = Duration::from_secs(5);
        scope.spawn(release);
        test_timers::mpsc::recv_timeout(&dropping, timeout).unwrap();
        let (done, finished) = mpsc::channel();
        let abandon = &abandon;
        scope.spawn(move || {
            abandon();
            done.send(()).unwrap();
        });
        test_timers::mpsc::recv_timeout(&arrived, timeout)
            .expect("second caller reached active release");
        assert!(finished.try_recv().is_err());
        resume.send(()).unwrap();
        test_timers::mpsc::recv_timeout(&finished, timeout).unwrap();
    });
}

pub(crate) fn assert_selected_notices_ring(participant: &dyn crate::coord::CountNotification) {
    use crate::coord::{ModCount, Notice};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    let deadline = Instant::now();
    let sampled = ModCount::new(0);
    let (entered, started) = mpsc::channel();
    let (resume, paused) = mpsc::channel();
    let _gate = participant
        .notify(
            sampled,
            deadline,
            Box::new(move || {
                entered.send(()).unwrap();
                let _ = paused.recv();
            }),
        )
        .unwrap();
    let timeout = Duration::from_secs(10);
    test_timers::mpsc::recv_timeout(&started, timeout).unwrap();
    let notices = Arc::new(Mutex::new(Vec::<Box<dyn Notice>>::new()));
    let (rang, rings) = mpsc::channel();
    for _ in 0..2 {
        let both = notices.clone();
        let rang = rang.clone();
        let notice = participant
            .notify(
                sampled,
                deadline,
                Box::new(move || {
                    both.lock().unwrap().clear();
                    rang.send(()).unwrap();
                }),
            )
            .unwrap();
        notices.lock().unwrap().push(notice);
    }
    resume.send(()).unwrap();
    for _ in 0..2 {
        test_timers::mpsc::recv_timeout(&rings, timeout).unwrap();
    }
}

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use crate::fault::{
    Corruption, Fault, FaultGate, FaultLog, FaultOp, FaultPolicy, FaultyFs, Observation,
    PathMatcher, When,
};
use crate::{FileType, MemFs, OsFs, Vfs, VfsErrorKind, VfsResult};

mod conformance;
mod coord;
mod fault;
mod locks;
mod mem_delta_sync;
mod mem_fork;
mod mem_model;
mod mem_reclaim;
mod publish;
mod signal;
mod waiting_locks;

/// A unique, self-cleaning scratch directory on the real filesystem for driving
/// [`OsFs`]. Created and removed through [`OsFs`] itself so the tests exercise
/// the backend rather than `std::fs` directly.
pub(crate) struct TempDir {
    path: PathBuf,
    fs: OsFs,
}

impl TempDir {
    pub(crate) fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("acadia-{}-{}-{}", std::process::id(), tag, n));
        let fs = OsFs::new();
        let _ = fs.remove_dir_all(&path);
        fs.create_dir_all(&path).expect("create scratch dir");
        TempDir { path, fs }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = self.fs.remove_dir_all(&self.path);
    }
}

/// Runs `body` against a fresh [`MemFs`] rooted at an existing directory.
fn on_mem(body: &dyn Fn(&dyn Vfs, &Path)) {
    let fs = MemFs::new();
    let root = Path::new("/data");
    fs.create_dir_all(root).expect("create mem root");
    body(&fs, root);
}

/// Runs `body` against a fresh [`OsFs`] rooted at a scratch directory.
fn on_os(tag: &str, body: &dyn Fn(&dyn Vfs, &Path)) {
    let tmp = TempDir::new(tag);
    let fs = OsFs::new();
    body(&fs, tmp.path());
}

/// Runs the same closure against both backends. `tag` names the scratch
/// directory for the [`OsFs`] pass.
pub(crate) fn on_both(tag: &str, body: impl Fn(&dyn Vfs, &Path)) {
    on_mem(&body);
    on_os(tag, &body);
}

/// Asserts `result` is an `Err` of exactly `kind`.
fn assert_kind<T: std::fmt::Debug>(result: VfsResult<T>, kind: VfsErrorKind) {
    match result {
        Ok(v) => panic!("expected error {kind:?}, got Ok({v:?})"),
        Err(e) => assert_eq!(
            e.kind(),
            kind,
            "op {:?} path {:?} message {:?}",
            e.op(),
            e.path(),
            e.message()
        ),
    }
}
