// Copyright (c) Michael Grier.

//! Blob ingest: the caller's stream in, our digest out (`D315`, `CVS-6`).
//!
//! The order of operations here is forced, not chosen. A blob's name contains the digest
//! of its content, so the destination is unknown until the stream has ended -- which makes
//! staging mandatory and the rename into place the commit.

use super::*;
use crate::naming::{Artifact, BlobName, BlobRef, PathHash};
use std::io::Read;
use std::sync::atomic::Ordering;

/// How much of the caller's stream to hold at once.
///
/// The stream is consumed in bounded chunks rather than read whole, because its size is
/// the caller's business and not something canopy will allocate on trust. 64 KiB is large
/// enough that the per-append cost is amortised and small enough to be uninteresting to
/// hold; it is not a format constant and may change freely.
const INGEST_CHUNK: usize = 64 * 1024;

/// The staging temp name for an ingest under `nonce`: `blob.tmp.<nonce>`.
///
/// Named for the nonce rather than the content, because at staging time the content's name
/// is precisely what is not yet known. The `.tmp.` infix is the same marker the rest of the
/// store's staging uses, so one recovery sweep recognises both.
fn blob_temp_name(nonce: &Nonce) -> String {
    format!("blob{TEMP_INFIX}{}", nonce.as_str())
}

impl Store {
    /// Ingests `source` as the `artifact` of the document at (`node`, `document`),
    /// returning the name the blob was written under (`D315`).
    ///
    /// The bytes are hashed on the single pass that writes them. **The caller's claim about
    /// the content is not accepted, and cannot be** -- a stream the caller can modify cannot
    /// be read a second time to check one, and trusting an external process's assertion
    /// about bytes it supplied is not a trust boundary worth having. What the caller gets
    /// back is what canopy computed.
    ///
    /// Every replica is durable before this returns, because `D315` requires a blob to be
    /// durable before any pointer names it: the reverse order leaves a dangling pointer,
    /// which nothing in the store detects or repairs.
    ///
    /// Writing an identical body to the same path a second time lands on the same name and
    /// replaces it byte-for-byte, which is a no-op rather than an error. That is why
    /// `D315`'s cleanup is a set difference rather than "delete the old one" -- see
    /// [`BlobName::version_prefix`].
    ///
    /// # Failure leaves the committed store untouched
    ///
    /// All replicas are staged before any is renamed, so an interruption during staging
    /// changes nothing committed and leaves only `.staging/` orphans -- swept by recovery
    /// when the nonce is a journaled transaction's, and otherwise inert until removed. An
    /// interruption *during* the rename phase can leave a subset of replicas carrying the
    /// new blob -- which is harmless, because a blob is immutable and named by its content:
    /// the partial set is a real blob with too few copies, not a corrupt one, and no
    /// pointer names it yet.
    pub(crate) fn ingest_blob<R: Read>(
        &self,
        node: &NodePath,
        document: &DocumentName,
        artifact: Artifact,
        source: R,
        nonce: &Nonce,
    ) -> Result<BlobName, StoreError> {
        self.refuse_if_unresolved()?;
        let path_hash = PathHash::of(node, document);
        let replicas = self.genesis.blob_replica_count;

        // Phase 1: the caller's stream lands once, hashed as it goes. Outside the writer gate:
        // the staging temp is private to this nonce, and a slow stream must not hold off every
        // other writer.
        let digest = self.stage_blob_stream(0, source, nonce)?;
        let name = BlobName::new(path_hash, digest, artifact);
        let _writer = self.writable()?;

        // Phase 2: the remaining replicas copy what we just wrote, never the caller's
        // stream -- the stream is the thing that cannot be read twice, and our own staged
        // bytes are already the bytes we hashed. Copied in the same bounded chunks the
        // stream arrived in, so a large blob is never held whole.
        let first = self.blob_staging_temp(0, nonce);
        for index in 1..replicas {
            let temp = self.blob_staging_temp(index, nonce);
            self.vfs.create_dir_all(
                temp.parent()
                    .expect("a staging temp always has a parent directory"),
            )?;
            self.vfs.write(&temp, &[])?;
            let mut hasher = IngestHasher::new();
            let mut offset = 0u64;
            loop {
                let chunk = self.vfs.read_at(&first, offset, INGEST_CHUNK)?;
                if chunk.is_empty() {
                    break;
                }
                hasher.update(&chunk);
                self.vfs.append(&temp, &chunk)?;
                offset += chunk.len() as u64;
            }
            // The copy is checked against the digest as it streams, so a staged first replica
            // that changed under us is refused rather than replicated.
            if hasher.finish() != digest {
                return Err(StoreError::StagedReplicaMismatch { staged: first });
            }
            self.vfs.fsync(&temp)?;
        }

        // Phase 3: every replica is staged and durable; commit them by rename.
        let modifying = self.begin_modification();
        for index in 0..replicas {
            let shard = self.blob_shard_dir(index, path_hash);
            self.create_dir_durably(&shard)?;
            let destination = shard.join(name.file_name());
            self.vfs
                .rename(&self.blob_staging_temp(index, nonce), &destination)?;
            self.vfs.fsync(&shard)?;
        }

        drop(modifying);
        self.sweep_blob_staging(nonce, replicas);
        Ok(name)
    }

    /// Streams `source` into partition `index`'s staging temp, hashing on the same pass.
    ///
    /// Returns the digest of what was written. The file is fsynced before this returns, so
    /// the bytes the digest describes are the bytes on disk rather than the bytes in a
    /// buffer.
    ///
    /// The whole stream is never held: it arrives in bounded chunks, each hashed and
    /// appended before the next is read.
    fn stage_blob_stream<R: Read>(
        &self,
        index: usize,
        mut source: R,
        nonce: &Nonce,
    ) -> Result<ContentDigest, StoreError> {
        let temp = self.blob_staging_temp(index, nonce);
        self.vfs.create_dir_all(
            temp.parent()
                .expect("a staging temp always has a parent directory"),
        )?;

        // Truncating first makes the ingest idempotent against a reused nonce: what follows
        // appends, and appending to a survivor of an earlier attempt would silently
        // concatenate two bodies into one that hashes to neither.
        self.vfs.write(&temp, &[])?;

        let mut hasher = IngestHasher::new();
        let mut buffer = vec![0u8; INGEST_CHUNK];
        loop {
            let read = source
                .read(&mut buffer)
                .map_err(|error| StoreError::IngestSource { error })?;
            if read == 0 {
                break;
            }
            let chunk = &buffer[..read];
            hasher.update(chunk);
            self.vfs.append(&temp, chunk)?;
        }

        self.vfs.fsync(&temp)?;
        Ok(hasher.finish())
    }

    /// Partition `index`'s staging temp for an ingest under `nonce`.
    pub(super) fn blob_staging_temp(&self, index: usize, nonce: &Nonce) -> PathBuf {
        blob_partition_root(&self.root, index)
            .join(STAGING_DIR)
            .join(blob_temp_name(nonce))
    }

    /// The shard directory holding `path_hash`'s blobs in partition `index`.
    pub(super) fn blob_shard_dir(&self, index: usize, path_hash: PathHash) -> PathBuf {
        blob_partition_root(&self.root, index).join(path_hash.shard(self.genesis.shard_width))
    }

    /// Discards the staging temps an ingest left behind.
    ///
    /// Best-effort by design: the temps have already been renamed away on the success path,
    /// and a leftover on the failure path is an orphan a recovery sweep collects. Failing
    /// an ingest that has already committed every replica, because tidying afterwards did
    /// not work, would turn a cleanup problem into a data problem.
    pub(crate) fn sweep_blob_staging(&self, nonce: &Nonce, replicas: usize) {
        for index in 0..replicas {
            let _ = self.vfs.remove_file(&self.blob_staging_temp(index, nonce));
        }
    }
}

impl Store {
    /// Reads the blob at `reference`, verifying it and falling through to the next replica
    /// if it does not verify (`D316`, `D317`).
    ///
    /// **Nothing is derived.** The reference carries the shard and the name, so this does no
    /// path hashing, no shard computation, no name assembly, and never enumerates a
    /// directory. `h(P)` for replica selection is parsed from the name it was handed.
    ///
    /// # Verification is not a second read
    ///
    /// The expected digest is already in the name, so checking it costs CPU over bytes that
    /// have been read anyway -- no additional I/O. That is what makes it affordable on every
    /// read rather than on a scrub.
    ///
    /// A copy that does not verify is **corrupt, not outvoted** (`D316`): it is recognised
    /// by examining it alone, without reference to any other replica, so there is no fan-out
    /// and no vote. Falling through to the next replica is the whole point of having one.
    ///
    /// # Rotation makes every read a probe
    ///
    /// Preference is `(h(P) + epoch) mod N` over a **coarse** epoch (`D317`). Verification
    /// without rotation would re-check the same copy forever and never find a rotted one;
    /// rotation without verification would serve bad bytes and detect nothing. Together, a
    /// bad replica surfaces the next time any document resident on it is read, rather than
    /// at the next whole-store scrub.
    pub fn read_blob(&self, reference: &BlobRef) -> Result<BlobRead, StoreError> {
        let replicas = self.genesis.blob_replica_count;
        let name = reference.name();
        let expected = name.digest();
        let start = name
            .path_hash()
            .rotation_seed()
            .wrapping_add(self.blob_epoch())
            % replicas as u64;

        let mut missing = Vec::new();
        let mut corrupt = Vec::new();
        for step in 0..replicas {
            let index = (start as usize + step) % replicas;
            let path = blob_partition_root(&self.root, index).join(reference.relative_path());
            let bytes = match self.vfs.read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == VfsErrorKind::NotFound => {
                    missing.push(index);
                    continue;
                }
                Err(error) => return Err(StoreError::Vfs(error)),
            };

            if ContentDigest::of(&bytes) != expected {
                corrupt.push(index);
                continue;
            }

            self.report_blob_rejections(reference, &corrupt, Some(index));
            return Ok(BlobRead {
                bytes,
                served_by: index,
                missing,
                corrupt,
            });
        }

        self.report_blob_rejections(reference, &corrupt, None);
        Err(StoreError::BlobUnavailable {
            blob: reference.relative_path(),
            missing: missing.len(),
            corrupt: corrupt.len(),
        })
    }

    /// Removes every blob owned by `path_hash` whose content digest is not in `keep`,
    /// across every replica (`D315`).
    ///
    /// Returns how many files were removed.
    ///
    /// # A set difference, not "delete the old one"
    ///
    /// The caller states what should survive -- "after this transaction the blobs belonging
    /// to `P` are exactly these" -- and this removes the complement. Phrasing it the other
    /// way round is destructive in a case that looks impossible until it happens: writing
    /// identical content to the same path lands on the *same* name, so the "old" blob and
    /// the "new" one are one file, and deleting the old one deletes the live one.
    ///
    /// An empty `keep` is the document's blobs going away entirely, which is what deletion
    /// is.
    ///
    /// # Matched by prefix, not by assembled name
    ///
    /// Ownership and version are decided from a file name's first two fields, and the
    /// artifact suffix is deliberately not inspected (see
    /// [`BlobName::owner_and_version`]). Assembling the names to remove would mean removing
    /// only the artifact types this build knows about, so a blob written by a newer build
    /// would survive every sweep an older one performed -- accumulating forever, owned by a
    /// version that no longer exists.
    ///
    /// **This is the only reason enumeration has to exist.** No read path ever lists a
    /// directory (`D321`); a sweep does, because only a prefix scan stays complete as the
    /// artifact vocabulary grows.
    ///
    /// # What it will not remove
    ///
    /// A file whose name is not shaped like one of ours is left alone, so something that
    /// wandered into a shard directory is not collected by a cleanup that does not own it.
    /// Empty shard directories are also left: they cost nothing, they will be reused, and
    /// removing one races an ingest creating the same shard for another document.
    pub(crate) fn retain_blobs(
        &self,
        path_hash: PathHash,
        keep: &[ContentDigest],
    ) -> Result<usize, StoreError> {
        let _writer = self.writable()?;
        let owner = path_hash.render();
        let survivors: Vec<String> = keep.iter().map(ContentDigest::to_base32).collect();
        let mut removed = 0;
        let _modifying = self.begin_modification();

        for index in 0..self.genesis.blob_replica_count {
            let shard = self.blob_shard_dir(index, path_hash);
            let entries = match self.vfs.read_dir(&shard) {
                Ok(entries) => entries,
                // No shard directory means nothing of this owner's was ever written here,
                // which is a clean sweep rather than a failure.
                Err(error) if error.kind() == VfsErrorKind::NotFound => continue,
                Err(error) => return Err(StoreError::Vfs(error)),
            };

            let mut removed_here = false;
            for entry in entries {
                let file_name = entry.file_name.to_string_lossy().into_owned();
                let Some((entry_owner, version)) = BlobName::owner_and_version(&file_name) else {
                    continue;
                };
                if entry_owner != owner || survivors.iter().any(|kept| kept == version) {
                    continue;
                }

                tracing::debug!(target: "canopy::blob", event = "blob_swept",
                    root = ?self.root, partition = index, blob = %file_name,
                    "removing a blob no surviving version owns");
                self.vfs.remove_file(&shard.join(&file_name))?;
                removed += 1;
                removed_here = true;
            }

            if removed_here {
                self.vfs.fsync(&shard)?;
            }
        }

        Ok(removed)
    }

    /// Discards the blobs a completed transaction superseded, for every document it touched
    /// (`D328`).
    ///
    /// **Run after `end`, never before.** Deleting at or before the commit point would
    /// destroy the body a rollback returns to, and would leave the still-current pointer
    /// naming a blob that is gone -- the dangling pointer `D327` exists to make unreachable.
    ///
    /// **Idempotent, and safe to replay in any order**, which is what lets recovery re-run it
    /// without tracking how far the live pass got.
    pub(crate) fn collect_superseded(&self, prepared: &PreparedSet) -> Result<(), StoreError> {
        for swap in prepared.swaps() {
            self.collect_document_blobs(swap.node(), swap.document())?;
        }
        Ok(())
    }

    /// Keeps only the blob the namespace currently points at for (`node`, `document`), and
    /// discards every other blob that document owns (`D328`).
    ///
    /// # The keep-set comes from the namespace, not from the log record
    ///
    /// The obvious implementation keeps the digest the `prepare` record names, and it is
    /// wrong in a way that destroys data. Recovery replays ended transactions in log order,
    /// so a log holding two writes to one document would replay the *first* transaction's
    /// collection -- keeping v1 and deleting v2 -- and then replay the second's, by which
    /// time the blob the current pointer names has already been removed. The result is a
    /// dangling pointer produced by the cleanup.
    ///
    /// Asking the namespace instead makes the question order-independent: whatever is live
    /// *now* survives, and everything else is by definition superseded. This is `D320`'s
    /// division of labour applied to deletion -- the namespace is the authority on which
    /// content is current, and the blob tree only holds content.
    ///
    /// It also makes removal fall out rather than needing its own path: a removed document
    /// has no pointer, so nothing is kept and every blob it owned goes.
    ///
    /// # An undecidable namespace means delete nothing
    ///
    /// If the replicas disagree or cannot be parsed, this cannot say what is live, and the
    /// two ways of being wrong are not symmetric: keeping a superseded blob wastes space that
    /// the next collection reclaims, while deleting a live one is unrecoverable -- the blob is
    /// named by its content, so there is nothing to restore it from. Absence is the one
    /// unavailable case that *is* decidable, and only because every replica agrees on it.
    pub(crate) fn collect_document_blobs(
        &self,
        node: &NodePath,
        document: &DocumentName,
    ) -> Result<(), StoreError> {
        let keep = match self.read_pointer(node, document) {
            Ok(read) => vec![read.pointer().target().name().digest()],
            // Every replica missing: the document is absent, so nothing it owned is live.
            Err(StoreError::PointerUnavailable {
                rejected: 0,
                disagreeing: 0,
                ..
            }) => Vec::new(),
            Err(StoreError::PointerUnavailable { .. }) => {
                tracing::warn!(target: "canopy::blob", event = "collection_skipped",
                    root = ?self.root, node = %node, document = document.as_str(),
                    "the namespace cannot say which blob is live; keeping every copy");
                return Ok(());
            }
            Err(other) => return Err(other),
        };
        // The keep-set was read from the live namespace; a pointer change whose parent sync
        // failed would otherwise let a crash restore a pointer to a blob deleted here.
        self.sync_pointer_entry(node, document)?;
        self.retain_blobs(PathHash::of(node, document), &keep)?;
        Ok(())
    }

    /// Logs every replica refused for failing verification, with what the read then did
    /// about it (`D326`).
    ///
    /// Deferred until the outcome is known rather than warned at the point of failure,
    /// because the remediation is the half an operator needs and it is not knowable yet:
    /// the same "verification failed" line would otherwise appear whether the read went on
    /// to serve correct bytes from another replica or gave up entirely, which are a store
    /// working as designed and an outage.
    ///
    /// Absence is not reported here. A replica with no copy is a normal state -- a blob
    /// mid-replication, or one whose sweep has run -- and warning about it would bury the
    /// case that means something.
    fn report_blob_rejections(
        &self,
        reference: &BlobRef,
        corrupt: &[usize],
        served_by: Option<usize>,
    ) {
        if corrupt.is_empty() {
            return;
        }
        let remediation = match served_by {
            Some(index) => format!("served from partition {index}, which verified"),
            None => "none: no replica verified, the read failed".to_string(),
        };
        for index in corrupt {
            tracing::warn!(target: "canopy::blob", event = "blob_replica_rejected",
                root = ?self.root, partition = index, blob = %reference,
                cause = "the bytes do not hash to the name they are stored under",
                remediation = %remediation,
                "a blob replica was refused");
        }
    }

    /// The current blob-read epoch (`D317`).
    ///
    /// Advances once every `BLOB_EPOCH_READS` blob reads rather than once per read. A
    /// per-read epoch would let two reads of one document land on different replicas inside
    /// a write window and observe the value going backwards -- permitted by the store's
    /// per-document guarantee, hostile to a caller with monotonicity expectations, and it
    /// multiplies the cached working set by `N`.
    ///
    /// Counted rather than timed, so selection stays deterministic: a store whose tests
    /// forbid runtime randomness cannot have its layout decided by a clock.
    pub fn blob_epoch(&self) -> u64 {
        self.blob_reads.fetch_add(1, Ordering::Relaxed) / BLOB_EPOCH_READS
    }
}

/// What a successful [`Store::read_blob`] found.
///
/// Carries the replicas that failed as well as the bytes, because `D316` has repair fire on
/// **verification failure** -- so the read is the thing that knows which copies need
/// replacing, and dropping that on the floor would make the repair path re-discover it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRead {
    bytes: Vec<u8>,
    served_by: usize,
    missing: Vec<usize>,
    corrupt: Vec<usize>,
}

impl BlobRead {
    /// The verified bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the read, yielding its bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Which partition served the bytes.
    pub fn served_by(&self) -> usize {
        self.served_by
    }

    /// Partitions that had no copy, in the order they were tried.
    pub fn missing(&self) -> &[usize] {
        &self.missing
    }

    /// Partitions whose copy did not hash to its own name, in the order they were tried.
    pub fn corrupt(&self) -> &[usize] {
        &self.corrupt
    }

    /// Whether every replica tried before this one was intact -- that is, whether the read
    /// found what it preferred.
    pub fn was_first_choice(&self) -> bool {
        self.missing.is_empty() && self.corrupt.is_empty()
    }
}

/// The path of blob partition `index` under `root`: `<root>/files/p{index}` (`D314`).
pub(crate) fn blob_partition_root(root: &Path, index: usize) -> PathBuf {
    root.join(BLOB_TREE_DIR)
        .join(format!("{PARTITION_PREFIX}{index}"))
}

/// A streaming SHA-256, so the digest is produced by the same pass that writes the bytes.
///
/// Wraps the hasher rather than exposing it, so that "the digest is computed here, from
/// these bytes" stays the only way to obtain one during ingest.
struct IngestHasher(sha2::Sha256);

impl IngestHasher {
    fn new() -> IngestHasher {
        use sha2::Digest;
        IngestHasher(sha2::Sha256::new())
    }

    fn update(&mut self, chunk: &[u8]) {
        use sha2::Digest;
        self.0.update(chunk);
    }

    fn finish(self) -> ContentDigest {
        use sha2::Digest;
        let out = self.0.finalize();
        let mut bytes = [0u8; ContentDigest::LEN];
        bytes.copy_from_slice(&out);
        ContentDigest::from_bytes(bytes)
    }
}
