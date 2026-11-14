// Copyright (c) Michael Grier.

//! The store's own write surface: verbs that mint their nonces from the source the store was
//! opened with, commit through the commit log, report what they wrote to an observer, and give
//! callbacks a view that can read but not write (the decision "The public write surface belongs
//! on `Store`" in [DESIGN-NOTES.md](../../DESIGN-NOTES.md)).
//!
//! # Nonce claims
//!
//! A nonce names a write's staging in both trees, a transaction's commit-log records, and
//! quarantined evidence. Every operation here claims its nonce before creating anything named
//! by it: the claim is a directory under [`CLAIMS_DIR`], created exclusively and made durable,
//! so two operations holding the same nonce cannot both proceed. Taking the claim also refuses a
//! nonce whose staging an earlier, interrupted operation left behind, because nothing would tell
//! the new operation's staging from the orphan's. A transaction id the commit log already holds
//! is refused when the transaction begins, and a quarantine name already taken is refused when
//! the evidence is kept, since kept evidence outlives its claim. Recovery reuses a transaction's
//! staging without claiming it: the transaction's `begin` record is what owns that nonce.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use acadia::coord::CoordinationFactory;
use acadia::coord::file::FileFactory;

use super::*;
use crate::feed::{CommitObserver, NonceSource};
use crate::read::{Read, WRITER_READ_ATTEMPTS};

mod stamp;
pub use stamp::WriteOutcome;

/// The directory under the store root holding one entry per claimed nonce.
///
/// Changing this value is a breaking change to the on-disk layout: a store opened by a build
/// using a different name would not see the claims of operations still in flight.
pub(crate) const CLAIMS_DIR: &str = ".claims";

/// The default time [`Store::snapshot`] waits for a stable modification count.
pub const DEFAULT_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(1);

/// The state behind the write surface that every clone of a store shares.
#[derive(Default)]
pub(crate) struct Surface {
    nonces: Mutex<Option<NonceSource>>,
    observer: Mutex<Option<Arc<dyn CommitObserver>>>,
    snapshot_timeout: Mutex<Option<Duration>>,
    agent: Mutex<Option<Agent>>,
    stamp_cache: Mutex<Option<(acadia::coord::Generation, Genesis)>>,
    description_pending: Mutex<Option<String>>,
}

/// How a store is opened.
///
/// Passed to [`Store::init`], [`Store::open`] and [`Store::migrate`]. Whether the store has a nonce
/// source is chosen explicitly, by the constructor used, so no store is left without one by
/// omission. Which coordination implementation owns the root is chosen by
/// [`with_coordination`](StoreOptions::with_coordination), and is acadia's file-based one unless
/// it is.
pub struct StoreOptions {
    agent: Agent,
    nonce_source: Option<NonceSource>,
    coordination: Arc<dyn CoordinationFactory>,
    snapshot_timeout: Duration,
}

impl StoreOptions {
    /// Explicit agent inputs and the nonce source, called once per operation that writes.
    ///
    /// Nonces need only be statistically unique: two operations whose traces are on disk at
    /// the same time must not share one. A collision is detected and fails the operation; it
    /// never corrupts the store.
    pub fn with_nonce_source(agent: Agent, source: impl FnMut() -> Nonce + Send + 'static) -> Self {
        StoreOptions {
            agent,
            nonce_source: Some(Box::new(source)),
            coordination: default_coordination(),
            snapshot_timeout: DEFAULT_SNAPSHOT_TIMEOUT,
        }
    }

    /// Explicit agent inputs with no nonce source, for a handle that only reads or holds the root. Every
    /// verb that writes is refused with [`StoreError::NoNonceSource`].
    pub fn without_nonce_source(agent: Agent) -> Self {
        StoreOptions {
            agent,
            nonce_source: None,
            coordination: default_coordination(),
            snapshot_timeout: DEFAULT_SNAPSHOT_TIMEOUT,
        }
    }

    /// Options that coordinate the root through `factory`'s implementation.
    ///
    /// A root records the implementation that owns it when it is created or migrated, and a store
    /// keeps the one it was created with. The store builds its coordination from its own
    /// [`Vfs`] and root through this factory, and accepts none built elsewhere.
    #[must_use]
    pub fn with_coordination(mut self, factory: Arc<dyn CoordinationFactory>) -> Self {
        self.coordination = factory;
        self
    }

    /// Bounds how long a snapshot waits for the modification count to change.
    #[must_use]
    pub fn with_snapshot_timeout(mut self, timeout: Duration) -> Self {
        self.snapshot_timeout = timeout;
        self
    }

    pub(crate) fn coordination(&self) -> Arc<dyn CoordinationFactory> {
        Arc::clone(&self.coordination)
    }

    /// The software and document schema this handle speaks.
    pub fn agent(&self) -> &Agent {
        &self.agent
    }
}

pub(crate) fn default_coordination() -> Arc<dyn CoordinationFactory> {
    Arc::new(FileFactory::new())
}

impl fmt::Debug for StoreOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreOptions")
            .field("agent", &self.agent)
            .field("nonce_source", &self.nonce_source.is_some())
            .field("coordination", self.coordination.implementation())
            .field("snapshot_timeout", &self.snapshot_timeout)
            .finish()
    }
}

/// What a logged write did, and the nonce that named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    outcome: TransactionOutcome,
    nonce: Nonce,
}

impl Commit {
    pub(crate) fn new(outcome: TransactionOutcome, nonce: Nonce) -> Self {
        Commit { outcome, nonce }
    }

    /// Whether the write committed or was rejected by a precondition.
    pub fn outcome(&self) -> &TransactionOutcome {
        &self.outcome
    }

    /// The nonce the write was made under, and its transaction id in the commit log.
    pub fn nonce(&self) -> &Nonce {
        &self.nonce
    }

    /// Whether the write committed.
    pub fn is_committed(&self) -> bool {
        self.outcome.is_committed()
    }

    /// Whether a precondition rejected the write.
    pub fn is_rejected(&self) -> bool {
        self.outcome.is_rejected()
    }
}

/// The store as a callback sees it: every read, and no write.
///
/// Lent to the closures of [`Store::plan_and_apply`] and [`Store::snapshot`] for the duration
/// of the call. A closure that has captured a clone of the store can still write through that
/// clone; the view only ensures that nothing handed to the closure invites it.
pub struct StoreView<'s> {
    store: &'s Store,
}

impl StoreView<'_> {
    /// Reads a document once; see [`Store::read_document`].
    pub fn read_document(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
    ) -> Result<Read, StoreError> {
        self.store.read_document(node, document)
    }

    /// Reads a document's body, resolving again if a write outran the first resolution.
    pub fn body(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
    ) -> Result<Option<DocumentBody>, StoreError> {
        let node = node.into();
        crate::read_within(self.store, &node, document, WRITER_READ_ATTEMPTS)
    }

    /// Lists a node's documents and child nodes; see [`Store::list_children`].
    pub fn list_children(&self, path: impl Into<NodePath>) -> Result<Children, StoreError> {
        self.store.list_children(path)
    }

    /// Lists every document in the store; see [`Store::list_documents`].
    pub fn list_documents(&self) -> Result<Vec<(NodePath, DocumentName)>, StoreError> {
        self.store.list_documents()
    }
}

/// A claimed nonce, released when dropped.
pub(crate) struct Claim<'s> {
    store: &'s Store,
    nonce: Nonce,
    kept: bool,
}

impl Claim<'_> {
    pub(crate) fn nonce(&self) -> &Nonce {
        &self.nonce
    }

    /// Leaves the claim on disk when dropped, for an operation whose traces recovery must
    /// still find under this nonce.
    pub(crate) fn keep(&mut self) {
        self.kept = true;
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if !self.kept {
            // Best-effort: a claim left behind refuses its nonce until the next resolve releases
            // it, which costs one collision and never a corruption.
            let _ = self
                .store
                .vfs
                .remove_dir(&self.store.claim_path(&self.nonce));
        }
    }
}

impl Store {
    pub(crate) fn install(&self, options: StoreOptions) {
        *self
            .surface
            .stamp_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = self
            .coordination
            .acquired()
            .map(|acquired| (acquired.advanced_to(), self.genesis.clone()));
        *self
            .surface
            .agent
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(options.agent);
        *self
            .surface
            .nonces
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = options.nonce_source;
        *self
            .surface
            .snapshot_timeout
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(options.snapshot_timeout);
    }

    fn snapshot_timeout(&self) -> Duration {
        self.surface
            .snapshot_timeout
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(DEFAULT_SNAPSHOT_TIMEOUT)
    }

    fn wait_for_snapshot_change(
        &self,
        sampled: acadia::coord::ModCount,
        deadline: Instant,
    ) -> Result<(), StoreError> {
        let (ring, wake) = mpsc::channel();
        let notice = self.coordination.notify(
            sampled,
            deadline,
            Box::new(move || {
                let _ = ring.send(());
            }),
        )?;
        #[expect(
            clippy::disallowed_methods,
            reason = "The synchronous snapshot's absolute deadline bounds a lost coordination notification (D120/D483)"
        )]
        let _ = wake.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        drop(notice);
        Ok(())
    }

    /// Calls the nonce source once.
    fn mint_nonce(&self) -> Result<Nonce, StoreError> {
        let mut nonces = self
            .surface
            .nonces
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let source = nonces.as_mut().ok_or(StoreError::NoNonceSource)?;
        Ok(source())
    }

    fn claim_path(&self, nonce: &Nonce) -> PathBuf {
        self.root
            .join(CLAIMS_DIR)
            .join(nonce.path_component().as_ref())
    }

    /// Mints a nonce and claims it.
    pub(crate) fn claim_fresh_nonce(&self) -> Result<Claim<'_>, StoreError> {
        let nonce = self.mint_nonce()?;
        self.claim_nonce(nonce)
    }

    /// Claims `nonce` for one operation, refusing it if another operation holds it or left
    /// staging under it.
    pub(crate) fn claim_nonce(&self, nonce: Nonce) -> Result<Claim<'_>, StoreError> {
        let claims = self.root.join(CLAIMS_DIR);
        self.create_dir_durably(&claims)?;
        match self.vfs.create_dir(&self.claim_path(&nonce)) {
            Ok(()) => {}
            Err(error) if error.kind() == VfsErrorKind::AlreadyExists => {
                return Err(StoreError::NonceCollision { nonce });
            }
            Err(error) => return Err(StoreError::Vfs(error)),
        }
        // Built before the sync so that every refusal below releases the claim.
        let claim = Claim {
            store: self,
            nonce,
            kept: false,
        };
        self.vfs.fsync(&claims)?;

        let blob_orphan = (0..self.genesis.blob_replica_count)
            .map(|index| self.blob_staging_temp(index, &claim.nonce));
        let namespace_orphan = (0..self.genesis.pointer_replica_count).map(|index| {
            namespace::namespace_partition_root(&self.root, index)
                .join(STAGING_DIR)
                .join(claim.nonce.path_component().as_ref())
        });
        for staged in blob_orphan.chain(namespace_orphan) {
            match self.vfs.metadata(&staged) {
                Ok(_) => {
                    return Err(StoreError::NonceCollision {
                        nonce: claim.nonce.clone(),
                    });
                }
                Err(error) if is_absent(error.kind()) => {}
                Err(error) => return Err(StoreError::Vfs(error)),
            }
        }
        Ok(claim)
    }

    /// Runs `operation` with a minter that draws each nonce from the source and claims it, and
    /// holds every claim until `operation` returns. Returns the nonces drawn, in order.
    pub(crate) fn with_claimed_nonces<Output>(
        &self,
        operation: impl FnOnce(
            &mut dyn FnMut() -> Result<Nonce, StoreError>,
        ) -> Result<Output, StoreError>,
    ) -> Result<(Output, Vec<Nonce>), StoreError> {
        let mut claims = Vec::new();
        let output = operation(&mut || {
            let claim = self.claim_fresh_nonce()?;
            let nonce = claim.nonce().clone();
            claims.push(claim);
            Ok(nonce)
        })?;
        let nonces = claims.iter().map(|claim| claim.nonce().clone()).collect();
        Ok((output, nonces))
    }

    /// Removes every claim. Run by resolving and by recovery under writer exclusion, when no
    /// operation can be in flight: a claim found there belongs to an operation that died, or to a
    /// transaction that failed unresolved and kept it.
    ///
    /// A transaction's nonce stays owned by its records in the commit log, and an interrupted
    /// write's orphaned staging still refuses its nonce when it is next claimed, so releasing
    /// the claims loses no protection.
    pub(crate) fn release_stale_claims(&self) -> Result<(), StoreError> {
        let claims = self.root.join(CLAIMS_DIR);
        let entries = match self.vfs.read_dir(&claims) {
            Ok(entries) => entries,
            Err(error) if error.kind() == VfsErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(StoreError::Vfs(error)),
        };
        for entry in entries {
            match self.vfs.remove_dir(&claims.join(&entry.file_name)) {
                Ok(()) => {}
                Err(error) if error.kind() == VfsErrorKind::NotFound => {}
                Err(error) => return Err(StoreError::Vfs(error)),
            }
        }
        Ok(())
    }

    /// Installs `observer`, replacing any observer already installed.
    ///
    /// The observer is told of every logged commit and every
    /// [`write_document_replicated`](Store::write_document_replicated), through any clone of this
    /// store; repairs are not reported, since they restore what was already committed.
    ///
    /// It runs on the writing thread, inside the write and under writer exclusion, so it holds
    /// up every writer while it runs. It must wait on nothing -- take no lock and make no call
    /// that can block on another thread, a process or a device, handing data off only without
    /// waiting -- must not call into this store through any handle, must return without
    /// panicking, and must do work bounded in time. Writes to one document are reported in the
    /// order they were applied; no order is promised between documents.
    pub fn observe(&self, observer: Arc<dyn CommitObserver>) {
        *self
            .surface
            .observer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(observer);
    }

    /// The installed observer, cloned out so the lock is not held while it runs.
    fn observer(&self) -> Option<Arc<dyn CommitObserver>> {
        self.surface
            .observer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(Arc::clone)
    }

    /// Reports a committed transaction's swaps to the observer.
    pub(crate) fn report_committed(&self, swaps: &SwapSet) {
        if let Some(observer) = self.observer() {
            for swap in swaps.swaps() {
                match swap.body() {
                    Some(body) => observer.committed(swap.node(), swap.document(), body),
                    None => observer.removed(swap.node(), swap.document()),
                }
            }
        }
    }

    /// Reports an unlogged write to the observer.
    pub(crate) fn report_written(
        &self,
        node: &NodePath,
        document: &DocumentName,
        body: &DocumentBody,
    ) {
        if let Some(observer) = self.observer() {
            observer.committed(node, document, body);
        }
    }

    /// Resets the commit log when the calling thread is the outermost holder of the writer
    /// gate, so the log is emptied before exclusion is released rather than left for the next
    /// open to replay.
    ///
    /// The result is discarded. A reset that fails, or is refused because the log holds a
    /// transaction recovery must finish, leaves the log for the next open, which is what the
    /// log is for.
    pub(crate) fn settle_log(&self) {
        if self.writer.held_depth() == 1 {
            let _ = self.quiesce_commit_log();
        }
    }

    /// Reads a document under exclusion, computes its new body with `transform`, and commits it
    /// as a swap conditional on what was read.
    ///
    /// `transform` sees `None` when the document is absent. Exclusion is held from the read to
    /// the commit, so no writer through another handle can land between them; a write made
    /// from inside `transform` itself, through a captured clone of the store, is not excluded,
    /// and the condition then rejects the commit rather than lose that write.
    pub fn transform(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
        transform: impl FnOnce(Option<&DocumentBody>) -> DocumentBody,
    ) -> Result<WriteOutcome<(DocumentBody, Commit)>, StoreError> {
        let _writer = self.writable()?;
        let node = node.into();
        let current = crate::read_within(self, &node, document, WRITER_READ_ATTEMPTS)?;
        let body = transform(current.as_ref());
        let swap = match &current {
            Some(current) => {
                Swap::conditional(node, document.clone(), body.clone(), current.digest())
            }
            None => Swap::conditional_absent(node, document.clone(), body.clone()),
        };
        let commit = self.update_document(swap)?;
        self.settle_log();
        Ok(commit.map(|commit| (body, commit)))
    }

    /// Plans a transaction from what the store holds and commits it, with exclusion held from
    /// the plan's first read to the commit.
    ///
    /// `plan` returns a value for the caller and, when there is something to write, the swaps
    /// to commit. It sees the store through a [`StoreView`]. The swaps may still carry
    /// preconditions, and a rejection is reported in the [`Commit`]. The second element is
    /// `None` when `plan` returned no swaps.
    pub fn plan_and_apply<Output>(
        &self,
        plan: impl FnOnce(&StoreView<'_>) -> (Output, Option<SwapSet>),
    ) -> Result<WriteOutcome<(Output, Option<Commit>)>, StoreError> {
        let _writer = self.writable()?;
        let (value, swaps) = plan(&StoreView { store: self });
        let outcome = match swaps {
            Some(swaps) => self
                .apply_transaction(&swaps)?
                .map(|commit| (value, Some(commit))),
            None => self.stamped((value, None), false),
        };
        self.settle_log();
        Ok(outcome)
    }

    /// Runs `read` against a state no write disturbed, without excluding writers.
    ///
    /// The [`modifications`](Store::modifications) count is sampled before and after each run.
    /// A run during which nothing was modified is returned; a disturbed run is discarded. When a
    /// write is in progress, the snapshot waits for the coordination notification and samples
    /// again, up to the configured timeout, after which it reports [`StoreError::SnapshotContended`].
    /// A count left odd by an unresolved transaction never settles, so a store that needs recovery
    /// is refused at once.
    ///
    /// `read` may therefore run more than once, and its result is used only from the run that
    /// was undisturbed.
    pub fn snapshot<Output>(
        &self,
        mut read: impl FnMut(&StoreView<'_>) -> Output,
    ) -> Result<Output, StoreError> {
        self.refuse_if_unresolved()?;
        let deadline = Instant::now() + self.snapshot_timeout();
        let view = StoreView { store: self };
        let mut attempts = 0;
        loop {
            let before = self.coordination.sample_count()?;
            if before.get().is_multiple_of(2) {
                attempts += 1;
                let value = read(&view);
                let after = self.coordination.sample_count()?;
                if Self::undisturbed(before.get(), after.get()) {
                    return Ok(value);
                }
                if after.get().is_multiple_of(2) {
                    if Instant::now() >= deadline {
                        break;
                    }
                    continue;
                }
                self.wait_for_snapshot_change(after, deadline)?;
            } else {
                self.wait_for_snapshot_change(before, deadline)?;
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        self.refuse_if_unresolved()?;
        Err(StoreError::SnapshotContended { attempts })
    }

    /// Empties the commit log, refusing while it holds a transaction recovery must finish.
    pub fn quiesce(&self) -> Result<WriteOutcome<()>, StoreError> {
        let _writer = self.writable()?;
        let changed = !self.commit_log_is_empty()? || _writer.publication_settled;
        self.quiesce_commit_log()?;
        Ok(self.stamped((), changed))
    }
}
