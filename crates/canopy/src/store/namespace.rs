// Copyright (c) Michael Grier.

//! The namespace: pointer read and write over `namespace/` (`D320`, `D321`, `CVS-11`).
//!
//! This is the mutable half of the store, and the rule here is the opposite of the blob
//! tree's. A blob is immutable, so a copy is intact or corrupt and examining one copy is a
//! complete diagnosis. A pointer has versions, so "is this copy intact?" does not answer
//! "is this copy current?" -- a stale pointer is a *valid* older pointer, and no amount of
//! per-copy verification sees it. Only counting distinguishes them, which is why a pointer
//! read votes and a blob read does not.

use super::*;
use crate::naming::{Artifact, BlobName, BlobRef, PathHash};
use crate::pointer::{Pointer, PointerError};

impl Store {
    /// Writes `pointer` as the namespace entry for (`node`, `document`), on every replica.
    ///
    /// Staged then renamed, with every replica staged before any is committed, so an
    /// interruption during staging changes no entry a reader can see and leaves only
    /// `.staging/` orphans -- swept by recovery when the nonce is a journaled transaction's,
    /// and otherwise inert until removed.
    ///
    /// This is the primitive, not the transaction. `Store::write_body_pointer` is the
    /// seam the transaction uses, and `D327` is what orders it after the blob it names is
    /// durable -- the reverse order leaves a **dangling pointer**, which `D315` notes
    /// nothing in the store detects or repairs.
    pub(crate) fn write_pointer(
        &self,
        node: &NodePath,
        document: &DocumentName,
        pointer: &Pointer,
        nonce: &Nonce,
    ) -> Result<(), StoreError> {
        let _writer = self.writable()?;
        pointer
            .check_owner(PathHash::of(node, document))
            .map_err(StoreError::InvalidPointer)?;
        // A reference at another width names a shard this store never writes into.
        if pointer.target().shard_width() != self.genesis.shard_width {
            return Err(StoreError::InvalidPointer(PointerError::ShardWidth {
                expected: self.genesis.shard_width,
                found: pointer.target().shard_width(),
            }));
        }
        let bytes = pointer.render();
        let replicas = self.genesis.pointer_replica_count;

        // Before anything is staged, so a refusal costs no fsync and leaves nothing behind.
        for index in 0..replicas {
            self.check_namespace_tree_collision(index, node, document)?;
        }

        for index in 0..replicas {
            let temp = self.pointer_staging_temp(index, document, nonce);
            self.vfs.create_dir_all(
                temp.parent()
                    .expect("a staging temp always has a parent directory"),
            )?;
            self.vfs.write(&temp, &bytes)?;
            self.vfs.fsync(&temp)?;
        }

        let _modifying = self.begin_modification();
        for index in 0..replicas {
            let entry = self.pointer_entry_path(index, node, document);
            let parent = entry
                .parent()
                .expect("an entry always has a parent directory");
            self.create_dir_durably(parent)?;
            self.vfs
                .rename(&self.pointer_staging_temp(index, document, nonce), &entry)?;
            self.vfs.fsync(parent)?;
        }

        self.sweep_pointer_staging(document, nonce, replicas);
        Ok(())
    }

    /// Writes the namespace entry naming the `Body` blob that `digest` identifies for
    /// (`node`, `document`) (`D327`).
    ///
    /// **The caller must already have made that blob durable.** This assembles the name
    /// from `(node, document, digest)` rather than being handed a [`BlobRef`], which is
    /// the same reconstruction a recovery replay performs from a `prepare` record --
    /// so the transaction and the replay cannot drift into naming blobs differently.
    /// Nothing here verifies the blob exists: a read that costs an I/O on every write, to
    /// re-check an ordering the apply path already guarantees, would buy nothing the
    /// scrub does not already cover (`CVS-18`).
    pub(crate) fn write_body_pointer(
        &self,
        node: &NodePath,
        document: &DocumentName,
        digest: ContentDigest,
        nonce: &Nonce,
    ) -> Result<(), StoreError> {
        let path_hash = PathHash::of(node, document);
        let name = BlobName::new(path_hash, digest, Artifact::Body);
        let target = BlobRef::new(name, self.genesis.shard_width)
            .expect("the genesis shard width was validated at init and open");
        self.write_pointer(node, document, &Pointer::new(path_hash, target), nonce)
    }

    /// Applies one prepared swap to the namespace: writes the entry for a write, drops it
    /// for a removal (`D328`).
    ///
    /// **The namespace is changed only after the commit point**, by the live apply's phase 4
    /// or by a recovery roll-forward replaying it -- which is why both call this rather than
    /// each spelling out the two cases. A write's target was made durable during staging, so
    /// the entry names content already on disk; a removal's entry goes before the blobs it
    /// named are discarded. Either way the window in which a pointer names a missing blob
    /// does not exist (`D327`).
    ///
    /// Both halves are idempotent, because a roll-forward replays a transaction without
    /// knowing how far the interrupted apply got.
    pub(crate) fn apply_swap_to_namespace(
        &self,
        swap: &PreparedSwap,
        txn: &Nonce,
    ) -> Result<(), StoreError> {
        match swap.digest() {
            Some(digest) => self.write_body_pointer(swap.node(), swap.document(), digest, txn),
            None => self.remove_pointer(swap.node(), swap.document()),
        }
    }

    /// Removes the namespace entry for (`node`, `document`) from every replica (`D328`).
    ///
    /// **Idempotent, for the same reason a document removal is:** `D53` recovery rolls a
    /// prepared transaction forward by replaying it, so an entry that is already gone is
    /// success rather than an error -- otherwise a crash between `prepare` and `end` would
    /// leave a store that can never finish recovering.
    ///
    /// Nothing is staged. A pointer write stages so that no replica changes until every one
    /// is ready, which is what keeps a reader from seeing a half-written *value*; a removal
    /// has no value to half-write. What a reader sees mid-removal is the majority flipping
    /// from the old pointer to absent, which is the same transition it would see either way.
    ///
    /// The parent directory is fsynced so the removal's own dir-entry change is durable, and
    /// an absent parent is success -- it cannot hold the entry.
    pub(crate) fn remove_pointer(
        &self,
        node: &NodePath,
        document: &DocumentName,
    ) -> Result<(), StoreError> {
        let _modifying = self.begin_modification();
        for index in 0..self.genesis.pointer_replica_count {
            let entry = self.pointer_entry_path(index, node, document);
            // Already absent still syncs the parent: a replayed removal cannot tell an entry
            // that was never there from one whose removal was never made durable.
            match self.vfs.remove_file(&entry) {
                Ok(()) => {}
                Err(error) if is_absent(error.kind()) => {}
                Err(error) => return Err(StoreError::Vfs(error)),
            }
            if let Some(parent) = entry.parent() {
                match self.vfs.fsync(parent) {
                    Ok(()) => {}
                    Err(error) if is_absent(error.kind()) => {}
                    Err(error) => return Err(StoreError::Vfs(error)),
                }
            }
        }
        Ok(())
    }

    /// Reads the namespace entry for (`node`, `document`) by majority (`D320`).
    ///
    /// Every replica is read and parsed; the pointer a strict majority agree on is returned.
    /// Replicas are visited in the seeded rotation order `(h(P) + epoch) mod N` that the
    /// blob read uses (`D317`), so the same replica is not always the one met first. Because
    /// every replica is read and the winner is a strict majority, the rotation cannot change
    /// *which* pointer is returned -- only the order in which faults and rejections are
    /// encountered, and which replica bears the cost of being first.
    ///
    /// # What the majority buys, and what it does not
    ///
    /// It buys a **defined and monotonic** answer. Mid-apply the majority still holds the
    /// *old* pointer, so a read inside a write window returns the previous value -- the same
    /// value every time, never regressing between two reads. It is **not** atomic and must
    /// not be mistaken for a commit barrier: the value can be one the writer has already
    /// superseded on a minority of replicas.
    ///
    /// # Disagreement is staleness, not damage
    ///
    /// A replica that parses cleanly but differs from the majority is **behind**, not broken
    /// (`D320`). It is reported and never repaired: the transaction completing is what
    /// converges it, which recovery already drives. Repairing it would put the repair queue
    /// in the business of landing updates, which is the log's job.
    ///
    /// # Rejections name their cause and their remediation
    ///
    /// A replica that fails to parse is refused with the typed cause `CVS-10` produces, and
    /// logged together with what this read then did about it (`D326`) -- which is only
    /// knowable once every replica has been examined, so the logging happens after the vote
    /// rather than at the point of failure.
    /// The current namespace rotation epoch (`D317`).
    ///
    /// Advances once every `POINTER_EPOCH_READS` reads,
    /// and counted rather than timed for the same reason the blob epoch is: selection stays
    /// deterministic, so a test that forbids runtime randomness cannot have its layout
    /// decided by a clock.
    pub fn pointer_epoch(&self) -> u64 {
        self.pointer_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            / crate::store::POINTER_EPOCH_READS
    }

    pub fn read_pointer(
        &self,
        node: &NodePath,
        document: &DocumentName,
    ) -> Result<PointerRead, StoreError> {
        let replicas = self.genesis.pointer_replica_count;
        let expected = PathHash::of(node, document);

        let mut found: Vec<(usize, Pointer)> = Vec::with_capacity(replicas);
        let mut rejected: Vec<(usize, PointerError)> = Vec::new();
        let mut missing: Vec<usize> = Vec::new();

        // Seeded rotation, the same system the blob read uses (`D317`): the replica a read
        // touches first is `(h(P) + epoch) mod N` rather than always `p0`.
        let start = expected.rotation_seed().wrapping_add(self.pointer_epoch()) % replicas as u64;

        for step in 0..replicas {
            let index = (start as usize + step) % replicas;
            let entry = self.pointer_entry_path(index, node, document);
            let bytes = match self.vfs.read(&entry) {
                Ok(bytes) => bytes,
                Err(error) if is_absent(error.kind()) => {
                    missing.push(index);
                    continue;
                }
                Err(error) => return Err(StoreError::Vfs(error)),
            };
            match Pointer::parse(&bytes, expected) {
                Ok(pointer) => found.push((index, pointer)),
                Err(cause) => rejected.push((index, cause)),
            }
        }

        let majority = pointer_majority(replicas);
        let winner = found.iter().map(|(_, pointer)| *pointer).find(|candidate| {
            found.iter().filter(|(_, other)| other == candidate).count() >= majority
        });

        let Some(pointer) = winner else {
            self.report_pointer_rejections(node, document, &rejected, None);
            return Err(StoreError::PointerUnavailable {
                node: node.to_string(),
                document: document.as_str().to_string(),
                missing: missing.len(),
                rejected: rejected.len(),
                disagreeing: found.len(),
            });
        };

        self.report_pointer_rejections(node, document, &rejected, Some(&pointer));

        let agreed = found.iter().filter(|(_, other)| *other == pointer).count();
        let stale = found
            .iter()
            .filter(|(_, other)| *other != pointer)
            .map(|(index, _)| *index)
            .collect();

        Ok(PointerRead {
            pointer,
            agreed,
            replicas,
            stale,
            rejected: rejected.into_iter().map(|(index, _)| index).collect(),
            missing,
        })
    }

    /// Logs every refused replica with the cause it was refused for and what the read then
    /// did about it (`D326`).
    ///
    /// `served` is `None` when no majority could be formed, which is the distinction an
    /// operator most needs and the one a per-replica warning emitted at the point of failure
    /// cannot make: the same "rejected" line would appear whether the read recovered or not.
    fn report_pointer_rejections(
        &self,
        node: &NodePath,
        document: &DocumentName,
        rejected: &[(usize, PointerError)],
        served: Option<&Pointer>,
    ) {
        let remediation = match served {
            Some(_) => "served the majority held by the remaining replicas",
            None => "none: no majority could be formed, the read failed",
        };
        for (index, cause) in rejected {
            tracing::warn!(target: "canopy::namespace", event = "pointer_replica_rejected",
                root = ?self.root, partition = index, node = %node,
                document = document.as_str(), cause = %cause, remediation,
                "a namespace replica was refused");
        }
    }

    /// Lists the immediate children of `path` in the namespace tree: its documents and its
    /// child nodes, distinguished (`D222`, `CVS-12`).
    ///
    /// Semantics are those of the layout this replaces, unchanged in meaning: the **union**
    /// across replicas, so an entry present in at least one appears once even where others
    /// lack it; sorted output, so a walk is deterministic; reserved dotfiles and any entry
    /// that is not a well-formed canopy name skipped, so a foreign tree cannot derail the
    /// traversal.
    ///
    /// **Union rather than majority, deliberately, and this is the one place the namespace
    /// does not vote.** A vote answers "what does this name point at?", where a stale
    /// replica is a wrong answer that must not win. Traversal asks "what names exist?",
    /// where a replica that has not yet caught up holds an entry that is *real* -- it was
    /// written, or is being written -- and dropping it because it is not yet everywhere
    /// would hide a document from a caller enumerating the store. Presence is monotone in a
    /// way a value is not.
    ///
    /// A path that exists on no replica lists **empty rather than erroring**: a node is not
    /// a durable object in its own right, so "no children" and "no such node" are the same
    /// fact, and inventing a distinction would be inventing state the store does not keep.
    pub fn list_children(&self, path: impl Into<NodePath>) -> Result<Children, StoreError> {
        let path = path.into();
        let mut documents = BTreeSet::new();
        let mut nodes = BTreeSet::new();

        for index in 0..self.genesis.pointer_replica_count {
            let dir = namespace_partition_root(&self.root, index).join(path.relative_path());
            let entries = match self.vfs.read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) if is_absent(error.kind()) => continue,
                Err(error) => return Err(StoreError::Vfs(error)),
            };
            for entry in entries {
                if is_dotfile(&entry.file_name) {
                    continue;
                }
                match entry.file_type {
                    FileType::Dir => {
                        if let Some(component) = parse_component(&entry.file_name)
                            && let Ok(child) = path.child(&component)
                        {
                            nodes.insert(child);
                        }
                    }
                    FileType::File => {
                        if let Some(document) = parse_document_name(&entry.file_name) {
                            documents.insert(document);
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(Children {
            documents: documents.into_iter().collect(),
            nodes: nodes.into_iter().collect(),
        })
    }

    /// Every document the namespace holds, as sorted `(node, document)` pairs (`CVS-12`).
    ///
    /// Presence only: it names what exists without reading a pointer, let alone a body, so
    /// a caller can enumerate the store without paying for its contents. Resolving what any
    /// one name points at is [`read_pointer`](Store::read_pointer)'s job.
    ///
    /// Walks the tree from each root-level child downward rather than recursing blindly, so
    /// a cycle cannot arise and depth is bounded by what was actually written.
    pub fn list_documents(&self) -> Result<Vec<(NodePath, DocumentName)>, StoreError> {
        let mut found = BTreeSet::new();
        let mut pending: Vec<NodePath> = self.namespace_roots()?;

        while let Some(node) = pending.pop() {
            let children = self.list_children(node.clone())?;
            for document in children.documents() {
                found.insert((node.clone(), document.clone()));
            }
            pending.extend(children.nodes().iter().cloned());
        }

        Ok(found.into_iter().collect())
    }

    /// The root-level nodes, unioned across replicas.
    ///
    /// Separate from [`list_children`](Store::list_children) because a
    /// partition root is not itself a [`NodePath`] -- a path always has at least one
    /// component, so there is no value that names "the top".
    fn namespace_roots(&self) -> Result<Vec<NodePath>, StoreError> {
        let mut roots = BTreeSet::new();
        for index in 0..self.genesis.pointer_replica_count {
            let partition = namespace_partition_root(&self.root, index);
            let entries = match self.vfs.read_dir(&partition) {
                Ok(entries) => entries,
                Err(error) if error.kind() == VfsErrorKind::NotFound => continue,
                Err(error) => return Err(StoreError::Vfs(error)),
            };
            for entry in entries {
                if is_dotfile(&entry.file_name) || entry.file_type != FileType::Dir {
                    continue;
                }
                if let Some(component) = parse_component(&entry.file_name)
                    && let Ok(node) = NodePath::new([component])
                {
                    roots.insert(node);
                }
            }
        }
        Ok(roots.into_iter().collect())
    }

    /// Refuses a pointer write that would make one name both a document and a child node
    /// (`D222`).
    ///
    /// Carried across unchanged in meaning from the layout this replaces. The `Vfs` would
    /// fail these cases anyway -- a file over a directory, a directory under a file -- but
    /// as a generic I/O error that says nothing about what the caller did wrong. A caller
    /// who has collided with the other namespace has made a modelling mistake, not hit a
    /// disk problem, and the error should say so.
    pub(super) fn check_namespace_tree_collision(
        &self,
        index: usize,
        path: &NodePath,
        document: &DocumentName,
    ) -> Result<(), StoreError> {
        let Some(node_dir) = self.check_node_path_is_not_document(index, path)? else {
            return Ok(());
        };
        if self.probe_namespace(&node_dir.join(document.as_str()))? == Some(FileType::Dir) {
            return Err(StoreError::NameIsChildNode {
                path: path.clone(),
                name: document.as_str().to_owned(),
            });
        }
        Ok(())
    }

    /// Refuses `path` as a node path in partition `index` if it, or any prefix of it, is a
    /// document, reporting the shallowest such prefix. Returns the node's directory if it
    /// exists, and `None` if some prefix is absent, since nothing deeper can exist then.
    pub(super) fn check_node_path_is_not_document(
        &self,
        index: usize,
        path: &NodePath,
    ) -> Result<Option<PathBuf>, StoreError> {
        // Outermost-first, so the *shallowest* conflict is reported: a deeper one would
        // name a directory that only failed because of this one.
        let mut ancestor = namespace_partition_root(&self.root, index);
        for (depth, component) in path.components().enumerate() {
            ancestor.push(component);
            match self.probe_namespace(&ancestor)? {
                Some(FileType::File) => {
                    let prefix = NodePath::new(path.components().take(depth + 1))
                        .expect("a prefix of a valid path is a valid path");
                    return Err(StoreError::PathIsDocument { path: prefix });
                }
                None => return Ok(None),
                Some(_) => {}
            }
        }
        Ok(Some(ancestor))
    }

    /// The type of what is at `path`, or `None` if nothing is.
    ///
    /// Only absence counts as "no collision". Any other failure is propagated, because a
    /// collision hidden by a transient error would reach `prepare` and wedge recovery.
    fn probe_namespace(&self, path: &Path) -> Result<Option<FileType>, StoreError> {
        match self.vfs.metadata(path) {
            Ok(metadata) => Ok(Some(metadata.file_type)),
            Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(None),
            Err(error) => Err(StoreError::Vfs(error)),
        }
    }

    /// Makes the namespace entry for (`node`, `document`) durable as it stands, present or
    /// absent, on every replica: syncs the nearest existing directory above the entry, which
    /// is the one whose listing last changed.
    pub(crate) fn sync_pointer_entry(
        &self,
        node: &NodePath,
        document: &DocumentName,
    ) -> Result<(), StoreError> {
        for index in 0..self.genesis.pointer_replica_count {
            let partition = namespace_partition_root(&self.root, index);
            let entry = self.pointer_entry_path(index, node, document);
            let mut dir = entry.parent();
            while let Some(current) = dir {
                match self.vfs.fsync(current) {
                    Ok(()) => break,
                    Err(error) if is_absent(error.kind()) && current != partition => {
                        dir = current.parent();
                    }
                    Err(error) => return Err(StoreError::Vfs(error)),
                }
            }
        }
        Ok(())
    }

    /// Partition `index`'s namespace entry for (`node`, `document`).
    pub(super) fn pointer_entry_path(
        &self,
        index: usize,
        node: &NodePath,
        document: &DocumentName,
    ) -> PathBuf {
        namespace_partition_root(&self.root, index)
            .join(node.relative_path())
            .join(document.as_str())
    }

    /// Partition `index`'s staging temp for a pointer write under `nonce`.
    pub(super) fn pointer_staging_temp(
        &self,
        index: usize,
        document: &DocumentName,
        nonce: &Nonce,
    ) -> PathBuf {
        namespace_partition_root(&self.root, index)
            .join(STAGING_DIR)
            .join(nonce.path_component().as_ref())
            .join(temp_name(document, nonce))
    }

    /// Discards the staging a pointer write left behind.
    ///
    /// Best-effort: on the success path the temps have been renamed away, and on the failure
    /// path what remains is an orphan a recovery sweep collects. Failing a write that has
    /// already committed every replica because tidying afterwards did not work would turn a
    /// cleanup problem into a data problem.
    pub(super) fn sweep_pointer_staging(
        &self,
        document: &DocumentName,
        nonce: &Nonce,
        replicas: usize,
    ) {
        for index in 0..replicas {
            let temp = self.pointer_staging_temp(index, document, nonce);
            let _ = self.vfs.remove_file(&temp);
            if let Some(parent) = temp.parent() {
                let _ = self.vfs.remove_dir(parent);
            }
        }
    }
}

/// The path of namespace partition `index` under `root`: `<root>/namespace/p{index}`.
/// How many replicas must agree for the namespace to have decided (`D320`).
///
/// One definition, because two readers of the same vote must not disagree about what a
/// majority is: [`Store::read_pointer`] applies it to the replicas holding a pointer, and
/// [`Store::read_document`] applies it to the replicas holding *nothing*, which is the
/// candidate a create window turns on.
pub(crate) const fn pointer_majority(replicas: usize) -> usize {
    replicas / 2 + 1
}

pub(crate) fn namespace_partition_root(root: &Path, index: usize) -> PathBuf {
    root.join(NAMESPACE_TREE_DIR)
        .join(format!("{PARTITION_PREFIX}{index}"))
}

/// What a successful [`Store::read_pointer`] found (`D320`).
///
/// Carries the dissenting replicas as well as the winner, because the two kinds of dissent
/// want opposite treatment: a **rejected** replica failed verification and is a repair's
/// business, while a **stale** one is merely behind and must never be repaired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerRead {
    pointer: Pointer,
    agreed: usize,
    replicas: usize,
    stale: Vec<usize>,
    rejected: Vec<usize>,
    missing: Vec<usize>,
}

impl PointerRead {
    /// The pointer the majority agreed on.
    pub fn pointer(&self) -> Pointer {
        self.pointer
    }

    /// How many replicas held it.
    pub fn agreed(&self) -> usize {
        self.agreed
    }

    /// How many replicas the namespace keeps.
    pub fn replicas(&self) -> usize {
        self.replicas
    }

    /// Replicas holding a valid pointer that is **not** the majority's -- behind, not
    /// broken, and never repaired (`D320`).
    pub fn stale(&self) -> &[usize] {
        &self.stale
    }

    /// Replicas whose entry could not be parsed.
    pub fn rejected(&self) -> &[usize] {
        &self.rejected
    }

    /// Replicas with no entry at all.
    pub fn missing(&self) -> &[usize] {
        &self.missing
    }

    /// Whether every replica agreed -- no staleness, nothing refused, nothing absent.
    pub fn is_unanimous(&self) -> bool {
        self.agreed == self.replicas
    }
}
