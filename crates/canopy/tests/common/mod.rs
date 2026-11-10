// Copyright (c) Michael Grier.

//! Fault-injecting store harness for the CANOPY-M4 recovery-stress tests
//! (CHAOS-1..CHAOS-3, `D55`/`D56`).
//!
//! This module is the shared substrate the CHAOS integration tests build on: it
//! opens a [`Store`] over the ACADIA-M1 in-memory mock backend
//! ([`MemFs`](acadia::MemFs)) wrapped in the deterministic fault decorator
//! ([`FaultyFs`](acadia::FaultyFs), D55), and lets a test *script* faults --
//! a missing or corrupt copy, a read error, a write error, a short (torn) write,
//! an `fsync` or `rename` failure -- at **chosen paths and phase boundaries**,
//! with no runtime randomness (every fault fires on a scripted call-count
//! schedule, per the repo quality rules).
//!
//! # Three pieces
//!
//! - [`Layout`] -- a pure calculator of the store's on-disk paths (marker,
//!   commit log, partition roots, node directories, committed documents, staging
//!   temps). It has no filesystem effect, so a fault rule can name the exact path
//!   an operation will touch *before* the store exists to be asked. It mirrors
//!   the store's **specified** layout (`D40`/`D53`/`D8`): the marker `.shire`,
//!   the log `.commit-log`, partitions `p{index}`, a node directory
//!   `p{index}/<shard>/<name>`, and the staging trees. Binding to the specified
//!   layout (not to incidental store behavior) is deliberate; if the store's
//!   published layout changes, these constants change with it.
//! - [`Script`] -- a builder that turns named phase boundaries and path targets
//!   into a [`FaultPolicy`](acadia::FaultPolicy). It knows the [`Layout`], so a
//!   caller writes `Script::new().fail_prepare_fsync(Fault::io_error())` instead
//!   of hand-computing the log path and the `prepare` call ordinal.
//! - [`Chaos`] -- owns the [`FaultyFs`] backend and opens / reopens the store
//!   over it. `open` installs the scripted policy from `init` onward; `reopen`
//!   runs the recovery replay over whatever on-disk state a faulted operation
//!   left behind.
//!
//! # Crash model (D74)
//!
//! A crash is modelled by letting a scripted fault abort a **real** store
//! operation part-way, then [reopening](Chaos::reopen) the live tree to run
//! recovery over the resulting on-disk state. The harness deliberately does not
//! reach for [`MemFs::simulate_crash`](acadia::MemFs::simulate_crash): the
//! store fsyncs each staged temp file but not the staging *directory*, so a
//! strict power-loss model would drop un-fsync'd directory entries and is a
//! separate durability question (see [DESIGN-NOTES.md](../../../../DESIGN-NOTES.md)
//! decision D74). CHAOS exercises recovery *logic* over faulted-but-live state.
//!
//! Each CHAOS test binary `mod common;`s this file and uses a subset of it, so
//! the crate-wide dead-code lint is silenced here rather than per item.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use acadia::{Fault, FaultOp, FaultPolicy, FaultyFs, MemFs, Vfs, When};
use canopy::{DocumentName, GenesisConfig, NodePath, Nonce, ShireId, Store, StoreError};

/// The store root every CHAOS test uses.
pub const ROOT: &str = "shire";
/// The reserved marker file name at the store root and mirrored in each partition
/// (`D40`). Mirrors the store's specified layout.
pub const MARKER_NAME: &str = ".shire";
/// The commit log file name at the store root (`D53`). Mirrors the store's
/// specified layout.
pub const LOG_NAME: &str = ".commit-log";
/// The partition directory name prefix (`p{index}`). Mirrors the store's
/// specified layout.
pub const PARTITION_PREFIX: &str = "p";
/// The namespace tree's directory name under the store root. Mirrors the store's
/// own constant so a fault rule can name a path before the store exists.
pub const NAMESPACE_TREE_DIR: &str = "namespace";
/// The reserved per-partition staging directory name (`D40`/`D53`). Mirrors the
/// store's specified layout.
pub const STAGING_DIR: &str = ".staging";
/// The infix between a document name and the nonce in a staging temp file name
/// (`<document>.tmp.<nonce>`). Mirrors the store's specified layout.
pub const TEMP_INFIX: &str = ".tmp.";

/// The Shire id every CHAOS genesis carries.
const SHIRE_ID: &str = "chaos-shire";

/// A pure calculator of the store's on-disk paths, with no filesystem effect.
///
/// Every accessor mirrors the store's specified layout so a [`Script`] can name
/// the exact path an operation touches before the store exists. Since `D222` the
/// layout is simply one directory per path component, so there is no depth
/// parameter to keep in step with the genesis.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

impl Layout {
    /// A layout at [`ROOT`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: PathBuf::from(ROOT),
        }
    }

    /// The store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `.shire` marker at the store root.
    #[must_use]
    pub fn marker(&self) -> PathBuf {
        self.root.join(MARKER_NAME)
    }

    /// The `.commit-log` file at the store root (the target of `begin`/`prepare`/
    /// `end` appends and their fsyncs).
    #[must_use]
    pub fn commit_log(&self) -> PathBuf {
        self.root.join(LOG_NAME)
    }

    /// Namespace replica `index`'s root directory (`<root>/namespace/p{index}`).
    #[must_use]
    pub fn namespace_root(&self, index: usize) -> PathBuf {
        self.root
            .join(NAMESPACE_TREE_DIR)
            .join(format!("{PARTITION_PREFIX}{index}"))
    }

    /// The directory holding namespace replica `index`'s entries for `node` -- the target
    /// the commit-phase `fsync` and the staging-phase `create_dir_all` operate on.
    #[must_use]
    pub fn node_dir(&self, index: usize, node: &NodePath) -> PathBuf {
        self.namespace_root(index).join(node.relative_path())
    }

    /// The committed pointer entry for `document` at `node` in namespace replica `index` --
    /// the target of a `Read` fault modelling a missing or damaged entry.
    #[must_use]
    pub fn committed(&self, index: usize, node: &NodePath, document: &DocumentName) -> PathBuf {
        self.node_dir(index, node).join(document.as_str())
    }

    /// The staging temp for a pointer write under `nonce`
    /// (`namespace/p{index}/.staging/<nonce>/<document>.tmp.<nonce>`), whose exact path the
    /// caller knows because it supplies the nonce.
    ///
    /// The transactional and non-transactional writes stage the same way, keyed only by the
    /// nonce, so there is one shape here rather than two.
    #[must_use]
    pub fn write_temp(&self, index: usize, nonce: &Nonce, document: &DocumentName) -> PathBuf {
        self.namespace_root(index)
            .join(STAGING_DIR)
            .join(nonce.as_str())
            .join(temp_name(document, nonce))
    }
}

/// The staging temp file name for `document` under `nonce` (`<document>.tmp.<nonce>`).
fn temp_name(document: &DocumentName, nonce: &Nonce) -> String {
    format!("{}{TEMP_INFIX}{}", document.as_str(), nonce.as_str())
}

/// A builder that scripts faults into a [`FaultPolicy`], keyed off a [`Layout`].
///
/// Named phase-boundary methods hide the log's call-count schedule: an
/// [`apply_transaction`](canopy::Store::apply_transaction) appends exactly three
/// records to the single commit log -- `begin` (1st), `prepare` (2nd, whose fsync
/// is the D53 commit point), and `end` (3rd) -- so `fail_begin` /
/// `fail_prepare` / `fail_end` target [`FaultOp::Append`] on the log at the
/// matching ordinal, and `fail_prepare_fsync` targets the commit-point
/// [`FaultOp::Fsync`]. Path-targeted methods cover the copy- and write-level
/// faults. The generic [`Script::at`] escape hatch scripts anything else.
pub struct Script {
    layout: Layout,
    policy: FaultPolicy,
}

impl Default for Script {
    fn default() -> Self {
        Self::new()
    }
}

impl Script {
    /// A script on the default-shard-depth [`Layout`] that injects nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::with_layout(Layout::new())
    }

    /// A script on `layout` that injects nothing yet.
    #[must_use]
    pub fn with_layout(layout: Layout) -> Self {
        Self {
            layout,
            policy: FaultPolicy::new(),
        }
    }

    /// The layout this script computes paths against.
    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Consumes the script, yielding the built [`FaultPolicy`].
    #[must_use]
    pub fn into_policy(self) -> FaultPolicy {
        self.policy
    }

    /// The generic escape hatch: fire `fault` on `op` at `path`, per `when`.
    #[must_use]
    pub fn at(mut self, op: FaultOp, path: impl AsRef<Path>, fault: Fault, when: When) -> Self {
        self.policy = self.policy.inject(op, path, fault, when);
        self
    }

    // ---- Phase boundaries on the commit log -----------------------------------

    /// Fail the `begin` log append (the 1st append), aborting an apply before it
    /// stages anything.
    #[must_use]
    pub fn fail_begin(self, fault: Fault) -> Self {
        let log = self.layout.commit_log();
        self.at(FaultOp::Append, log, fault, When::OnCall(1))
    }

    /// Fail the `prepare` log append (the 2nd append), aborting an apply after
    /// staging but before the commit point is durable (rolls back).
    #[must_use]
    pub fn fail_prepare(self, fault: Fault) -> Self {
        let log = self.layout.commit_log();
        self.at(FaultOp::Append, log, fault, When::OnCall(2))
    }

    /// Fail the `prepare` record's fsync (the 2nd log fsync) -- the D53 commit
    /// point -- so the prepared transaction may or may not have been made durable.
    #[must_use]
    pub fn fail_prepare_fsync(self, fault: Fault) -> Self {
        let log = self.layout.commit_log();
        self.at(FaultOp::Fsync, log, fault, When::OnCall(2))
    }

    /// Fail the `end` log append (the 3rd append), aborting an apply after every
    /// swap has committed but before the transaction is marked ended (recovery
    /// re-runs the idempotent commit and ends it).
    #[must_use]
    pub fn fail_end(self, fault: Fault) -> Self {
        let log = self.layout.commit_log();
        self.at(FaultOp::Append, log, fault, When::OnCall(3))
    }

    /// Fail the `read` of the commit log itself, on the schedule `when` -- the
    /// read that drives the recovery replay at
    /// [`Store::open`](canopy::Store::open). Store *creation* never reads the log
    /// and a clean apply only appends, so the first matching log read is the
    /// reopen's recovery: `When::Once` faults exactly that recovery read without
    /// disturbing the original open or the intervening apply.
    #[must_use]
    pub fn fail_log_read(self, fault: Fault, when: When) -> Self {
        let log = self.layout.commit_log();
        self.at(FaultOp::Read, log, fault, when)
    }

    // ---- Commit-phase durability ----------------------------------------------

    /// Fail the durability `fsync` of namespace replica `index`'s node directory for
    /// `node`, on the schedule `when`. Failing the first such fsync aborts a commit after
    /// the `prepare` point while the pointer moves are in progress.
    #[must_use]
    pub fn fail_commit_fsync(
        self,
        index: usize,
        node: &NodePath,
        fault: Fault,
        when: When,
    ) -> Self {
        let dir = self.layout.node_dir(index, node);
        self.at(FaultOp::Fsync, dir, fault, when)
    }
    /// Fail the commit-phase `rename` that publishes namespace replica `index`'s
    /// transaction-staged pointer for `document` (under `txn`) over its committed entry,
    /// on the schedule `when`. The rename rule matches the temp (source) path, so the
    /// failure lands after the `prepare` commit point and a reopen must roll the
    /// transaction forward.
    #[must_use]
    pub fn fail_swap_rename(
        self,
        index: usize,
        txn: &Nonce,
        document: &DocumentName,
        fault: Fault,
        when: When,
    ) -> Self {
        let temp = self.layout.write_temp(index, txn, document);
        self.at(FaultOp::Rename, temp, fault, when)
    }
}

/// The fault-injecting store harness: owns the [`FaultyFs`] backend and opens or
/// reopens the store over it.
pub struct Chaos {
    faulty: Arc<FaultyFs<MemFs>>,
    layout: Layout,
}

impl Chaos {
    /// Opens a fresh store of `replica_count` replicas whose filesystem applies
    /// `script`, returning the harness and the opened [`Store`].
    ///
    /// The scripted policy is active from `init` onward, so a fault whose path
    /// `init` touches (a partition root, the root marker) fires during
    /// initialization -- target paths `init` does not touch (node directories, the
    /// commit log, committed documents, staging temps) to fault store *operations*
    /// rather than genesis. Panics if `init` itself fails, since a harness whose
    /// store never opened cannot run a scenario.
    #[must_use]
    pub fn open(replica_count: usize, script: Script) -> (Chaos, Store) {
        let layout = script.layout().clone();
        let faulty = Arc::new(FaultyFs::new(MemFs::new(), script.into_policy()));
        let vfs: Arc<dyn Vfs> = faulty.clone();
        vfs.create_dir_all(layout.root())
            .expect("create store root");
        let genesis = GenesisConfig::new(ShireId::new(SHIRE_ID).unwrap(), 0)
            .with_pointer_replica_count(replica_count)
            .unwrap()
            .with_blob_replica_count(replica_count)
            .unwrap();
        let store =
            Store::init(faulty.clone(), layout.root().to_path_buf(), genesis).expect("init store");
        (Chaos { faulty, layout }, store)
    }

    /// Opens a fresh store of `replica_count` replicas with no faults scripted, on
    /// the default shard depth.
    #[must_use]
    pub fn open_clean(replica_count: usize) -> (Chaos, Store) {
        Self::open(replica_count, Script::new())
    }

    /// The layout of the opened store.
    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The fault-injecting filesystem the store runs over, as a trait object for
    /// reopening.
    #[must_use]
    pub fn fs(&self) -> Arc<dyn Vfs> {
        self.faulty.clone()
    }

    /// The wrapped in-memory backend (e.g. to inspect raw bytes).
    #[must_use]
    pub fn mem(&self) -> &MemFs {
        self.faulty.inner()
    }

    /// Reopens the store at the harness root over the same faulted filesystem,
    /// running the recovery replay. Any `When::Once`/`When::Times` fault scripted
    /// for the first run has, if it fired, already been consumed, so a reopen
    /// exercises recovery over the state that fault left behind.
    pub fn reopen(&self, previous: impl Into<Option<Store>>) -> Result<Store, StoreError> {
        // Taking the old handle is the point, not ceremony: a root has one writer
        // (`D116`), so reopening while the previous handle lives is refused -- and was
        // never a restart. Threading it through makes the release impossible to forget.
        // `None` is for the case where the last attempt *failed*: a failed `open` returns
        // no handle, so its lock was already released.
        drop(previous.into());
        Store::open(self.fs(), self.layout.root().to_path_buf())
    }
}
