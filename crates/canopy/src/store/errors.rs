// Copyright (c) Michael Grier.
// Split from store.rs at 1303f9a.

//! The store's error vocabulary and its conversions from the layers below.

use super::*;

/// A failure from a [`Store`] layout operation ([`Store::init`] / [`Store::open`]).
#[derive(Debug)]
pub enum StoreError {
    /// An underlying filesystem error from the [`Vfs`] seam.
    Vfs(VfsError),
    /// The genesis marker could not be serialized or was not valid genesis JSON.
    Json(serde_json::Error),
    /// `init` was asked to create a store under a root that does not exist.
    RootNotFound {
        /// The root that was missing.
        root: PathBuf,
    },
    /// `init` was pointed at a root path that is not a directory.
    RootNotADirectory {
        /// The offending path.
        root: PathBuf,
    },
    /// `init` refused a root that already holds a non-ignorable (non-dotfile)
    /// entry, so creating a store there would risk clobbering existing data.
    NotEmpty {
        /// The root that was not empty.
        root: PathBuf,
        /// The first non-dotfile entry found.
        entry: OsString,
    },
    /// `init` refused a root that already contains a [`.shire`](MARKER_NAME)
    /// marker -- it is already a store.
    AlreadyInitialized {
        /// The root that already held a marker.
        root: PathBuf,
    },
    /// A [`ShireId`] was built from the empty string.
    EmptyShireId,
    /// A blob replica count that is not positive and no greater than
    /// [`MAX_REPLICA_COUNT`] was requested (`D316`, `D320`).
    ///
    /// Separate from [`InvalidPointerReplicaCount`](StoreError::InvalidPointerReplicaCount)
    /// because the two counts are validated against different rules, and an error that named
    /// only "a replica count" would leave the caller to work out which rule it broke.
    InvalidBlobReplicaCount {
        /// The rejected count.
        count: usize,
    },
    /// A pointer replica count that is not a positive **odd** number no greater than
    /// [`MAX_REPLICA_COUNT`] was requested (`D320`).
    InvalidPointerReplicaCount {
        /// The rejected count.
        count: usize,
    },
    /// A shard width outside `1..=`[`MAX_SHARD_WIDTH`] was requested (`D322`, `D324`).
    InvalidShardWidth {
        /// The rejected width, in leading characters of the rendered path hash.
        width: usize,
    },
    /// The caller's stream failed while a blob was being ingested (`D315`, `CVS-6`).
    ///
    /// Distinct from [`Vfs`](StoreError::Vfs): the store's own storage is fine and nothing
    /// committed has changed. The bytes stopped arriving, which is the caller's side of the
    /// boundary, and blaming the filesystem for it would send an operator to the wrong
    /// place entirely.
    IngestSource {
        /// What the source reported.
        error: std::io::Error,
    },
    /// No replica of a blob could be read and verified (`D316`, `CVS-7`).
    ///
    /// The two counts are reported separately because they are different failures with
    /// different remedies: every copy **missing** means the blob was never written or has
    /// been removed, while every copy **corrupt** means they were written and have since
    /// rotted. Collapsing them into "unavailable" would hide which one happened at the
    /// moment an operator most needs to know.
    BlobUnavailable {
        /// The blob's partition-relative path.
        blob: String,
        /// How many replicas had no copy.
        missing: usize,
        /// How many replicas held a copy that did not hash to its own name.
        corrupt: usize,
    },
    /// A document read kept being contended by writers and gave up (`D328`, `D338`).
    ///
    /// Either the namespace kept moving out from under the read (the version it resolved was
    /// collected before the content could be fetched) or it kept failing to settle at all
    /// (replicas mid-commit, no majority either way). The caller's loop decides how long to
    /// tolerate that; this is what it raises when it stops. Nothing is damaged and a later
    /// read will very likely succeed.
    ///
    /// It says the writer never paused for as long as the caller was willing to wait -- so a
    /// caller seeing it repeatedly should ask whether its budget is too small or its writer
    /// too busy, not whether the store is broken.
    DocumentOvertaken {
        /// The node holding the document.
        node: String,
        /// The document that kept moving.
        document: String,
        /// How many resolutions were tried.
        attempts: usize,
    },
    /// No majority of namespace replicas agreed on a pointer (`D320`, `CVS-11`).
    ///
    /// The three counts are the three ways a replica can fail to contribute a vote, and are
    /// reported apart because they point at different things: **missing** is a document that
    /// may never have existed, **rejected** is damage, and **disagreeing** means the copies
    /// parsed but were too scattered to form a majority -- which for a pointer is a far
    /// stranger condition than either, since an apply moves replicas one at a time and a
    /// majority survives every intermediate state.
    PointerUnavailable {
        /// The node the document sits under.
        node: String,
        /// The document whose entry could not be determined.
        document: String,
        /// How many replicas had no entry.
        missing: usize,
        /// How many replicas held an entry that would not parse.
        rejected: usize,
        /// How many replicas held a valid entry that was not part of any majority.
        disagreeing: usize,
    },
    /// `open` found no [`.shire`](MARKER_NAME) marker at the root: it is not a
    /// store.
    NotAShire {
        /// The root that carried no marker.
        root: PathBuf,
    },
    /// Another process already holds this root open for writing (`D116`).
    ///
    /// Named rather than folded into [`StoreError::Vfs`] because it is the one
    /// failure here a caller can act on without a code change: nothing is
    /// corrupt or misconfigured, and the usual cause is a second daemon pointed
    /// at the same Shire.
    RootInUse {
        /// The root that is already held.
        root: PathBuf,
    },
    /// `open` read a marker written by a genesis version this build does not
    /// understand.
    UnsupportedFormatVersion {
        /// The version found in the marker.
        found: u32,
        /// The version this build writes and understands ([`GENESIS_FORMAT_VERSION`]).
        expected: u32,
    },
    /// `open` read a marker written by the genesis version [`Store::migrate`] advances, so the
    /// root must be migrated before this build opens it.
    MigrationRequired {
        /// The store root.
        root: PathBuf,
        /// The version found in the marker ([`MIGRATABLE_FORMAT_VERSION`]).
        found: u32,
        /// The version this build writes and understands ([`GENESIS_FORMAT_VERSION`]).
        expected: u32,
    },
    /// The publication record held a complete line that is not a record this store writes, so
    /// the publication it may name cannot be finished.
    MalformedPublicationRecord {
        /// The store root.
        root: PathBuf,
        /// What was wrong with the line.
        detail: String,
    },
    /// `open` read a marker whose recorded layout is internally inconsistent or
    /// disagrees with what is on disk.
    MalformedMarker {
        /// The store root.
        root: PathBuf,
        /// A short description of the inconsistency.
        reason: &'static str,
    },
    /// `open` found the marker but a namespace replica it names is absent.
    MissingPartition {
        /// The store root.
        root: PathBuf,
        /// The index of the missing partition.
        index: usize,
    },
    /// A commit-log operation failed while applying a transaction (D53/TXN-2).
    CommitLog(CommitLogError),
    /// A write targeted a name that is already a child node (`D222`).
    ///
    /// A name at a path is either a document or a child node, never both, so writing
    /// a document where a subtree already lives is a modelling mistake rather than a
    /// disk problem, and is named as one.
    NameIsChildNode {
        /// The node path the write targeted.
        path: NodePath,
        /// The name that is already a child node.
        name: String,
    },
    /// A write's node path passes through a name that is already a document (`D222`).
    ///
    /// The reciprocal of [`NameIsChildNode`](StoreError::NameIsChildNode): a document
    /// cannot also be a directory, so no path may descend through one. The reported
    /// path is the shallowest offending prefix, which is the one a caller can act on.
    PathIsDocument {
        /// The prefix of the write's path that is a document.
        path: NodePath,
    },
    /// A repair named a blob replica index the store does not have.
    ///
    /// Valid indices are `0..count`, where `count` is the genesis blob replica count.
    ReplicaIndexOutOfRange {
        /// The rejected index.
        index: usize,
        /// How many blob replicas the store keeps.
        count: usize,
    },
    /// A repair named a blob at another shard width than the store's, a path the store never
    /// writes into.
    ReferenceShardWidth {
        /// The store's shard width.
        expected: usize,
        /// The reference's shard width.
        found: usize,
    },
    /// An ingest's copy of its own staged replica did not hash to the digest computed while
    /// staging it, so the staged bytes changed underneath the ingest. Nothing was committed.
    StagedReplicaMismatch {
        /// The staged file whose contents changed.
        staged: PathBuf,
    },
    /// A transaction through some handle to this store may have passed its commit point
    /// without completing, so every handle refuses writes until the store is reopened and
    /// recovery has finished it.
    RecoveryRequired {
        /// The store root.
        root: PathBuf,
    },
    /// A pointer handed to a write does not belong to the document it would be filed under.
    InvalidPointer(PointerError),
    /// A verb that mints its own nonce was called on a store opened without a nonce source.
    NoNonceSource,
    /// A snapshot's read was disturbed by a modification on every attempt it was allowed.
    SnapshotContended {
        /// How many times the read ran.
        attempts: usize,
    },
    /// An operation's nonce is already claimed -- by an operation in flight, or one that left
    /// its claim behind -- so the operation was refused before creating anything named by it.
    NonceCollision {
        /// The colliding nonce.
        nonce: Nonce,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Vfs(e) => write!(f, "store filesystem error: {e}"),
            StoreError::Json(e) => write!(f, "store genesis marker is not valid JSON: {e}"),
            StoreError::RootNotFound { root } => {
                write!(
                    f,
                    "cannot initialize a store: {} does not exist",
                    root.display()
                )
            }
            StoreError::RootNotADirectory { root } => {
                write!(
                    f,
                    "cannot initialize a store: {} is not a directory",
                    root.display()
                )
            }
            StoreError::NotEmpty { root, entry } => write!(
                f,
                "cannot initialize a store: {} is not empty (found {})",
                root.display(),
                entry.to_string_lossy()
            ),
            StoreError::AlreadyInitialized { root } => write!(
                f,
                "cannot initialize a store: {} is already a Shire",
                root.display()
            ),
            StoreError::EmptyShireId => write!(f, "a Shire identity must not be empty"),
            StoreError::InvalidBlobReplicaCount { count } => {
                write!(
                    f,
                    "blob replica count {count} is not a positive number at most {MAX_REPLICA_COUNT}"
                )
            }
            StoreError::InvalidPointerReplicaCount { count } => {
                write!(
                    f,
                    "pointer replica count {count} is not a positive odd number at most {MAX_REPLICA_COUNT}"
                )
            }
            StoreError::InvalidShardWidth { width } => {
                write!(
                    f,
                    "shard width {width} is not between 1 and {MAX_SHARD_WIDTH} characters"
                )
            }
            StoreError::IngestSource { error } => {
                write!(f, "the ingest source failed while being read: {error}")
            }
            StoreError::BlobUnavailable {
                blob,
                missing,
                corrupt,
            } => write!(
                f,
                "no replica of {blob} could be verified ({missing} missing, {corrupt} corrupt)"
            ),
            StoreError::DocumentOvertaken {
                node,
                document,
                attempts,
            } => write!(
                f,
                "{document} under {node} was contended by a writer during each of {attempts} read attempts"
            ),
            StoreError::PointerUnavailable {
                node,
                document,
                missing,
                rejected,
                disagreeing,
            } => write!(
                f,
                "no majority names {document} under {node} \
                 ({missing} missing, {rejected} rejected, {disagreeing} disagreeing)"
            ),
            StoreError::NotAShire { root } => {
                write!(
                    f,
                    "{} is not a Shire (no {MARKER_NAME} marker)",
                    root.display()
                )
            }
            StoreError::RootInUse { root } => {
                write!(
                    f,
                    "{} is already open for writing by another process",
                    root.display()
                )
            }
            StoreError::UnsupportedFormatVersion { found, expected } => write!(
                f,
                "store genesis version {found} is unsupported (this build understands {expected})"
            ),
            StoreError::MigrationRequired {
                root,
                found,
                expected,
            } => write!(
                f,
                "store at {} is at genesis version {found} and must be migrated to {expected} \
                 with Store::migrate before it is opened",
                root.display()
            ),
            StoreError::MalformedPublicationRecord { root, detail } => write!(
                f,
                "store at {} has a publication record this build did not write: {detail}",
                root.display()
            ),
            StoreError::MalformedMarker { root, reason } => {
                write!(
                    f,
                    "store at {} has a malformed marker: {reason}",
                    root.display()
                )
            }
            StoreError::MissingPartition { root, index } => write!(
                f,
                "store at {} is missing partition p{index}",
                root.display()
            ),
            StoreError::CommitLog(e) => write!(f, "store commit-log error: {e}"),
            StoreError::NameIsChildNode { path, name } => write!(
                f,
                "cannot write document {name} at {path}: that name is already a child node"
            ),
            StoreError::PathIsDocument { path } => write!(
                f,
                "cannot use {path} as a node path: it is already a document"
            ),
            StoreError::ReplicaIndexOutOfRange { index, count } => write!(
                f,
                "blob replica index {index} is out of range (the store keeps {count})"
            ),
            StoreError::ReferenceShardWidth { expected, found } => write!(
                f,
                "blob reference has shard width {found}, but the store uses {expected}"
            ),
            StoreError::InvalidPointer(e) => write!(f, "refused to write a pointer: {e}"),
            StoreError::RecoveryRequired { root } => write!(
                f,
                "store at {} has an unfinished transaction; reopen it to recover",
                root.display()
            ),
            StoreError::StagedReplicaMismatch { staged } => write!(
                f,
                "staged blob {} changed while it was being replicated",
                staged.display()
            ),
            StoreError::NoNonceSource => {
                f.write_str("the store was opened without a nonce source, so it cannot mint one")
            }
            StoreError::SnapshotContended { attempts } => write!(
                f,
                "a snapshot was disturbed by a concurrent modification on all {attempts} attempts"
            ),
            StoreError::NonceCollision { nonce } => {
                write!(f, "nonce {} is already claimed", nonce.as_str())
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Vfs(e) => Some(e),
            StoreError::Json(e) => Some(e),
            StoreError::CommitLog(e) => Some(e),
            StoreError::InvalidPointer(e) => Some(e),
            _ => None,
        }
    }
}

impl From<VfsError> for StoreError {
    fn from(e: VfsError) -> Self {
        StoreError::Vfs(e)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Json(e)
    }
}

impl From<CommitLogError> for StoreError {
    fn from(e: CommitLogError) -> Self {
        StoreError::CommitLog(e)
    }
}
