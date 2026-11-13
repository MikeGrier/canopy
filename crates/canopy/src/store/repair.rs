// Copyright (c) Michael Grier.

//! Repair over the content-verified layout, and the quarantine that precedes it
//! (`D316`, `D320`, `D326`, `D331`).
//!
//! # Repair fires on verification failure, not on disagreement
//!
//! A blob is valid or corrupt, decided by examining one copy against the digest in its own
//! name (`D316`). There is no stale state in between, so a repair here can never fire against
//! a partially applied write -- the case that made the old majority-vote repair dangerous.
//!
//! A pointer *does* have a stale state, and the rule is the one `D320` keeps: a replica that
//! parses cleanly but trails the majority is **behind, not broken**, and is never repaired.
//! The transaction completing is what converges it, which recovery drives. Only a replica that
//! fails to parse is re-replicated. Putting staleness in the repair path would make repair
//! responsible for landing updates, which is the log's job.
//!
//! # The source is a verified copy, never a vote
//!
//! Every repair rewrites from bytes that were checked against the expected digest before being
//! used. For a blob that is what [`Store::read_blob`](crate::Store::read_blob) already
//! returned; for a pointer it is the entry the majority agreed on, re-checked on the way out.
//! No repair reconstructs content by counting replicas.

use std::collections::VecDeque;

use super::blob::blob_partition_root;
use super::namespace::namespace_partition_root;
use super::*;
use crate::naming::BlobRef;

/// How many corrupt copies a single partition keeps for diagnosis before it stops keeping
/// them (`D331`).
///
/// The quarantine exists so an operator can tell a bug in our own write path from failing
/// hardware, and that question is answered by **how the corruption is distributed**, not by
/// having every instance of it: the four-hundredth bad copy on a dying disk says nothing the
/// tenth did not. A cap keeps the evidence and bounds the cost.
///
/// **It bounds the count, not the bytes.** Document bodies are opaque to canopy and have no
/// ceiling, so the disk cost is this many bodies per partition and cannot be stated in bytes
/// here. Chosen large enough that the clustering is unmistakable and small enough that the
/// worst case is survivable.
pub const QUARANTINE_LIMIT: usize = 64;

/// How many suspect blobs the backlog holds before it starts dropping findings (`D339`).
///
/// The backlog is an **accelerator, not a guarantee** -- [`Store::scrub_content`] remains the
/// exhaustive pass -- so overflowing it costs a repair its promptness and never its
/// eventuality. That is what makes a bound safe to impose at all. Sized to absorb the
/// clustered findings of one failing disk between two drains; a store producing more than
/// this many distinct damaged blobs between drains has a problem the backlog was not going to
/// solve.
pub const REPAIR_BACKLOG_LIMIT: usize = 256;

/// Blobs a read found damaged, waiting for a writer to act on them (`D339`).
///
/// `D317` makes every read a probe, and a probe is only worth taking if something acts on
/// what it finds. A read cannot act on it itself -- repairing is a write, a write needs a
/// nonce, and `D61` has canopy mint none -- so the finding is parked here for the write
/// monitor, which already holds a nonce source and already knows when the store is idle.
///
/// **Identities, not findings.** Only the [`BlobRef`] is kept; which replicas were bad is
/// re-established when the repair runs. A tally recorded at read time could be acted on
/// after a write had already moved things, and repairing from a stale finding is precisely
/// the hazard `D316` removed from the old majority-vote path.
///
/// **Bounded and lossy on purpose.** A dying disk can produce findings faster than a drain
/// consumes them, and an unbounded queue would answer that by exhausting memory. Dropping is
/// safe here in a way it would not be for a write queue, because every finding it holds is
/// **re-derivable**: the next read of that blob reports it again, and the scrub finds it
/// regardless. `D341` makes that property, rather than a judgement about severity, the test
/// for what may live here.
///
/// **Full means evict the oldest, not refuse the newest** (`D341`). The two are not
/// equivalent under the deduplication above: entries are ordered by when the damage was first
/// seen, so the oldest is the one most likely to have been repaired already -- by a scrub, by
/// a rewrite of the document, or by an earlier drain -- while the newest is the one most
/// likely still true. Refusing the newest would preserve the least useful end of the queue.
#[derive(Debug, Default)]
pub(crate) struct RepairBacklog {
    suspects: VecDeque<BlobRef>,
    dropped: u64,
}

impl RepairBacklog {
    /// Records `reference` as needing repair, ignoring a blob already waiting.
    ///
    /// Deduplicating matters more than it looks: a hot document whose replica is rotted is
    /// read repeatedly and would otherwise fill the backlog with one blob, evicting every
    /// other finding.
    pub(crate) fn note(&mut self, reference: BlobRef) {
        if self.suspects.contains(&reference) {
            return;
        }
        if self.suspects.len() >= REPAIR_BACKLOG_LIMIT {
            self.suspects.pop_front();
            self.dropped += 1;
        }
        self.suspects.push_back(reference);
    }

    /// Removes and returns everything waiting, oldest first.
    pub(crate) fn take(&mut self) -> VecDeque<BlobRef> {
        std::mem::take(&mut self.suspects)
    }

    pub(crate) fn len(&self) -> usize {
        self.suspects.len()
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// What a repair did, per replica (`D326`).
///
/// Names the remediation as well as the fault, because "verification failed" is the same line
/// whether the store healed itself or gave up, and those are a store working as designed and
/// an outage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepairOutcome {
    replaced: Vec<usize>,
    sequestered: Vec<usize>,
    discarded: Vec<usize>,
}

impl RepairOutcome {
    /// Replicas rewritten from a verified copy.
    pub fn replaced(&self) -> &[usize] {
        &self.replaced
    }

    /// Replicas whose corrupt bytes were moved aside and kept.
    pub fn sequestered(&self) -> &[usize] {
        &self.sequestered
    }

    /// Replicas whose corrupt bytes were dropped because the quarantine was full.
    ///
    /// A full quarantine never blocks a repair: redundancy is correctness and the evidence is
    /// diagnostics, so when the two compete the bytes lose. That this list is non-empty is
    /// itself worth an operator's attention -- it means a partition has produced more
    /// corruption than the store was prepared to keep.
    pub fn discarded(&self) -> &[usize] {
        &self.discarded
    }

    /// Whether the repair changed nothing, because nothing needed changing.
    pub fn is_noop(&self) -> bool {
        self.replaced.is_empty()
    }
}

/// Every replica's standing for one blob, from a pass that examines all of them (`D331`).
///
/// [`Store::read_blob`](crate::Store::read_blob) stops at the first copy that verifies, so
/// what it reports about the others covers only the ones it tried on the way. That is right
/// for a read -- `D317` makes every read a partial probe and rotation covers the rest over
/// time -- and wrong for a scrub, which is the pass that must leave nothing unexamined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobVerification {
    intact: Vec<usize>,
    missing: Vec<usize>,
    corrupt: Vec<usize>,
}

impl BlobVerification {
    /// Replicas holding a copy that hashes to its own name.
    pub fn intact(&self) -> &[usize] {
        &self.intact
    }

    /// Replicas holding no copy at all.
    pub fn missing(&self) -> &[usize] {
        &self.missing
    }

    /// Replicas holding a copy that does not hash to its own name.
    pub fn corrupt(&self) -> &[usize] {
        &self.corrupt
    }

    /// Whether every replica holds an intact copy.
    pub fn is_whole(&self) -> bool {
        self.missing.is_empty() && self.corrupt.is_empty()
    }
}

impl Store {
    /// Examines **every** replica of `reference` without stopping at the first good one.
    ///
    /// The exhaustive counterpart to [`Store::read_blob`](crate::Store::read_blob), for the
    /// scrub. A read stops as soon as it can answer; a scrub exists precisely to look at what
    /// a read would not have reached.
    pub fn verify_blob(&self, reference: &BlobRef) -> Result<BlobVerification, StoreError> {
        let expected = reference.name().digest();
        let mut verification = BlobVerification {
            intact: Vec::new(),
            missing: Vec::new(),
            corrupt: Vec::new(),
        };

        for index in 0..self.genesis.blob_replica_count {
            let path = blob_partition_root(&self.root, index).join(reference.relative_path());
            match self.vfs.read(&path) {
                Ok(bytes) if ContentDigest::of(&bytes) == expected => {
                    verification.intact.push(index)
                }
                Ok(_) => verification.corrupt.push(index),
                Err(error) if error.kind() == VfsErrorKind::NotFound => {
                    verification.missing.push(index)
                }
                Err(error) => return Err(StoreError::Vfs(error)),
            }
        }
        Ok(verification)
    }

    /// Restores `reference`'s redundancy from `source`, which must already have been verified
    /// against the digest in the blob's name (`D316`, `D326`).
    ///
    /// Corrupt copies are moved aside before being replaced, so the repair does not destroy
    /// the only record of what the corruption looked like (`D326`). Missing copies are simply
    /// written -- there is nothing to preserve.
    ///
    /// The source is re-checked here rather than trusted. It costs a hash over bytes already
    /// in hand, and it makes "a repair never propagates content it has not verified" a
    /// property of this function instead of an obligation on every caller.
    ///
    /// The repair is made under a nonce drawn from the store's source and claimed for it, and
    /// the nonce is returned with the outcome.
    pub fn repair_blob(
        &self,
        reference: &BlobRef,
        source: &[u8],
        missing: &[usize],
        corrupt: &[usize],
    ) -> Result<(RepairOutcome, Nonce), StoreError> {
        let _writer = self.writable()?;
        let claim = self.claim_fresh_nonce()?;
        let outcome = self.repair_blob_as(reference, source, missing, corrupt, claim.nonce())?;
        Ok((outcome, claim.nonce().clone()))
    }

    /// [`repair_blob`](Store::repair_blob) under `nonce`, which the caller has claimed or, in a
    /// test, chosen.
    pub(crate) fn repair_blob_as(
        &self,
        reference: &BlobRef,
        source: &[u8],
        missing: &[usize],
        corrupt: &[usize],
        nonce: &Nonce,
    ) -> Result<RepairOutcome, StoreError> {
        let _writer = self.writable()?;
        let count = self.genesis.blob_replica_count;
        if let Some(&index) = missing.iter().chain(corrupt).find(|&&index| index >= count) {
            return Err(StoreError::ReplicaIndexOutOfRange { index, count });
        }
        if reference.shard_width() != self.genesis.shard_width {
            return Err(StoreError::ReferenceShardWidth {
                expected: self.genesis.shard_width,
                found: reference.shard_width(),
            });
        }
        if ContentDigest::of(source) != reference.name().digest() {
            return Err(StoreError::BlobUnavailable {
                blob: reference.relative_path(),
                missing: 0,
                corrupt: 1,
            });
        }

        let path_hash = reference.name().path_hash();
        let mut outcome = RepairOutcome::default();
        let _modifying = self.begin_modification();

        for &index in corrupt {
            let entry = blob_partition_root(&self.root, index).join(reference.relative_path());
            match self.sequester(blob_partition_root(&self.root, index), &entry, nonce)? {
                Sequestered::Kept => outcome.sequestered.push(index),
                Sequestered::Dropped => outcome.discarded.push(index),
            }
        }

        for &index in corrupt.iter().chain(missing) {
            let shard = self.blob_shard_dir(index, path_hash);
            self.create_dir_durably(&shard)?;
            let temp = self.blob_staging_temp(index, nonce);
            self.vfs.create_dir_all(
                temp.parent()
                    .expect("a staging temp always has a parent directory"),
            )?;
            self.vfs.write(&temp, source)?;
            self.vfs.fsync(&temp)?;
            self.vfs
                .rename(&temp, &shard.join(reference.name().file_name()))?;
            self.vfs.fsync(&shard)?;
            outcome.replaced.push(index);
        }

        self.report_repair(&reference.relative_path(), &outcome);
        Ok(outcome)
    }

    /// Restores the namespace entry for (`node`, `document`) on the replicas that could not
    /// parse it, and on those that have none (`D320`, `D326`).
    ///
    /// **Stale replicas are left exactly as they are.** A pointer that parses but trails the
    /// majority is behind rather than broken, and converges when the transaction that moved it
    /// completes. Rewriting it here would make repair a writer of updates.
    ///
    /// **`read` is re-established, not trusted.** The namespace is read again under the writer
    /// gate and only what that read finds is repaired. If the majority has moved since `read`
    /// was taken, the report is obsolete and nothing is written: replaying it would put a
    /// superseded pointer back over a current replica.
    ///
    /// The repair is made under a nonce drawn from the store's source and claimed for it, and
    /// the nonce is returned with the outcome.
    pub fn repair_pointer(
        &self,
        node: &NodePath,
        document: &DocumentName,
        read: &PointerRead,
    ) -> Result<(RepairOutcome, Nonce), StoreError> {
        let _writer = self.writable()?;
        let claim = self.claim_fresh_nonce()?;
        let outcome = self.repair_pointer_as(node, document, read, claim.nonce())?;
        Ok((outcome, claim.nonce().clone()))
    }

    /// [`repair_pointer`](Store::repair_pointer) under `nonce`, which the caller has claimed
    /// or, in a test, chosen.
    pub(crate) fn repair_pointer_as(
        &self,
        node: &NodePath,
        document: &DocumentName,
        read: &PointerRead,
        nonce: &Nonce,
    ) -> Result<RepairOutcome, StoreError> {
        let _writer = self.writable()?;
        let current = self.read_pointer(node, document)?;
        if current.pointer() != read.pointer() {
            return Ok(RepairOutcome::default());
        }
        let read = &current;
        let bytes = read.pointer().render();
        let mut outcome = RepairOutcome::default();
        let _modifying = self.begin_modification();

        for &index in read.rejected() {
            let entry = self.pointer_entry_path(index, node, document);
            match self.sequester(namespace_partition_root(&self.root, index), &entry, nonce)? {
                Sequestered::Kept => outcome.sequestered.push(index),
                Sequestered::Dropped => outcome.discarded.push(index),
            }
        }

        for &index in read.rejected().iter().chain(read.missing()) {
            let entry = self.pointer_entry_path(index, node, document);
            let parent = entry
                .parent()
                .expect("an entry always has a parent directory");
            self.create_dir_durably(parent)?;
            let temp = self.pointer_staging_temp(index, document, nonce);
            self.vfs.create_dir_all(
                temp.parent()
                    .expect("a staging temp always has a parent directory"),
            )?;
            self.vfs.write(&temp, &bytes)?;
            self.vfs.fsync(&temp)?;
            self.vfs.rename(&temp, &entry)?;
            self.vfs.fsync(parent)?;
            outcome.replaced.push(index);
        }

        self.sweep_pointer_staging(document, nonce, self.genesis.pointer_replica_count);
        self.report_repair(&format!("{node}/{}", document.as_str()), &outcome);
        Ok(outcome)
    }

    /// Moves the file at `entry` into `tree`'s quarantine, or drops it if the quarantine is
    /// already holding its limit (`D331`).
    ///
    /// The quarantine is **per partition** because the diagnostic question is where the
    /// corruption clusters: a hundred bad copies under one replica is a failing disk, the same
    /// hundred spread evenly is a bug in our own write path. Encoding the replica in the path
    /// makes that a directory listing rather than an analysis.
    ///
    /// The `nonce` discriminates repeated sequestrations of the same name, so a copy that goes
    /// bad twice does not overwrite the record of the first time.
    fn sequester(
        &self,
        tree: PathBuf,
        entry: &Path,
        nonce: &Nonce,
    ) -> Result<Sequestered, StoreError> {
        let quarantine = tree.join(QUARANTINE_DIR);
        // Durably named, or a crash could lose evidence this reports as kept.
        self.create_dir_durably(&quarantine)?;

        let held = match self.vfs.read_dir(&quarantine) {
            Ok(entries) => entries.len(),
            Err(error) if error.kind() == VfsErrorKind::NotFound => 0,
            Err(error) => return Err(StoreError::Vfs(error)),
        };
        if held >= QUARANTINE_LIMIT {
            tracing::warn!(target: "canopy::repair", event = "quarantine_full",
                root = ?self.root, quarantine = ?quarantine, held,
                limit = QUARANTINE_LIMIT, discarded = ?entry,
                "a corrupt copy was discarded rather than kept: the quarantine is full");
            match self.vfs.remove_file(entry) {
                Ok(()) => {}
                Err(error) if error.kind() == VfsErrorKind::NotFound => {}
                Err(error) => return Err(StoreError::Vfs(error)),
            }
            return Ok(Sequestered::Dropped);
        }

        let name = entry
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unnamed".to_owned());
        let kept = quarantine.join(format!("{name}.{}", nonce.as_str()));
        // Kept evidence outlives the claim on its nonce, so an earlier repair under the same
        // nonce can have left this name; the rename below would replace it.
        match self.vfs.metadata(&kept) {
            Ok(_) => {
                return Err(StoreError::NonceCollision {
                    nonce: nonce.clone(),
                });
            }
            Err(error) if error.kind() == VfsErrorKind::NotFound => {}
            Err(error) => return Err(StoreError::Vfs(error)),
        }
        match self.vfs.rename(entry, &kept) {
            Ok(()) => {}
            // Already gone is success: something else removed it, and the point of the move
            // was to stop it being served.
            Err(error) if error.kind() == VfsErrorKind::NotFound => return Ok(Sequestered::Kept),
            Err(error) => return Err(StoreError::Vfs(error)),
        }
        self.vfs.fsync(&quarantine)?;
        Ok(Sequestered::Kept)
    }

    /// Reports what a repair replaced, kept and dropped (`D326`).
    ///
    /// Deferred to the end of the repair rather than logged per replica, for the same reason
    /// the read defers its rejections: the remediation is the half an operator needs, and it
    /// is not knowable until the repair has finished.
    fn report_repair(&self, subject: &str, outcome: &RepairOutcome) {
        if outcome.is_noop() {
            return;
        }
        tracing::info!(target: "canopy::repair", event = "repaired",
            root = ?self.root, subject,
            replaced = ?outcome.replaced, sequestered = ?outcome.sequestered,
            discarded = ?outcome.discarded,
            "restored redundancy from a verified copy");
    }
}

/// Whether a corrupt copy was kept for diagnosis or dropped because the quarantine was full.
enum Sequestered {
    Kept,
    Dropped,
}
