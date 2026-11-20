// Copyright (c) Michael Grier.

//! A `Vfs` through which a replay watches, fails and kills the calls a store makes, and the
//! grouping of those calls into the steps a model takes.
//!
//! Failures are injected by `FaultyFs`: every operation carries a rule behind one gate, and the
//! recorder opens the gate for exactly the calls its watcher condemns, so a failed call has no
//! effect, as a model's `Fail` has none.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use acadia::{
    DirEntry, FaultOp, FaultPolicy, FaultyFs, LockMode, LockSpan, MemFs, Metadata, Vfs, VfsError,
    VfsErrorKind, VfsLock, VfsResult,
};

mod tests;

/// What a watcher decides about a call before it is made.
pub(super) enum Verdict {
    /// The call goes ahead.
    Proceed,
    /// The call fails, and later calls go ahead.
    Fail,
    /// A lock attempt reports contention, and later calls go ahead.
    Locked,
    /// The call and every later one fail: the process is dead from here.
    Terminate,
}

/// Sees each call the store makes: before, to decide its fate, and after, once it succeeded.
pub(super) trait Watcher: Send + Sync {
    fn before(&self, op: FaultOp, path: &Path, to: Option<&Path>) -> Verdict;
    fn after(&self, op: FaultOp, path: &Path, to: Option<&Path>);
}

fn operation_name(op: FaultOp) -> &'static str {
    match op {
        FaultOp::Read => "read",
        FaultOp::ReadAt => "read_at",
        FaultOp::Write => "write",
        FaultOp::Append => "append",
        FaultOp::Truncate => "truncate",
        FaultOp::Fsync => "fsync",
        FaultOp::Rename => "rename",
        FaultOp::RemoveFile => "remove_file",
        FaultOp::CreateDir => "create_dir",
        FaultOp::CreateDirAll => "create_dir_all",
        FaultOp::RemoveDir => "remove_dir",
        FaultOp::RemoveDirAll => "remove_dir_all",
        FaultOp::ReadDir => "read_dir",
        FaultOp::Metadata => "metadata",
        FaultOp::LockExclusive => "lock_exclusive",
        FaultOp::TryLock => "try_lock",
        FaultOp::Lock => "lock",
        FaultOp::PublishNew => "publish_new",
        FaultOp::OpenOutput => "open_output",
        FaultOp::BootIdentity => "boot_identity",
    }
}

/// Whether `op` can change what a filesystem holds.
pub(super) fn mutates(op: FaultOp) -> bool {
    matches!(
        op,
        FaultOp::Write
            | FaultOp::Append
            | FaultOp::Truncate
            | FaultOp::Fsync
            | FaultOp::Rename
            | FaultOp::RemoveFile
            | FaultOp::CreateDir
            | FaultOp::CreateDirAll
            | FaultOp::RemoveDir
            | FaultOp::RemoveDirAll
            | FaultOp::PublishNew
    )
}

pub(super) struct Recorder {
    faulty: FaultyFs<MemFs>,
    dead: AtomicBool,
    watcher: Mutex<Option<Arc<dyn Watcher>>>,
}

impl fmt::Debug for Recorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Recorder").finish_non_exhaustive()
    }
}

impl Recorder {
    pub(super) fn new() -> Recorder {
        Recorder::over(MemFs::new())
    }

    /// A recorder over `mem`, which may be one simulated process of a filesystem others share.
    pub(super) fn over(mem: MemFs) -> Recorder {
        Recorder {
            faulty: FaultyFs::new(mem, FaultPolicy::new()),
            dead: AtomicBool::new(false),
            watcher: Mutex::new(None),
        }
    }

    /// The filesystem beneath, for a crash, a process's death, or a look at what it holds.
    pub(super) fn mem(&self) -> &MemFs {
        self.faulty.inner()
    }

    pub(super) fn watch(&self, watcher: Arc<dyn Watcher>) {
        *self.watcher.lock().unwrap_or_else(PoisonError::into_inner) = Some(watcher);
    }

    pub(super) fn unwatch(&self) {
        *self.watcher.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Fails every call until [`revive`](Recorder::revive): the process is dead.
    pub(super) fn kill(&self) {
        self.dead.store(true, Ordering::Release);
    }

    pub(super) fn revive(&self) {
        self.dead.store(false, Ordering::Release);
    }

    fn call<T>(
        &self,
        op: FaultOp,
        path: &Path,
        to: Option<&Path>,
        run: impl FnOnce(&FaultyFs<MemFs>) -> VfsResult<T>,
    ) -> VfsResult<T> {
        let watcher = self
            .watcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut fail = self.dead.load(Ordering::Acquire);
        if !fail && let Some(watcher) = &watcher {
            match watcher.before(op, path, to) {
                Verdict::Proceed => {}
                Verdict::Fail => fail = true,
                Verdict::Locked => {
                    return Err(VfsError::new(
                        VfsErrorKind::Locked,
                        operation_name(op),
                        "injected contention",
                    )
                    .with_path(path));
                }
                Verdict::Terminate => {
                    self.kill();
                    fail = true;
                }
            }
        }
        if fail {
            let op = operation_name(op);
            self.faulty.check_process(op, path)?;
            self.faulty.commit_process(op, path, &mut || {})?;
            return Err(VfsError::new(VfsErrorKind::Other, op, "injected fault").with_path(path));
        }
        let result = run(&self.faulty);
        if result.is_ok()
            && let Some(watcher) = watcher
        {
            watcher.after(op, path, to);
        }
        result
    }
}

impl Vfs for Recorder {
    fn check_process(&self, op: &'static str, path: &Path) -> VfsResult<()> {
        self.faulty.check_process(op, path)
    }

    fn commit_process(
        &self,
        op: &'static str,
        path: &Path,
        commit: &mut dyn FnMut(),
    ) -> VfsResult<()> {
        self.faulty.commit_process(op, path, commit)
    }

    fn read(&self, path: &Path) -> VfsResult<Vec<u8>> {
        self.call(FaultOp::Read, path, None, |fs| fs.read(path))
    }

    fn read_at(&self, path: &Path, offset: u64, len: usize) -> VfsResult<Vec<u8>> {
        self.call(FaultOp::ReadAt, path, None, |fs| {
            fs.read_at(path, offset, len)
        })
    }

    fn write(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        self.call(FaultOp::Write, path, None, |fs| fs.write(path, contents))
    }

    fn append(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        self.call(FaultOp::Append, path, None, |fs| fs.append(path, contents))
    }

    fn truncate(&self, path: &Path, len: u64) -> VfsResult<()> {
        self.call(FaultOp::Truncate, path, None, |fs| fs.truncate(path, len))
    }

    fn fsync(&self, path: &Path) -> VfsResult<()> {
        self.call(FaultOp::Fsync, path, None, |fs| fs.fsync(path))
    }

    fn rename(&self, from: &Path, to: &Path) -> VfsResult<()> {
        self.call(FaultOp::Rename, from, Some(to), |fs| fs.rename(from, to))
    }

    fn remove_file(&self, path: &Path) -> VfsResult<()> {
        self.call(FaultOp::RemoveFile, path, None, |fs| fs.remove_file(path))
    }

    fn create_dir(&self, path: &Path) -> VfsResult<()> {
        self.call(FaultOp::CreateDir, path, None, |fs| fs.create_dir(path))
    }

    fn create_dir_all(&self, path: &Path) -> VfsResult<()> {
        self.call(FaultOp::CreateDirAll, path, None, |fs| {
            fs.create_dir_all(path)
        })
    }

    fn remove_dir(&self, path: &Path) -> VfsResult<()> {
        self.call(FaultOp::RemoveDir, path, None, |fs| fs.remove_dir(path))
    }

    fn remove_dir_all(&self, path: &Path) -> VfsResult<()> {
        self.call(FaultOp::RemoveDirAll, path, None, |fs| {
            fs.remove_dir_all(path)
        })
    }

    fn read_dir(&self, path: &Path) -> VfsResult<Vec<DirEntry>> {
        self.call(FaultOp::ReadDir, path, None, |fs| fs.read_dir(path))
    }

    fn metadata(&self, path: &Path) -> VfsResult<Metadata> {
        self.call(FaultOp::Metadata, path, None, |fs| fs.metadata(path))
    }

    fn lock_exclusive(&self, path: &Path) -> VfsResult<Box<dyn VfsLock>> {
        self.call(FaultOp::LockExclusive, path, None, |fs| {
            fs.lock_exclusive(path)
        })
    }

    fn try_lock(&self, path: &Path, mode: LockMode, span: LockSpan) -> VfsResult<Box<dyn VfsLock>> {
        self.call(FaultOp::TryLock, path, None, |fs| {
            fs.try_lock(path, mode, span)
        })
    }

    fn lock(&self, path: &Path, mode: LockMode, span: LockSpan) -> VfsResult<Box<dyn VfsLock>> {
        self.call(FaultOp::Lock, path, None, |fs| fs.lock(path, mode, span))
    }

    fn publish_new(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        self.call(FaultOp::PublishNew, path, None, |fs| {
            fs.publish_new(path, contents)
        })
    }

    fn open_output(&self, path: &Path) -> VfsResult<std::fs::File> {
        self.call(FaultOp::OpenOutput, path, None, |fs| fs.open_output(path))
    }

    fn is_os_filesystem(&self) -> bool {
        self.faulty.is_os_filesystem()
    }

    fn boot_identity(&self) -> VfsResult<Vec<u8>> {
        self.call(FaultOp::BootIdentity, Path::new(""), None, |fs| {
            fs.boot_identity()
        })
    }
}

/// The calls a store makes, grouped into runs of one class, each run standing for at most one
/// step of a model.
///
/// A model step can be several calls -- a collection removes every superseded blob of a replica
/// in one step, one file at a time -- so what a step changed is compared once its run has ended,
/// which is when a call of another class begins. A run that changed nothing is not a step the
/// model needs to have taken.
pub(super) struct Runs<C, S> {
    /// What the filesystem holds now, as the last call left it.
    current: S,
    /// The run in progress: its class, and what the filesystem held when it began.
    run: Option<(C, S)>,
    /// Each run that changed what the filesystem holds, with its class and what it left.
    changes: Vec<(C, S)>,
}

impl<C: Copy + PartialEq, S: Clone + PartialEq> Runs<C, S> {
    pub(super) fn new(start: S) -> Self {
        Runs {
            current: start,
            run: None,
            changes: Vec::new(),
        }
    }

    /// A call of `class` is about to be made. Ends the run in progress if it is of another class,
    /// and returns the index of the change it made, if it made one.
    pub(super) fn begin(&mut self, class: C) -> Option<usize> {
        if let Some((running, _)) = &self.run
            && *running == class
        {
            return None;
        }
        let ended = self.end();
        self.run = Some((class, self.current.clone()));
        ended
    }

    /// A call succeeded and the filesystem now holds `state`.
    pub(super) fn observe(&mut self, state: S) {
        self.current = state;
    }

    /// Ends the run in progress, returning the index of the change it made, if it made one.
    pub(super) fn end(&mut self) -> Option<usize> {
        let (class, start) = self.run.take()?;
        if start == self.current {
            return None;
        }
        self.changes.push((class, self.current.clone()));
        Some(self.changes.len() - 1)
    }

    pub(super) fn changes(&self) -> &[(C, S)] {
        &self.changes
    }

    pub(super) fn current(&self) -> &S {
        &self.current
    }
}
