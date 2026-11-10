// Copyright (c) Michael Grier.
// Split from store.rs at 1303f9a.

//! The two-phase transactional apply (TXN-2): the outcome vocabulary, the
//! single-document convenience, and the staging phase that precedes the commit
//! point.

use std::collections::HashSet;

use super::*;
use crate::naming::Artifact;

/// The precondition a rejected transaction's failing swap required (D54).
///
/// Names why a [`TransactionOutcome::Rejected`] swap's precondition did not hold: it
/// expected a specific digest (a stale compare-and-set), expected the document to be
/// absent (a conditional-create that lost the race), or -- for a node removal -- expected
/// the node to be empty (`BG-6`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectedPrecondition {
    /// The swap required the document to hold exactly this digest (D54 CAS).
    ExpectedDigest(ContentDigest),
    /// The swap required the document to be absent (conditional-create).
    ExpectedAbsent,
    /// A node removal required the node to be empty once this transaction's own
    /// changes applied, and something would remain (`BG-6`).
    ///
    /// Carries what would remain, so a caller learns *why* rather than only that the
    /// node was not empty -- the difference between "another document is still there"
    /// and "a child node is still there" is the difference between two quite different
    /// mistakes.
    NodeNotEmpty {
        /// Documents that would remain at the node.
        documents: Vec<String>,
        /// Child nodes that would remain beneath it.
        nodes: Vec<String>,
    },
    /// A node removal found the node holding no documents or child nodes, but its directory
    /// still holds entries the namespace does not recognise -- a dotfile, a name that is not
    /// a valid document or node, or another file type. The removal cannot apply over them.
    NodeHoldsUnrecognizedEntries {
        /// The unrecognised entry names, across every replica.
        entries: Vec<String>,
    },
}

/// The outcome of [`Store::apply_transaction`] (D53/TXN-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionOutcome {
    /// Every swap's precondition held and the whole set was committed to all
    /// partitions.
    Committed,
    /// A conditional swap's precondition did not hold, so the **entire** transaction
    /// was rejected and nothing -- log or disk -- was written.
    Rejected {
        /// The node of the swap whose precondition failed.
        node: String,
        /// The document of the swap whose precondition failed, or `None` when the
        /// failing precondition belonged to a node removal, which names no document.
        document: Option<String>,
        /// The precondition the failing swap required.
        expected: RejectedPrecondition,
        /// The digest the document actually held (`None` when absent, and always
        /// `None` for a node removal, which has no body to digest).
        current: Option<ContentDigest>,
    },
}

impl TransactionOutcome {
    /// Whether the transaction committed.
    pub fn is_committed(&self) -> bool {
        matches!(self, TransactionOutcome::Committed)
    }

    /// Whether the transaction was rejected by a conditional swap's precondition.
    pub fn is_rejected(&self) -> bool {
        matches!(self, TransactionOutcome::Rejected { .. })
    }
}

impl Store {
    /// Roll back only a transaction proven not to have a prepare record.
    /// A prepared or unreadable journal must survive for reopen recovery.
    pub(crate) fn rollback_unprepared(&self, txn: &Nonce) -> Result<bool, StoreError> {
        let _writer = self.writable()?;
        let records = self.commit_log().read()?;
        if records
            .iter()
            .any(|record| record.txn() == txn && matches!(record, LogRecord::Prepare { .. }))
        {
            return Ok(false);
        }
        self.sweep_transaction_staging(txn)?;
        // The `begin` names what this transaction may already have ingested (`D329`); collect
        // it now, because truncating the log below erases the only record of where to look.
        for record in &records {
            if let LogRecord::Begin { txn: id, targets } = record
                && id == txn
            {
                for (node, document) in targets {
                    self.collect_document_blobs(node, document)?;
                }
            }
        }
        self.quiesce_commit_log()?;
        Ok(true)
    }

    /// Atomically applies a single-document update -- the one-element case of a D53
    /// transaction (D54, TXN-6).
    ///
    /// The degenerate transaction: `swap` is applied all-or-nothing through the same
    /// two-phase commit as [`apply_transaction`](Store::apply_transaction), landing the
    /// new body in all N partitions atomically. An unconditional [`Swap::new`] always
    /// commits; a conditional [`Swap::conditional`] commits only when the current quorum
    /// body's digest matches the expected value (the D54 compare-and-set) and is
    /// otherwise [`Rejected`](TransactionOutcome::Rejected) with nothing touched. `txn`
    /// is the caller-supplied transaction id (D61); canopy mints no tokens of its own.
    ///
    /// This is the generic store surface's single-document write verb; the multi-document
    /// sibling is [`apply_transaction`](Store::apply_transaction). Anything domain-specific
    /// belongs to the caller, layered on top (D54).
    pub fn update_document(
        &self,
        swap: Swap,
        txn: &Nonce,
    ) -> Result<TransactionOutcome, StoreError> {
        self.apply_transaction(&SwapSet::single(swap), txn)
    }

    /// Atomically applies `swaps` as one D53 transaction, journaled to the commit log
    /// (TXN-2).
    ///
    /// The two-phase apply (D53/D8):
    /// 1. **Preconditions.** Every conditional swap's expected digest (the D54
    ///    compare-and-set) is checked against the current quorum body first; if any
    ///    fails, the whole transaction is [`Rejected`](TransactionOutcome::Rejected)
    ///    and nothing -- log or disk -- is touched.
    /// 2. **Begin + ingest.** A `begin` record brackets the transaction, then every
    ///    write's body is ingested into the content store: a crash here leaves only
    ///    unreferenced blobs, and moves no pointer.
    /// 3. **Prepare.** The full swap set is written as a `prepare` record and the log
    ///    is fsync'd -- D53's single commit / linearization point.
    /// 4. **Apply.** Each swap's pointer is moved over the namespace replicas. Every
    ///    step is idempotent -- an entry already in place is a no-op -- so the apply
    ///    is safely repeatable by recovery.
    /// 5. **End.** An `end` record records completion and the transaction's staging
    ///    trees are swept.
    ///
    /// `txn` is the caller-supplied transaction id (D61): it names both the log
    /// records and the per-transaction staging directories, as canopy mints no tokens
    /// of its own. Multi-document all-or-nothing is exactly what the log buys: a
    /// crash after the `prepare` fsync rolls the transaction forward, before it rolls
    /// back -- but that recovery replay is TXN-4; TXN-2 lands the forward apply and
    /// makes it idempotent so the replay can drive it.
    ///
    /// # A failure is rolled back when it can be, and fences the store when it cannot
    ///
    /// A transaction that fails before its `prepare` is durable is rolled back here: its
    /// staging is swept, what it ingested is collected, and the journal is reset. One that
    /// fails after -- or whose journal cannot be read to tell -- may have passed its commit
    /// point, so every handle to this store then refuses writes with
    /// [`StoreError::RecoveryRequired`] and leaves the journal untouched until the store is
    /// reopened and recovery has finished it.
    pub fn apply_transaction(
        &self,
        swaps: &SwapSet,
        txn: &Nonce,
    ) -> Result<TransactionOutcome, StoreError> {
        // Held from the precondition checks through the last phase, so no other writer in
        // this process can change what the checks saw before the transaction lands.
        let _writer = self.writable()?;
        let mut began = false;
        let result = self.apply_transaction_gated(swaps, txn, &mut began);
        if result.is_err() && began && !matches!(self.rollback_unprepared(txn), Ok(true)) {
            self.writer.mark_unresolved();
        }
        result
    }

    /// [`apply_transaction`](Store::apply_transaction)'s phases, run with the gate held.
    /// `began` is set before the `begin` record is appended, so the caller knows whether a
    /// failure has anything in the journal to roll back.
    fn apply_transaction_gated(
        &self,
        swaps: &SwapSet,
        txn: &Nonce,
        began: &mut bool,
    ) -> Result<TransactionOutcome, StoreError> {
        // Phase 1: preconditions (D54 CAS), before any log or disk mutation.
        for swap in swaps.swaps() {
            match swap.precondition() {
                Precondition::Unconditional => {}
                Precondition::ExpectedDigest(expected) => {
                    // Writer-side, so being superseded would mean a second writer: resolve
                    // once more and let `DocumentOvertaken` report it if it persists.
                    let current = crate::read_within(
                        self,
                        swap.node(),
                        swap.document(),
                        crate::read::WRITER_READ_ATTEMPTS,
                    )?;
                    let current_digest = current.as_ref().map(DocumentBody::digest);
                    if current_digest != Some(expected) {
                        return Ok(TransactionOutcome::Rejected {
                            node: swap.node().to_string(),
                            document: Some(swap.document().as_str().to_string()),
                            expected: RejectedPrecondition::ExpectedDigest(expected),
                            current: current_digest,
                        });
                    }
                }
                Precondition::ExpectedAbsent => {
                    let current = crate::read_within(
                        self,
                        swap.node(),
                        swap.document(),
                        crate::read::WRITER_READ_ATTEMPTS,
                    )?;
                    let current_digest = current.as_ref().map(DocumentBody::digest);
                    if current_digest.is_some() {
                        return Ok(TransactionOutcome::Rejected {
                            node: swap.node().to_string(),
                            document: Some(swap.document().as_str().to_string()),
                            expected: RejectedPrecondition::ExpectedAbsent,
                            current: current_digest,
                        });
                    }
                }
            }
        }

        // Phase 1b: node-removal emptiness (`BG-6`). A different *kind* of precondition
        // from the digest CAS above -- it asks what the node will hold once this
        // transaction's own swaps apply, where a CAS asks what a document holds now. The
        // computation is sound because `D116` gives a store root exactly one writer, so
        // nothing can change the node between this check and phase 4.
        for path in swaps.node_removals() {
            if let Some(rejection) = self.node_removal_rejection(path, swaps)? {
                return Ok(rejection);
            }
        }

        // Phase 1c: namespace shape. A write that would make one name both a document and a
        // child node can never apply, so it must be refused before `prepare` makes it a
        // commitment that every later recovery would try, and fail, to roll forward.
        self.check_transaction_namespace(swaps)?;

        let log = self.commit_log();

        // Phase 2: begin, then ingest every write's body into the content store. A removal
        // ingests nothing: there is no body to place, and the pointer it takes must stay in
        // place until after the commit point. It also means a crash before `prepare` needs
        // no rollback for removals -- there is nothing to roll back.
        // The `begin` names every target about to be ingested, and is fsynced before the
        // first ingest -- so a transaction that dies mid-ingest leaves a durable record of
        // where to look for what it abandoned (`D329`). Removals ingest nothing and are
        // not named.
        *began = true;
        log.append(&LogRecord::Begin {
            txn: txn.clone(),
            targets: swaps
                .swaps()
                .iter()
                .filter(|swap| !swap.is_removal())
                .map(|swap| (swap.node().clone(), swap.document().clone()))
                .collect(),
        })?;
        let mut prepared = Vec::with_capacity(swaps.swaps().len());
        for swap in swaps.swaps() {
            if swap.is_removal() {
                prepared.push(PreparedSwap::removing(swap));
                continue;
            }
            let body = swap
                .body()
                .expect("a swap that is not a removal carries a body");

            // The blob lands and is fsynced on every replica before this returns, which is
            // the ordering rule this phase exists to enforce (`D327`): the content is
            // durable before the `prepare` that names it, so no pointer written after that
            // commit point -- by this apply or by a later roll-forward -- can name a blob
            // that is not on disk. A crash here leaves an unreferenced blob, which is a
            // space leak rather than a corruption, because nothing points at it.
            let name = self.ingest_blob(
                swap.node(),
                swap.document(),
                Artifact::Body,
                body.as_bytes(),
                txn,
            )?;

            // The digest recorded is the one the content store computed over the bytes it
            // wrote, never one re-derived from the in-memory body: the blob's name, the
            // pointer's target and this record are then the same value by construction
            // rather than three computations that are expected to agree.
            prepared.push(PreparedSwap::writing(swap, name.digest()));
        }
        // Deepest first, so a child removed alongside its parent is gone before the parent's
        // non-recursive remove runs. The order is recorded, so recovery replays it the same way.
        let mut removals = swaps.node_removals().to_vec();
        removals.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        let prepared = PreparedSet::new(prepared, removals);

        // Phase 3: prepare + fsync -- the single commit point.
        log.append(&LogRecord::Prepare {
            txn: txn.clone(),
            prepared: prepared.clone(),
        })?;

        // Phase 4: move each swap's pointer over the namespace replicas.
        //
        // An error here surfaces to the caller **by design**, even though `prepare` is
        // already durable (the commit point has passed). We deliberately do *not* swallow
        // it and report `Committed`: a failure after our own `prepare` is an anomaly worth
        // surfacing, and no commit is lost by reporting it -- `prepare` without `end` is
        // exactly the roll-forward signal `recover()` acts on at the next `open()`,
        // re-running these idempotent steps to completion. Reporting the error is therefore
        // "committed but not yet fully applied; a fault interrupted the apply", not "the
        // commit was lost".
        for swap in prepared.swaps() {
            self.apply_swap_to_namespace(swap, txn)?;
        }

        // Node removals come after every document change, so a node whose last document
        // this transaction removes is empty by the time its own removal runs.
        for path in prepared.node_removals() {
            self.apply_node_removal(path)?;
        }

        // Phase 5: end, then sweep the transaction's staging trees.
        log.append(&LogRecord::End { txn: txn.clone() })?;
        self.sweep_transaction_staging(txn)?;

        // Phase 6: discard what this transaction superseded (`D328`). After `end` because
        // deleting any earlier destroys the body a rollback returns to. A crash between the
        // two leaves superseded blobs behind, and recovery re-runs this pass for every ended
        // transaction still in the log -- which is why no collector or scan is needed.
        //
        // An error here surfaces for the same reason phase 4's does: the transaction is
        // committed and applied, and what is left undone is cleanup that recovery will
        // complete, but a fault is still worth reporting rather than swallowing.
        self.collect_superseded(&prepared)?;

        Ok(TransactionOutcome::Committed)
    }

    /// Refuses a set whose writes collide with the namespace on disk or with each other
    /// (`D222`): a document named where a child node is, or a node nested under a document.
    fn check_transaction_namespace(&self, swaps: &SwapSet) -> Result<(), StoreError> {
        let writes: Vec<&Swap> = swaps.swaps().iter().filter(|s| !s.is_removal()).collect();
        // Removals too: removing a name that is a child node would fail only after `prepare`.
        for swap in swaps.swaps() {
            for index in 0..self.genesis.pointer_replica_count {
                self.check_namespace_tree_collision(index, swap.node(), swap.document())?;
            }
        }
        // Node paths (with every ancestor) the set addresses: `under_writes` from writes only,
        // `under_any` from every swap. A written document whose full path is in `under_any`
        // has a write or removal beneath it; a removed one in `under_writes` would have
        // become a directory by the time the removal ran.
        fn prefixes(swap: &Swap) -> Vec<Vec<&str>> {
            let components: Vec<&str> = swap.node().components().collect();
            (1..=components.len())
                .map(|depth| components[..depth].to_vec())
                .collect()
        }
        let under_writes: HashSet<Vec<&str>> = writes.iter().flat_map(|s| prefixes(s)).collect();
        let under_any: HashSet<Vec<&str>> = swaps.swaps().iter().flat_map(prefixes).collect();
        for swap in swaps.swaps() {
            let mut entry: Vec<&str> = swap.node().components().collect();
            entry.push(swap.document().as_str());
            let nested = if swap.is_removal() {
                &under_writes
            } else {
                &under_any
            };
            if nested.contains(&entry) {
                return Err(StoreError::NameIsChildNode {
                    path: swap.node().clone(),
                    name: swap.document().as_str().to_owned(),
                });
            }
        }
        // A removed node that is, or lies beneath, a document already on disk can never be
        // removed as a directory.
        for removal in swaps.node_removals() {
            for index in 0..self.genesis.pointer_replica_count {
                self.check_node_path_is_not_document(index, removal)?;
            }
        }
        // Node removals: a written document at a removed node's path, or at one of its
        // ancestors, turns that path into a file (or puts a file above it) before the removal
        // runs, and the removal then fails after `prepare` on every recovery. A removed node
        // above a write is refused when the set is built (`SwapSet::with_node_removals`).
        for swap in &writes {
            let mut entry: Vec<&str> = swap.node().components().collect();
            entry.push(swap.document().as_str());
            for removal in swaps.node_removals() {
                let removed: Vec<&str> = removal.components().collect();
                if removed.starts_with(&entry) {
                    return Err(StoreError::NameIsChildNode {
                        path: swap.node().clone(),
                        name: swap.document().as_str().to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Whether removing `path` must be rejected, given what this transaction's own swaps
    /// will leave behind (`BG-6`).
    ///
    /// `rmdir` semantics: a node goes only if it ends up holding no documents and no
    /// child nodes. "Ends up" is the load-bearing part -- the driving case is removing a
    /// node's last document *and* the node in one transaction, so a check against
    /// pre-transaction state would refuse exactly what this exists to serve. So the
    /// current contents are adjusted by the set's own effects: documents it removes here
    /// are subtracted, documents it writes here are added.
    ///
    /// A write at or beneath `path` cannot appear, because [`SwapSet::with_node_removals`]
    /// refuses that set outright as contradictory; the addition below therefore only ever
    /// concerns... nothing, and is kept as a total accounting rather than an assumption,
    /// so the rule stays correct if that construction check is ever relaxed.
    fn node_removal_rejection(
        &self,
        path: &NodePath,
        swaps: &SwapSet,
    ) -> Result<Option<TransactionOutcome>, StoreError> {
        let children = self.list_children(path)?;

        let mut documents: BTreeSet<String> = children
            .documents()
            .iter()
            .map(|document| document.as_str().to_owned())
            .collect();
        for swap in swaps.swaps() {
            if swap.node() != path {
                continue;
            }
            let name = swap.document().as_str().to_owned();
            if swap.is_removal() {
                documents.remove(&name);
            } else {
                documents.insert(name);
            }
        }

        // A child node this transaction would create cannot exist either -- the same
        // construction check forbids a write beneath a removed node. A child this transaction
        // removes is gone by the time this node's removal runs (removals apply deepest
        // first), and its own emptiness is checked by its own pass through here.
        let nodes: Vec<String> = children
            .nodes()
            .iter()
            .filter(|child| !swaps.node_removals().contains(child))
            .map(NodePath::to_string)
            .collect();

        if !documents.is_empty() || !nodes.is_empty() {
            return Ok(Some(TransactionOutcome::Rejected {
                node: path.to_string(),
                document: None,
                expected: RejectedPrecondition::NodeNotEmpty {
                    documents: documents.into_iter().collect(),
                    nodes,
                },
                current: None,
            }));
        }

        // Logically empty is not physically empty: the listing skips dotfiles and names it
        // cannot parse, and the non-recursive remove would fail on them after `prepare`.
        let entries = self.unremovable_entries(path, swaps)?;
        if entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(TransactionOutcome::Rejected {
            node: path.to_string(),
            document: None,
            expected: RejectedPrecondition::NodeHoldsUnrecognizedEntries { entries },
            current: None,
        }))
    }

    /// Every entry in `path`'s directory, on any replica, that this transaction's removals
    /// will not take away first.
    fn unremovable_entries(
        &self,
        path: &NodePath,
        swaps: &SwapSet,
    ) -> Result<Vec<String>, StoreError> {
        let depth = path.components().count();
        let removed_documents: HashSet<&str> = swaps
            .swaps()
            .iter()
            .filter(|swap| swap.is_removal() && swap.node() == path)
            .map(|swap| swap.document().as_str())
            .collect();
        let removed_children: HashSet<&str> = swaps
            .node_removals()
            .iter()
            .filter(|child| {
                child.components().count() == depth + 1
                    && child
                        .components()
                        .zip(path.components())
                        .all(|(a, b)| a == b)
            })
            .filter_map(|child| child.components().last())
            .collect();

        let mut entries = BTreeSet::new();
        for index in 0..self.genesis.pointer_replica_count {
            let dir =
                namespace::namespace_partition_root(&self.root, index).join(path.relative_path());
            let listed = match self.vfs.read_dir(&dir) {
                Ok(listed) => listed,
                Err(error) if super::is_absent(error.kind()) => continue,
                Err(error) => return Err(StoreError::Vfs(error)),
            };
            for entry in listed {
                let name = entry.file_name.to_string_lossy();
                let goes = match entry.file_type {
                    FileType::File => removed_documents.contains(name.as_ref()),
                    FileType::Dir => removed_children.contains(name.as_ref()),
                    _ => false,
                };
                if !goes {
                    entries.insert(name.into_owned());
                }
            }
        }
        Ok(entries.into_iter().collect())
    }

    /// Stages `swap`'s body to partition `index`'s per-transaction staging temp
    /// `p{index}/.staging/<txn>/<node>/<document>.tmp.<txn>` and fsyncs it, writes an
    /// intent sidecar recording the staged body's digest, and fsyncs the staging
    /// directory chain so both survive a crash before the `prepare` commit point,
    /// returning the paths [`Store::commit_swap`] will rename (TXN-2).
    ///
    /// The extra `<node>/` level under the transaction's staging directory keeps two
    /// swaps that write the same document name at different nodes from colliding on
    /// one temp path. Non-destructive like the LAY-3 stager: the committed copy is
    /// untouched, and the committed node directory is created so the later rename can
    /// land.
    ///
    /// Directory-entry durability matters here (D71): the temp and intent sidecar are
    /// fsynced for their contents, but the newly-created staging directories that hold
    /// their dir-entries must also be fsynced, or a crash could leave a durable
    /// `prepare` record with no staged temp on disk. Recovery would then see `NotFound`
    /// on the rename and -- finding a stale committed body still present -- wrongly
    /// treat the swap as already applied, silently losing the committed update. The
    /// intent sidecar closes that gap: [`Store::commit_swap`] only tolerates a missing
    /// temp once the committed body's digest matches the recorded intent. The committed
    /// node directory's own durability is carried by [`Store::commit_swap`]'s
    /// post-rename `fsync`, and [`Store::roll_forward`] re-creates it before replaying,
    /// so it is not fsynced here.
    /// Ingests every write of `txn` (phase 2 of the apply) without moving any pointer,
    /// returning each swap as its `prepare` record would carry it.
    ///
    /// The crash point recovery rolls forward or back from: exposed to recovery tests so
    /// they can reconstruct an interrupted transaction's on-disk state through the real
    /// ingest path, then drive [`Store::open`] over it. A removal ingests nothing and is
    /// returned unchanged, matching what phase 2 does with it.
    #[cfg(test)]
    pub(crate) fn stage_transaction_for_test(
        &self,
        txn: &Nonce,
        swaps: &SwapSet,
    ) -> Result<Vec<PreparedSwap>, StoreError> {
        let mut prepared = Vec::new();
        for swap in swaps.swaps() {
            let Some(body) = swap.body() else {
                prepared.push(PreparedSwap::removing(swap));
                continue;
            };
            let name = self.ingest_blob(
                swap.node(),
                swap.document(),
                Artifact::Body,
                body.as_bytes(),
                txn,
            )?;
            prepared.push(PreparedSwap::writing(swap, name.digest()));
        }
        Ok(prepared)
    }
}
