// Copyright (c) Michael Grier.

//! The single-writer monitor: one actor that serializes every document write
//! (D41/D33/D54, LAY-5).
//!
//! All store writes funnel through **one** [`WriteMonitor`] actor draining an mpsc
//! queue (D41/D38). A single global writer is single-writer-per-*everything*, so it
//! satisfies the per-node serialization requirement as a degenerate case with no
//! per-node registry, lifecycle, or lock ordering. The actor owns a [`Store`] clone
//! and applies each queued write to disk one at a time; a caller submits work and
//! blocks on a reply channel until the actor has applied it.
//!
//! **Reads bypass the monitor.** Only writes are serialized. Reads go straight to
//! disk through the [`store`](WriteMonitor::store) handle, lock-free: atomic rename
//! (D8) already isolates a reader from a landing write, so routing reads through the
//! actor would add a bottleneck for no correctness gain.
//!
//! **Transformations, not blind overwrites (no lost update).** The primary primitive
//! is [`transform`](WriteMonitor::transform): the caller supplies a function of the
//! current body, and the actor performs the **read-modify-write inside the single
//! writer**, so two racing updates apply in sequence with no lost update. A blind
//! [`write`](WriteMonitor::write) is the degenerate transformation that ignores the
//! current body, and [`conditional_write`](WriteMonitor::conditional_write) is the
//! compare-and-set for the read-compute-then-write case (D54). There is **no**
//! allocation special case -- incrementing a durable counter is just a `transform`
//! over a document, like any other write (D54).
//!
//! **Contract: per-node serialization only.** The *specified* guarantee is narrower
//! than the implementation: writes to a **given node** are serialized, with no
//! promised cross-node global order. Callers must bind to that per-node primitive,
//! never to the total order the single actor incidentally provides (D41), so the one
//! actor can later be sharded into per-node-hashed actors without a contract change.
//!
//! **Runtime-neutral (D33).** The actor is a plain OS thread over `std` channels: it
//! bakes in no async runtime. A submit-and-block handle is the runtime-neutral core;
//! an async edge (the tonic gRPC front-end) adapts to it with `spawn_blocking`, and a
//! future async submission seam is a non-breaking addition that leaves the actor loop
//! unchanged.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use crate::body::{ContentDigest, DocumentBody};
use crate::name::{DocumentName, NodePath, Nonce};
use crate::store::{BacklogRepairReport, Store, StoreError, TransactionOutcome};
use crate::transaction::SwapSet;

/// A caller-supplied source of write nonces (D61).
///
/// canopy mints no nonce of its own, so the monitor is handed a generator it calls
/// once per **committed** write to name that write's staging temps. The actor owns the
/// source and is the only caller, so it is invoked single-threaded and needs no
/// synchronization. A rejected [`conditional_write`](WriteMonitor::conditional_write)
/// performs no write and so consumes no nonce.
pub type NonceSource = Box<dyn FnMut() -> Nonce + Send>;

/// A boxed caller read-modify-write over a document body (LAY-5).
///
/// Sees `None` when the document is absent and returns the new body to replicate. It
/// is the erased form of [`WriteMonitor::transform`]'s generic function argument.
type TransformFn = Box<dyn FnOnce(Option<&DocumentBody>) -> DocumentBody + Send>;

/// The outcome of a [`conditional_write`](WriteMonitor::conditional_write) (D54 CAS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// The expected digest matched (or the write was unconditional): the new body was
    /// committed to all partitions.
    Committed,
    /// The expected digest did not match the current body, so nothing was written.
    /// Carries the actual current digest (`None` when the document was absent).
    Rejected {
        /// The digest of the document as it actually stood, or `None` if absent.
        current: Option<ContentDigest>,
    },
}

impl WriteOutcome {
    /// Whether the write was committed.
    pub fn is_committed(&self) -> bool {
        matches!(self, WriteOutcome::Committed)
    }

    /// Whether the write was rejected by the compare-and-set precondition.
    pub fn is_rejected(&self) -> bool {
        matches!(self, WriteOutcome::Rejected { .. })
    }
}

/// An observer of committed document writes: canopy's change feed over the D41 funnel.
///
/// The actor calls [`committed`](CommitObserver::committed) once per document that has
/// actually been written, for every write shape it serves -- transform, compare-and-set,
/// and each swap of a committed transaction. Hooking the actor rather than its callers is
/// the point: a caller-side hook has to be repeated at every write site and is one
/// forgotten site away from a silently incomplete feed, whereas the actor is the one place
/// a write cannot avoid. A rejected CAS and a rejected transaction wrote nothing, so
/// neither reports.
///
/// # What the feed promises
///
/// **Per-document order, and nothing more.** Writes to a given document are reported in
/// the order they were applied. No order is promised *between* documents, even though a
/// single actor incidentally provides one: D41 specifies per-ticket serialization only and
/// deliberately reserves the right to shard the actor, so an observer that binds to the
/// total order binds to an implementation accident it would lose.
///
/// The feed also carries no *prior* body. Supplying one would cost a read on every write
/// to serve an observer that may not exist, and an observer that needs a before-and-after
/// is better served by remembering what it last saw -- which is the only thing it can
/// honestly diff against anyway once it batches.
///
/// # What an observer owes
///
/// `committed` runs **on the single-writer thread**, inside the write path, so it holds up
/// every write in the process for as long as it runs. An implementation must therefore do
/// bounded work and must not block, perform I/O, or call back into the monitor. It must
/// also not panic: an unwinding observer would take the writer thread down and with it all
/// further writes, so a fallible observer swallows its own failures.
pub trait CommitObserver: Send + Sync {
    /// Reports that `body` is now the committed value of (`node`, `document`).
    fn committed(&self, node: &NodePath, document: &DocumentName, body: &DocumentBody);

    /// Reports that (`node`, `document`) has been removed (`BG-5`).
    ///
    /// A separate method rather than a `committed` with no body, because the two are
    /// different facts and an observer that cares about one usually does not care about
    /// the other in the same way -- a watcher rebuilding state from bodies has nothing
    /// to decode here, only an entry to drop.
    ///
    /// **Defaulted to ignore the removal**, so an existing observer keeps compiling; but
    /// ignoring it *is* a choice. An observer that maintains a view keyed on documents
    /// and does not implement this will keep reporting an entry that no longer exists.
    fn removed(&self, node: &NodePath, document: &DocumentName) {
        let _ = (node, document);
    }
}

/// The installed observer, shared between the handle (which sets it) and the actor
/// (which calls it).
///
/// A lock rather than an atomic swap: it is taken once per committed write, uncontended,
/// beside a replicated write and an fsync that cost several orders of magnitude more.
type CommitFeed = Arc<Mutex<Option<Arc<dyn CommitObserver>>>>;

/// The installed observer, if any.
///
/// Cloned out and the lock dropped before any call, so a panicking or re-entrant observer
/// can neither poison the feed nor deadlock against a concurrent install. The poison
/// tolerance is then belt-and-braces: nothing may wedge the single writer.
fn current_observer(feed: &CommitFeed) -> Option<Arc<dyn CommitObserver>> {
    feed.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(Arc::clone)
}

/// A planning closure the actor runs over the store; `None` means there is nothing to commit.
type PlanFn = Box<dyn FnOnce(&Store) -> Option<SwapSet> + Send>;

/// A unit of work the [`WriteMonitor`] actor applies, with the reply channel the
/// submitter blocks on.
enum Command {
    Snapshot(Box<dyn FnOnce(&Store) + Send>),
    /// Read-modify-write: apply `transform` to the current body and replicate it.
    Transform {
        node: NodePath,
        document: DocumentName,
        transform: TransformFn,
        reply: Sender<Result<DocumentBody, StoreError>>,
    },
    /// Compare-and-set: replicate `body` only if the current digest equals `expected`.
    ConditionalWrite {
        node: NodePath,
        document: DocumentName,
        body: DocumentBody,
        expected: ContentDigest,
        reply: Sender<Result<WriteOutcome, StoreError>>,
    },
    /// Multi-document transaction (D53): replicate the whole `swaps` set atomically
    /// (all-or-nothing, one fsync D69) or reject the whole set on a CAS miss.
    ApplyTransaction {
        swaps: SwapSet,
        reply: Sender<Result<TransactionOutcome, StoreError>>,
    },
    /// Plan-then-commit on the actor: run `plan` over the store, then replicate the swaps it
    /// returns as one transaction, with no write able to interleave between the two.
    PlanTransaction {
        plan: PlanFn,
        reply: Sender<Result<Option<TransactionOutcome>, StoreError>>,
    },
    /// Drain-and-stop: the actor finishes any queued work already dequeued, then exits.
    Shutdown,
    /// On-demand quiescence barrier: truncate the `D53` commit log now and confirm
    /// (TXN-3). Unlike [`Shutdown`](Command::Shutdown) the actor keeps running.
    Barrier {
        reply: Sender<Result<(), StoreError>>,
    },
    /// Repair the blobs reads have reported damaged, and report what was done (`D339`).
    RepairReportedDamage {
        reply: Sender<Result<BacklogRepairReport, StoreError>>,
    },
}

/// The single-writer monitor handle (D41, LAY-5).
///
/// Obtain one from [`WriteMonitor::spawn`]. Submit writes with
/// [`transform`](WriteMonitor::transform), [`write`](WriteMonitor::write), or
/// [`conditional_write`](WriteMonitor::conditional_write); each blocks until the actor
/// has applied it. Read through [`store`](WriteMonitor::store), which bypasses the
/// actor. Share the monitor across threads or tasks behind an [`Arc`](std::sync::Arc):
/// every method takes `&self`, and submissions serialize at the actor. Dropping the
/// monitor asks the actor to finish and joins its thread.
pub struct WriteMonitor {
    store: Store,
    tx: Sender<Command>,
    feed: CommitFeed,
    handle: Option<JoinHandle<()>>,
}

impl WriteMonitor {
    /// Reads a consistent snapshot between monitor writes. The closure must be
    /// bounded and must not call back into this monitor. It runs under the store's writer
    /// gate, so direct writes from this process wait for it; writes from another process
    /// are excluded by the root lock instead. A stopped monitor refuses snapshots.
    pub fn snapshot<Output: Send + 'static>(
        &self,
        read: impl FnOnce(&Store) -> Output + Send + 'static,
    ) -> Result<Output, MonitorError> {
        let (reply, receive) = mpsc::channel();
        self.tx
            .send(Command::Snapshot(Box::new(move |store| {
                let _ = reply.send(read(store));
            })))
            .map_err(|_| self.stopped())?;
        receive.recv().map_err(|_| self.stopped())
    }

    /// Plans a transaction against the store and commits it in one step on the actor, so no
    /// write can land between the read the plan was made from and the commit (`D41`).
    ///
    /// This is the read-compute-write that [`transform`](WriteMonitor::transform) cannot express
    /// because a transform always writes, and the one [`conditional_write`](WriteMonitor::conditional_write)
    /// and [`apply_transaction`](WriteMonitor::apply_transaction) can only approximate: a plan made
    /// from a read outside the actor is stale the moment another write commits, and a caller racing
    /// several others can lose that compare-and-set round after round.
    ///
    /// `plan` runs on the actor and returns a value for the caller and, when there is something to
    /// write, the [`SwapSet`] to commit. It must be bounded and must not call back into this
    /// monitor. It runs under the store's writer gate, so no direct write from this process can
    /// land between the plan and its commit. The swaps may still be guarded by digests, and the
    /// outcome then reports any rejection. The second element is `None` when `plan`
    /// returned no swaps.
    pub fn plan_and_apply<Output: Send + 'static>(
        &self,
        plan: impl FnOnce(&Store) -> (Output, Option<SwapSet>) + Send + 'static,
    ) -> Result<(Output, Option<TransactionOutcome>), MonitorError> {
        let (value_tx, value_rx) = mpsc::channel();
        let (reply, reply_rx) = mpsc::channel();
        self.tx
            .send(Command::PlanTransaction {
                plan: Box::new(move |store| {
                    let (value, swaps) = plan(store);
                    let _ = value_tx.send(value);
                    swaps
                }),
                reply,
            })
            .map_err(|_| self.stopped())?;
        let outcome = reply_rx
            .recv()
            .map_err(|_| self.stopped())?
            .map_err(MonitorError::Store)?;
        let value = value_rx.recv().map_err(|_| self.stopped())?;
        Ok((value, outcome))
    }

    /// Spawns the write-monitor actor over `store`, calling `nonce_source` to name
    /// each committed write's staging temps (D61).
    ///
    /// The actor owns a clone of `store`; the returned handle keeps its own clone for
    /// lock-free reads. Returns an error only if the OS refuses to create the actor
    /// thread.
    pub fn spawn(
        store: Store,
        nonce_source: impl FnMut() -> Nonce + Send + 'static,
    ) -> std::io::Result<WriteMonitor> {
        let (tx, rx) = mpsc::channel();
        let actor_store = store.clone();
        let feed: CommitFeed = Arc::new(Mutex::new(None));
        let actor_feed = Arc::clone(&feed);
        let mut nonce_source: NonceSource = Box::new(nonce_source);
        let handle = thread::Builder::new()
            .name("canopy-write-monitor".to_string())
            .spawn(move || run(actor_store, &mut nonce_source, rx, &actor_feed))?;
        Ok(WriteMonitor {
            store,
            tx,
            feed,
            handle: Some(handle),
        })
    }

    /// Installs `observer` as the monitor's change feed, replacing any previous one.
    ///
    /// Separate from [`spawn`](WriteMonitor::spawn) because an observer is generally built
    /// *over* the monitor and so cannot exist before it. Writes already applied are not
    /// replayed: the feed reports what happens from here on, and an observer that needs a
    /// starting point reads the store (reads bypass the actor, D41) to establish one.
    ///
    /// See [`CommitObserver`] for what an implementation owes the single writer.
    pub fn observe(&self, observer: Arc<dyn CommitObserver>) {
        *self.feed.lock().unwrap_or_else(PoisonError::into_inner) = Some(observer);
    }

    /// The store handle for lock-free reads and layout accessors (reads bypass the
    /// monitor, D41).
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Applies `transform` to the current body of (`node`, `document`) and replicates
    /// the result to all partitions, returning the new body (LAY-5).
    ///
    /// The actor performs the whole read-modify-write, so no other write interleaves
    /// between the read and the write: concurrent transforms apply in sequence with no
    /// lost update. The transform sees `None` when the document does not yet exist (so
    /// it can create it) and `Some(current)` otherwise. A transform always results in a
    /// replicated write, even if it returns an unchanged body.
    ///
    /// The read reflects the quorum majority. A namespace that will not settle is retried
    /// a fixed number of times and then surfaces as
    /// [`StoreError::DocumentOvertaken`], while one holding an unreadable replica surfaces as
    /// [`StoreError::PointerUnavailable`]. Both report that the value could not be determined
    /// rather than convicting the store (`D319`, `D338`).
    pub fn transform<F>(
        &self,
        node: impl Into<NodePath>,
        document: DocumentName,
        transform: F,
    ) -> Result<DocumentBody, MonitorError>
    where
        F: FnOnce(Option<&DocumentBody>) -> DocumentBody + Send + 'static,
    {
        let (reply, reply_rx) = mpsc::channel();
        self.tx
            .send(Command::Transform {
                node: node.into(),
                document,
                transform: Box::new(transform),
                reply,
            })
            .map_err(|_| self.stopped())?;
        reply_rx
            .recv()
            .map_err(|_| self.stopped())?
            .map_err(MonitorError::Store)
    }

    /// Blindly replicates `body` as the new value of (`node`, `document`),
    /// overwriting any current body (LAY-5).
    ///
    /// The degenerate transformation that ignores the current body. Use
    /// [`transform`](WriteMonitor::transform) for a read-modify-write and
    /// [`conditional_write`](WriteMonitor::conditional_write) for a compare-and-set.
    pub fn write(
        &self,
        node: impl Into<NodePath>,
        document: DocumentName,
        body: DocumentBody,
    ) -> Result<(), MonitorError> {
        self.transform(node, document, move |_| body).map(|_| ())
    }

    /// Replicates `body` as the new value of (`node`, `document`) only if the current
    /// body's digest equals `expected` (the D54 compare-and-set, LAY-5).
    ///
    /// For the read-compute-then-write case: the caller read a body earlier, computed
    /// `body` from it, and wants the write to land only if the stored value has not
    /// changed since. On a digest mismatch (including the document being absent) the
    /// write is skipped and [`WriteOutcome::Rejected`] carries the actual current
    /// digest. A [`transform`](WriteMonitor::transform) is the ordering-free
    /// alternative when the new body can be computed inside the monitor.
    pub fn conditional_write(
        &self,
        node: impl Into<NodePath>,
        document: DocumentName,
        body: DocumentBody,
        expected: ContentDigest,
    ) -> Result<WriteOutcome, MonitorError> {
        let (reply, reply_rx) = mpsc::channel();
        self.tx
            .send(Command::ConditionalWrite {
                node: node.into(),
                document,
                body,
                expected,
                reply,
            })
            .map_err(|_| self.stopped())?;
        reply_rx
            .recv()
            .map_err(|_| self.stopped())?
            .map_err(MonitorError::Store)
    }

    /// Applies `swaps` as one `D53` multi-document transaction through the single
    /// writer: the whole set commits atomically (all-or-nothing, one fsync D69) or the
    /// whole set is rejected on a CAS miss, so a partial batch can never be observed.
    ///
    /// This is the multi-document sibling of
    /// [`conditional_write`](WriteMonitor::conditional_write): where that compares and
    /// sets a single document, this compares and sets an arbitrary set of documents in
    /// one atomic step (each swap may be unconditional or CAS-guarded). Routing the
    /// transaction through the actor keeps it on the `D41` single-writer path, so it
    /// serializes against every other write with no lost update. Unlike a rejected
    /// `conditional_write`, a rejected transaction still consumes a nonce (the nonce is
    /// minted before the store's precondition phase); nonces need not be gapless, so
    /// this is harmless.
    pub fn apply_transaction(&self, swaps: SwapSet) -> Result<TransactionOutcome, MonitorError> {
        let (reply, reply_rx) = mpsc::channel();
        self.tx
            .send(Command::ApplyTransaction { swaps, reply })
            .map_err(|_| self.stopped())?;
        reply_rx
            .recv()
            .map_err(|_| self.stopped())?
            .map_err(MonitorError::Store)
    }

    /// Drives the store to quiescence on demand, truncating the `D53` commit log, and
    /// returns once that truncation has happened (TXN-3).
    ///
    /// A barrier: it submits a `Command::Barrier` and blocks until the actor -- the
    /// `D41` single writer -- has processed it. No transaction is open while the actor
    /// sits between commands, so the log is discardable at that instant; the actor
    /// truncates it and only then replies, so observing this call return means the log
    /// was empty at a quiescent moment. This is the deterministic shutdown counterpart
    /// to the automatic drain-triggered reset: a caller (the daemon's graceful shutdown)
    /// can force the O(1)-startup clean state even while other handles keep the actor
    /// alive, without relying on [`Drop`] ordering.
    ///
    /// A truncation that did not happen is never reported as one. While a transaction through
    /// any handle to this store is unresolved, the log holds what recovery needs and this
    /// returns [`StoreError::RecoveryRequired`] (as [`MonitorError::Store`]), whether or not
    /// the actor is still running; any other failure to truncate is returned the same way.
    /// [`MonitorError::Stopped`] means only that the actor has exited for another reason; it
    /// says nothing about whether the log was truncated.
    pub fn quiesce(&self) -> Result<(), MonitorError> {
        let (reply, reply_rx) = mpsc::channel();
        let replied = match self.tx.send(Command::Barrier { reply }) {
            Ok(()) => reply_rx.recv().ok(),
            Err(_) => None,
        };
        match replied {
            Some(result) => result.map_err(MonitorError::Store),
            None => Err(self.stopped()),
        }
    }

    /// Why the actor is no longer answering: the store-wide fence if one is up, otherwise
    /// plain [`MonitorError::Stopped`].
    fn stopped(&self) -> MonitorError {
        if self.store.recovery_required() {
            MonitorError::Store(StoreError::RecoveryRequired {
                root: self.store.root().to_path_buf(),
            })
        } else {
            MonitorError::Stopped
        }
    }

    /// Repairs the replicas reads have reported damaged, and reports what was done
    /// (`D317`, `D339`).
    ///
    /// A read records damage it finds but cannot act on it: repairing is a write and a write
    /// needs a nonce the reader has no reason to hold (`D61`). The actor holds one, so the
    /// repair belongs here.
    ///
    /// **Calling this is optional.** The actor already drains the backlog whenever it runs
    /// out of queued work, so a store that takes writes repairs itself. This is for the store
    /// that does not: on a read-only workload the actor never reaches that idle point,
    /// because it never leaves it. A caller that wants repair on a schedule drives it here
    /// rather than growing a clock inside the actor.
    pub fn repair_reported_damage(&self) -> Result<BacklogRepairReport, MonitorError> {
        let (reply, reply_rx) = mpsc::channel();
        self.tx
            .send(Command::RepairReportedDamage { reply })
            .map_err(|_| self.stopped())?;
        reply_rx
            .recv()
            .map_err(|_| self.stopped())?
            .map_err(MonitorError::Store)
    }
}

impl Drop for WriteMonitor {
    fn drop(&mut self) {
        // Ask the actor to finish, then wait for its thread so no write is lost and no
        // thread is leaked. Errors mean the actor already stopped -- nothing to do.
        let _ = self.tx.send(Command::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl std::fmt::Debug for WriteMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteMonitor")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

/// The actor loop: apply queued writes one at a time, truncating the commit log each
/// time the queue drains (quiescence), until the queue closes or a
/// [`Command::Shutdown`] arrives.
///
/// Quiescence is the moment the actor has applied a command and no further command is
/// waiting: the queue is empty and, because each command completes synchronously
/// before the next is dequeued, no transaction is open. That is exactly D53's
/// quiescent moment, so the actor resets the commit log to empty there -- the
/// O(1)-startup commitment (TXN-3). The reset is sequenced *ahead of* the last
/// command's reply, so a caller that has observed its write complete has also observed
/// the reset: there is no window in which the queue looks drained but the log is still
/// populated.
///
/// A [`Command::Barrier`] is the on-demand version of that same reset: it truncates the
/// log (no transaction is open when the actor holds a command, so the log is
/// discardable) and confirms to the caller, then the actor keeps serving.
fn run(store: Store, nonce_source: &mut NonceSource, rx: Receiver<Command>, feed: &CommitFeed) {
    while let Ok(command) = rx.recv() {
        let mut pending = command;
        // Whether this burst applied a write, so a burst that ends on a snapshot or a repair
        // still owes the quiescence reset (and, after a snapshot, the backlog drain).
        let mut wrote = false;
        loop {
            // One hold per command, not per burst: a transform's read and write must not
            // interleave with another handle's write, but a monitor whose queue never empties
            // must still let that handle in between commands.
            let _writer = store.exclusive();
            // Checked before any callback runs, so a snapshot or plan never reads a store
            // another handle left half-applied. Dropping `pending` tells its caller.
            if store.recovery_required() {
                return;
            }
            match pending {
                Command::Snapshot(read) => {
                    read(&store);
                    if store.recovery_required() {
                        return;
                    }
                    match rx.try_recv() {
                        Ok(next) => pending = next,
                        Err(mpsc::TryRecvError::Empty) => {
                            if wrote {
                                let _ = on_quiescence(&store);
                                drain_repair_backlog(&store, nonce_source);
                            }
                            break;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                // Drain-and-stop: the store is quiescent, so reset the log before exit.
                Command::Shutdown => {
                    let _ = on_quiescence(&store);
                    return;
                }
                // On-demand quiescence: no transaction is open, so the log is
                // discardable -- truncate it and confirm, then keep serving.
                Command::Barrier { reply } => {
                    let reset = on_quiescence(&store);
                    wrote = false;
                    let _ = reply.send(reset);
                    if store.recovery_required() {
                        return;
                    }
                    match rx.try_recv() {
                        Ok(next) => pending = next,
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                // Maintenance, not a caller's write, but it runs on the actor because it
                // needs the nonce source and the store's quiescence (`D339`).
                Command::RepairReportedDamage { reply } => {
                    let _ = reply.send(store.repair_reported_damage(&mut *nonce_source));
                    if store.recovery_required() {
                        return;
                    }
                    match rx.try_recv() {
                        Ok(next) => pending = next,
                        Err(mpsc::TryRecvError::Empty) => {
                            if wrote {
                                let _ = on_quiescence(&store);
                            }
                            break;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                // Apply the command's work but hold its reply, so a quiescence reset can
                // be sequenced ahead of unblocking the caller.
                work => {
                    let (deliver_reply, recovery_required) =
                        apply_deferred(&store, nonce_source, work, feed);
                    wrote = true;
                    // Another handle's failed transaction fences this monitor too.
                    if recovery_required || store.recovery_required() {
                        deliver_reply();
                        return;
                    }
                    match rx.try_recv() {
                        Ok(next) => {
                            deliver_reply();
                            pending = next;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            let _ = on_quiescence(&store);
                            // Reply first: the caller's write is done, and a repair pass must
                            // never be charged to its latency (`D339`).
                            deliver_reply();
                            drain_repair_backlog(&store, nonce_source);
                            break;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            deliver_reply();
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// Applies one non-[`Shutdown`](Command::Shutdown) command's work and returns a closure
/// that delivers its reply.
///
/// Splitting the work from the reply lets [`run`] sequence a quiescence reset between
/// them. [`Command::Shutdown`], [`Command::Barrier`] and
/// [`Command::RepairReportedDamage`] never reach here ([`run`] handles them), so their arms
/// are inert no-ops.
fn apply_deferred(
    store: &Store,
    nonce_source: &mut NonceSource,
    command: Command,
    feed: &CommitFeed,
) -> (Box<dyn FnOnce()>, bool) {
    let observer = current_observer(feed);
    match command {
        Command::Transform {
            node,
            document,
            transform,
            reply,
        } => {
            let result = apply_transform(store, nonce_source, &node, &document, transform);
            if let (Some(observer), Ok(body)) = (&observer, &result) {
                observer.committed(&node, &document, body);
            }
            (
                Box::new(move || {
                    let _ = reply.send(result);
                }),
                false,
            )
        }
        Command::ConditionalWrite {
            node,
            document,
            body,
            expected,
            reply,
        } => {
            // The body is consumed by the write, so an observed monitor keeps a copy to
            // report. An unobserved one -- the whole service until an operator attaches --
            // clones nothing.
            let reported = observer.as_ref().map(|_| body.clone());
            let result = apply_conditional(store, nonce_source, &node, &document, body, expected);
            if let (Some(observer), Some(body), Ok(WriteOutcome::Committed)) =
                (&observer, &reported, &result)
            {
                observer.committed(&node, &document, body);
            }
            (
                Box::new(move || {
                    let _ = reply.send(result);
                }),
                false,
            )
        }
        Command::ApplyTransaction { swaps, reply } => {
            let (result, recovery_required) =
                commit_swaps(store, nonce_source, &swaps, observer.as_deref());
            (
                Box::new(move || {
                    let _ = reply.send(result);
                }),
                recovery_required,
            )
        }
        Command::PlanTransaction { plan, reply } => match plan(store) {
            None => (
                Box::new(move || {
                    let _ = reply.send(Ok(None));
                }),
                false,
            ),
            Some(swaps) => {
                let (result, recovery_required) =
                    commit_swaps(store, nonce_source, &swaps, observer.as_deref());
                (
                    Box::new(move || {
                        let _ = reply.send(result.map(Some));
                    }),
                    recovery_required,
                )
            }
        },
        Command::Shutdown
        | Command::Barrier { .. }
        | Command::RepairReportedDamage { .. }
        | Command::Snapshot(_) => (Box::new(|| {}), false),
    }
}

/// Truncates the commit log at the D53 quiescent moment (TXN-3).
///
/// The automatic resets after a drain and at shutdown discard the result: a failed
/// truncation there costs at most a non-O(1) next startup, and nobody was told it happened.
/// A [`Command::Barrier`] returns it, because its caller is told the log is empty.
fn on_quiescence(store: &Store) -> Result<(), StoreError> {
    store.quiesce_commit_log()
}

/// Repairs whatever reads have reported damaged, now that the writer has nothing else to do
/// (`D317`, `D339`).
///
/// This is the caller `D317`'s probe argument needs: rotation makes every read examine a
/// different replica, and what those reads find is rebuilt here rather than waiting for a
/// whole-store scrub.
///
/// Best-effort and unreported, like the log truncation above. A repair that fails leaves the
/// store exactly as the read already found it -- one replica short and still serving from a
/// verified copy -- so failing the single writer over it would turn a redundancy loss into an
/// outage. The findings stay on the backlog's floor rather than being retried here; the next
/// read of the same blob reports it again, and the scrub remains the exhaustive backstop.
fn drain_repair_backlog(store: &Store, nonce_source: &mut NonceSource) {
    let _ = store.repair_reported_damage(nonce_source);
}

/// Reads the current quorum body, applies `transform`, and replicates the result.
fn apply_transform(
    store: &Store,
    nonce_source: &mut NonceSource,
    node: &NodePath,
    document: &DocumentName,
    transform: TransformFn,
) -> Result<DocumentBody, StoreError> {
    // Writer-side, so being superseded would mean a second writer: resolve once more and let
    // `DocumentOvertaken` report it if it persists.
    let current = crate::read_within(store, node, document, crate::read::WRITER_READ_ATTEMPTS)?;
    let new_body = transform(current.as_ref());
    let nonce = nonce_source();
    store.write_document_replicated(node, document, &new_body, &nonce)?;
    Ok(new_body)
}

/// Reads the current quorum digest and replicates `body` only if it equals `expected`.
fn apply_conditional(
    store: &Store,
    nonce_source: &mut NonceSource,
    node: &NodePath,
    document: &DocumentName,
    body: DocumentBody,
    expected: ContentDigest,
) -> Result<WriteOutcome, StoreError> {
    let current = crate::read_within(store, node, document, crate::read::WRITER_READ_ATTEMPTS)?;
    let current_digest = current.as_ref().map(DocumentBody::digest);
    if current_digest != Some(expected) {
        return Ok(WriteOutcome::Rejected {
            current: current_digest,
        });
    }
    let nonce = nonce_source();
    store.write_document_replicated(node, document, &body, &nonce)?;
    Ok(WriteOutcome::Committed)
}

/// Commits `swaps` and reports the committed ones to `observer`.
///
/// A transaction is all-or-nothing (D53), so one committed outcome reports every swap and a
/// rejected one reports none.
fn commit_swaps(
    store: &Store,
    nonce_source: &mut NonceSource,
    swaps: &SwapSet,
    observer: Option<&dyn CommitObserver>,
) -> (Result<TransactionOutcome, StoreError>, bool) {
    let (result, recovery_required) = apply_swaps(store, nonce_source, swaps);
    if let (Some(observer), Ok(TransactionOutcome::Committed)) = (observer, &result) {
        for swap in swaps.swaps() {
            match swap.body() {
                Some(body) => observer.committed(swap.node(), swap.document(), body),
                None => observer.removed(swap.node(), swap.document()),
            }
        }
    }
    (result, recovery_required)
}

/// Applies `swaps` as one `D53` atomic multi-document transaction.
///
/// Mints one nonce to name the transaction's staging temps and delegates to
/// [`Store::apply_transaction`], which performs the CAS precondition phase, the atomic
/// stage-then-commit, and the single fsync. A rejected transaction still consumes the
/// minted nonce; nonces need not be gapless, so the gap is harmless.
fn apply_swaps(
    store: &Store,
    nonce_source: &mut NonceSource,
    swaps: &SwapSet,
) -> (Result<TransactionOutcome, StoreError>, bool) {
    let nonce = nonce_source();
    let result = store.apply_transaction(swaps, &nonce);
    (result, store.recovery_required())
}

/// A failure from submitting work to the [`WriteMonitor`].
#[derive(Debug)]
pub enum MonitorError {
    /// The write failed inside the store.
    Store(StoreError),
    /// The monitor actor is no longer running, so the write was not applied.
    Stopped,
}

impl std::fmt::Display for MonitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MonitorError::Store(e) => write!(f, "write monitor store error: {e}"),
            MonitorError::Stopped => f.write_str("the write monitor actor has stopped"),
        }
    }
}

impl std::error::Error for MonitorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MonitorError::Store(e) => Some(e),
            MonitorError::Stopped => None,
        }
    }
}

impl From<StoreError> for MonitorError {
    fn from(e: StoreError) -> Self {
        MonitorError::Store(e)
    }
}
