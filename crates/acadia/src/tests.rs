// Copyright (c) Michael Grier.

//! Unit tests for ACADIA-M1 (VFS-5).
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
mod mem_reclaim;
mod publish;
mod signal;
mod waiting_locks;

/// A unique, self-cleaning scratch directory on the real filesystem for driving
/// [`OsFs`]. Created and removed through [`OsFs`] itself so the tests exercise
/// the backend rather than `std::fs` directly.
struct TempDir {
    path: PathBuf,
    fs: OsFs,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("acadia-{}-{}-{}", std::process::id(), tag, n));
        let fs = OsFs::new();
        let _ = fs.remove_dir_all(&path);
        fs.create_dir_all(&path).expect("create scratch dir");
        TempDir { path, fs }
    }

    fn path(&self) -> &Path {
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
fn on_both(tag: &str, body: impl Fn(&dyn Vfs, &Path)) {
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
