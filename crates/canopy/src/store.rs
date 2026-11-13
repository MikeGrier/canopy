// Copyright (c) Michael Grier.

//! The on-disk store layout: the `.shire` marker over the content and namespace
//! trees (`D314`, LAY-1).
//!
//! A Canopy store lives under a *Shire root* directory whose top level is a
//! [`.shire`](MARKER_NAME) marker file plus `files/` (content-verified blobs, replicated
//! `blob_replica_count` ways) and `namespace/` (self-validating pointers, replicated an
//! odd `pointer_replica_count` ways). canopy stores opaque documents keyed by
//! caller-supplied node names; it has no ticket tables and no `tickets/` subtree (D54) --
//! the well-known singletons (retention, allocation counter, key) are just documents
//! the caller places at well-known paths.
//!
//! The marker carries the store's own *genesis* configuration -- the physical
//! layout parameters canopy needs to reopen the store deterministically (format
//! version, the two replica counts, shard width) plus a caller-supplied identity and
//! creation time. Its presence is the authoritative "this is a Shire" signal:
//! [`Store::init`] creates the layout and [`Store::open`] refuses a root
//! whose marker is absent or malformed, so the two stay distinct and Canopy
//! never scribbles documents into a directory pointed at by mistake.
//!
//! Runtime-variable values (the identity and the timestamp) are supplied by the caller;
//! canopy therefore needs no clock or RNG dependency.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use acadia::{FileType, Vfs, VfsError, VfsErrorKind, VfsLock};
use serde::{Deserialize, Serialize};

use crate::body::{ContentDigest, DocumentBody};
use crate::commit_log::{CommitLog, CommitLogError, LogRecord, PreparedSet, PreparedSwap};
use crate::name::{DocumentName, NodePath, Nonce};
use crate::naming::BlobRef;
use crate::pointer::PointerError;
use crate::transaction::{Precondition, Swap, SwapSet};

mod apply;
mod blob;
mod document;
mod errors;
mod genesis;
mod namespace;
mod publication;
mod repair;
mod scrub;
mod surface;
mod writer;

pub use genesis::Migration;
pub use publication::PUBLICATION_RECORD_NAME;
use publication::PublicationDurability;
use repair::RepairBacklog;
pub use repair::{BlobVerification, QUARANTINE_LIMIT, REPAIR_BACKLOG_LIMIT, RepairOutcome};
pub use scrub::BacklogRepairReport;
pub use scrub::ContentScrubReport;
use writer::{WriterGate, WriterGuard};
#[cfg(test)]
mod tests;

pub use self::apply::*;
pub use self::blob::BlobRead;
pub use self::errors::*;
pub use self::namespace::PointerRead;
#[cfg(test)]
pub(crate) use self::surface::CLAIMS_DIR;
pub use self::surface::{Commit, SNAPSHOT_ATTEMPTS, StoreOptions, StoreView};

/// The marker file name at the Shire root (D40).
///
/// Its presence is the authoritative "this is a Shire" signal. The leading dot
/// keeps it out of the node namespace: a node name never begins with `.`, so the
/// marker can never collide with a shard directory.
pub const MARKER_NAME: &str = ".shire";

/// The name of the root's exclusive-writer lock file (`D116`).
///
/// A dotfile for the same reason [`MARKER_NAME`] is: a node name never begins
/// with `.`, so it cannot collide with a shard directory, and `init`'s
/// empty-root check already ignores dotfiles.
///
/// **Its presence means nothing.** The file is left behind when the lock is
/// released and is never reaped; only a live holder excludes anyone. Anything
/// that treats this file as evidence of a running process is wrong.
pub const LOCK_NAME: &str = ".lock";

/// The genesis schema version.
///
/// Bump on any breaking change to the on-disk genesis shape. Per the repo's
/// no-magic-constants rule, changing this assigned value is itself a breaking
/// change. It is independent of any format version the caller keeps inside its opaque
/// document bodies.
///
/// v2 (`D222`) made the namespace hierarchical and dropped the `shard_depth` field: a
/// v1 store's directory layout carries a shard tier this build would misread as an
/// ordinary path component, so a v1 store is refused rather than reinterpreted.
///
/// v3 (`D314`..`D322`, `D324`) records the content-verified layout's parameters: separate
/// replica counts for blobs and pointers, and a shard width. A v2 store is refused for the
/// reason `D318` gives -- nothing is migrated, because nothing yet stored is worth
/// migrating. Refusal is decided from the version alone, before the rest of the marker is
/// interpreted (`D325`).
///
/// v4 (`D327`) changed the commit log's `prepare` record from carrying document bodies to
/// carrying their digests. The marker does not describe the log, so this bump may look
/// misplaced -- it is not. The version is the store's *one* refusal gate, and it is read
/// before anything else (`D325`); the log has no version of its own, and giving it one
/// would mean a second gate that can disagree with the first. A v3 `prepare` would
/// otherwise deserialize into a write with no digest -- structurally valid, silently
/// wrong -- so the store carrying it is refused instead.
///
/// v5 (`CVS-19.4`) retired the partition tree and dropped the `partition_count` field. A v4
/// marker's natural reading would be a lie: it names a replication width for a tree this
/// build does not create, read, or repair, so a v4 store would open and then behave as if
/// its `p{i}` copies were still part of the store's truth. Refusing it is the honest answer,
/// and is decided from the version alone (`D325`).
///
/// v6 added the publication record ([`PUBLICATION_RECORD_NAME`]) that guards an unlogged write
/// against `STRANDED-VOTE`. A v5 binary would ignore a pending record and could strand the very
/// vote it guards, so the version is the barrier that keeps one out of a v6 root. A v5 root holds
/// nothing a v6 build reads differently, so it is not refused but migrated: `open` reports
/// [`StoreError::MigrationRequired`] and [`Store::migrate`] advances it.
pub const GENESIS_FORMAT_VERSION: u32 = 6;

/// The genesis version [`Store::migrate`] advances to [`GENESIS_FORMAT_VERSION`]. Changing this
/// value changes which roots are migrated rather than refused, so it is a
/// compatibility-affecting constant.
pub const MIGRATABLE_FORMAT_VERSION: u32 = 5;

/// The largest replica count canopy accepts -- a safety ceiling (`D79`).
///
/// A realistic store uses a single-digit count; this generous ceiling exists only to bound
/// the store paths that scale with a replica count -- every `0..count` loop and each
/// `Vec::with_capacity(count)` -- so a malformed or tampered genesis marker, or an extreme
/// caller-supplied count, surfaces a typed error at config time or
/// [`StoreError::MalformedMarker`] at open time instead of driving an unbounded allocation.
/// Changing this value changes which markers `open` accepts, so it is a
/// compatibility-affecting constant.
pub const MAX_REPLICA_COUNT: usize = 4095;

/// The default number of replicas of each blob (`D316`, `D320`).
pub const DEFAULT_BLOB_REPLICA_COUNT: usize = 3;

/// The default number of replicas of each namespace pointer (`D320`).
pub const DEFAULT_POINTER_REPLICA_COUNT: usize = 3;

/// The default shard width, in leading characters of the rendered path hash (`D322`).
///
/// Two characters is 32^2 = 1024 shard directories. Shard directories are created lazily,
/// so over-sharding costs nothing, while under-sharding is a format break to correct.
pub const DEFAULT_SHARD_WIDTH: usize = 2;

/// The largest shard width canopy accepts (`D322`, `D324`).
///
/// Four characters is 32^4 = 1,048,576 shard directories, past any plausible per-store
/// document count. The ceiling is not only about pathology: every shard character is taken
/// directly out of the path budget `D322` computes, so a wide shard buys nothing and costs
/// root headroom. A width of zero is rejected as well -- it would mean a blob path with no
/// shard component at all, which is a second layout shape rather than a smaller one.
pub const MAX_SHARD_WIDTH: usize = 4;

/// The prefix of a partition directory name: partition `i` is `p{i}` (D40).
const PARTITION_PREFIX: &str = "p";

/// The blob tree: `<root>/files/p{k}/<shard>/<name>` (`D314`).
///
/// One of the three parts a Shire root holds, each with its own replication partitions and
/// its own lifecycle. Not a dotfile, because it is a peer of the namespace rather than
/// something hidden from it. Changing this value is a breaking change to the on-disk layout.
pub(crate) const BLOB_TREE_DIR: &str = "files";

/// The namespace tree: `<root>/namespace/p{k}/<node components>/<document>` (`D314`).
///
/// The client-visible tree, laid out by path so a traversal walks directories rather than
/// consulting an index. Its partition count is independent of the blob tree's, and is odd,
/// because pointer reads vote (`D320`). Changing this value is a breaking change to the
/// on-disk layout.
pub(crate) const NAMESPACE_TREE_DIR: &str = "namespace";

/// The directory inside a blob or namespace partition holding corrupt copies moved aside by
/// a repair (`D326`, `D331`): `<tree>/p{i}/.quarantine/`.
///
/// A dotfile for the same reason [`STAGING_DIR`] is: a node name never begins with `.`, so it
/// cannot collide with a shard or a node directory, and every traversal already skips it.
///
/// **Per partition, deliberately.** The question a quarantine answers is whether corruption
/// clusters on one replica -- failing hardware -- or is spread evenly -- a bug in our own
/// write path. Keeping each partition's bad copies under that partition turns that question
/// into a directory listing.
pub(crate) const QUARANTINE_DIR: &str = ".quarantine";

/// The file at the store root recording directories about to be created, one root-relative
/// path per line, so a reopen syncs the name of any whose creation was not made durable.
/// Changing this value is a breaking change to the on-disk layout.
pub(crate) const DIR_SYNC_DEBT_NAME: &str = ".dir-sync-debt";

/// The modification count (`D335`) and how deeply the current writer has nested
/// modifications inside one another.
///
/// Only the outermost modification moves the count, so a logical operation built from several
/// -- a transaction's swaps, a write's ingest, publication and collection -- reads as one odd
/// span to a sampler rather than as a run of settled moments between its steps. The depth is
/// touched only by the holder of the writer gate, one thread at a time.
#[derive(Debug, Default)]
pub(crate) struct ModificationCount {
    count: std::sync::atomic::AtomicU64,
    depth: std::sync::atomic::AtomicUsize,
}

/// Ends a modification begun by `Store::begin_modification`, however the modification ends.
#[must_use]
pub(crate) struct ModificationGuard {
    modifications: Arc<ModificationCount>,
    writer: Arc<WriterGate>,
}

impl Drop for ModificationGuard {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        let outermost = self.modifications.depth.fetch_sub(1, Ordering::AcqRel) == 1;
        // A store whose transaction ended unresolved keeps the count odd, so no reader treats
        // the partial state it left as settled before recovery has finished it.
        if outermost && !self.writer.is_unresolved() {
            self.modifications.count.fetch_add(1, Ordering::Release);
        }
    }
}

/// Whether a failure to reach a path means only that nothing is there.
///
/// A path beneath a file is as absent as a path beneath nothing: a document written where a
/// subtree used to be makes every old path below it unreachable for good. Windows reports
/// that as not found and POSIX as not a directory, so both are absence, and every other
/// failure is a real error.
pub(crate) fn is_absent(kind: VfsErrorKind) -> bool {
    matches!(kind, VfsErrorKind::NotFound | VfsErrorKind::NotADirectory)
}

/// Directories owed a parent sync, and whether the record file's own name is known durable.
#[derive(Debug, Default)]
struct DirSyncDebt {
    dirs: Vec<PathBuf>,
    name_synced: bool,
    /// An append to the record failed and may have left an unterminated prefix, which the
    /// next append would otherwise extend into a different, wrong path.
    torn: bool,
}

/// How many blob reads advance the rotation epoch by one (`D317`).
///
/// Deliberately coarse. Rotation exists to sweep every replica over hours so a rotted copy
/// is found early; it does not need to vary per read, and varying per read is actively
/// harmful -- two reads of one document inside a write window could land on different
/// replicas and observe the value going backwards, and the cached working set multiplies by
/// the replica count.
///
/// Not a format constant: it decides which copy is preferred, never where a blob lives, so
/// changing it is invisible on disk.
pub(crate) const BLOB_EPOCH_READS: u64 = 4096;

/// How many pointer reads advance the namespace rotation epoch by one (`D317`).
///
/// The pointer read visits **every** replica, because a pointer is validated against its
/// own path hash rather than a content digest and that cannot tell a stale copy from a
/// current one -- so rotation does not choose a winner here the way it does for a blob, and
/// the majority is unaffected by the order. What it does decide is which replica is touched
/// *first*, and that is worth rotating for the same reason `D317` gives: a fixed order
/// always meets the same replica first, so a slow device or a hard I/O fault on it is met
/// on every read, and the order rejections are discovered in never varies.
///
/// Counted separately from blob reads: a document read takes one of each, so a shared
/// counter would advance twice per read and tie the two trees' sweeps together for no
/// reason.
pub(crate) const POINTER_EPOCH_READS: u64 = 4096;

/// The reserved per-partition staging directory name (D40/D53, LAY-3).
///
/// A durable write builds its new body under `p{i}/.staging/<nonce>/` and then
/// atomically renames it into the committed node path, so a committed copy is
/// never overwritten in place. The leading `.` keeps staging out of the node
/// namespace (a path component never begins with `.`), so a namespace traversal
/// skips it like the `.shire` marker, and its distinctive name lets crash-recovery
/// sweep the orphaned staging of an interrupted write. Changing this value is a
/// breaking change to the on-disk staging convention.
pub(crate) const STAGING_DIR: &str = ".staging";

/// The infix separating a document name from the staging nonce in a temp file
/// name: `<document>.tmp.<nonce>` (D40 update protocol, LAY-3).
///
/// The distinctive `.tmp.` lets crash-recovery recognize a staging temp. Changing
/// this value is a breaking change to the on-disk staging convention.
const TEMP_INFIX: &str = ".tmp.";

/// The staging temp file name for `document` under write `nonce`:
/// `<document>.tmp.<nonce>` (D40 update protocol, LAY-3).
fn temp_name(document: &DocumentName, nonce: &Nonce) -> String {
    format!("{}{TEMP_INFIX}{}", document.as_str(), nonce.as_str())
}

/// Whether a directory entry name is one of canopy's own dotfiles (the `.shire` marker
/// mirror or the `.staging` directory), which a namespace traversal skips.
///
/// A [`NodePath`] component never begins with `.`, so a leading dot unambiguously
/// marks a canopy-reserved entry rather than a node.
fn is_dotfile(name: &OsString) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// Reconstructs the path component an on-disk directory carries, or `None` if the
/// entry is not a well-formed component (foreign content to be skipped).
///
/// The directory name is canopy's canonical (uppercased) form, so re-validation
/// round-trips for any entry canopy wrote; a name that fails to parse was not placed by
/// canopy and is not part of the namespace.
fn parse_component(name: &OsString) -> Option<String> {
    let raw = name.to_str()?;
    NodePath::parse(raw).ok().map(|path| path.name().to_owned())
}

/// Reconstructs the [`DocumentName`] an on-disk document file carries, or `None` if the
/// entry is not a well-formed document name (foreign content to be skipped).
fn parse_document_name(name: &OsString) -> Option<DocumentName> {
    DocumentName::new(name.to_str()?).ok()
}
/// One transaction's records collected from the commit log during recovery (TXN-4).
///
/// The three phase records for a transaction id are reduced to the two facts recovery
/// needs: whether a `prepare` was written (and its swap set, for roll-forward) and
/// whether an `end` closed it. A bare `begin` leaves both unset -- the roll-back case.
struct RecoveringTxn<'a> {
    /// The transaction id these records belong to.
    id: Nonce,
    /// The `prepare` swap set to re-apply on roll-forward, if a `prepare` was written.
    prepared: Option<&'a PreparedSet>,
    /// The targets the `begin` record named as about to be ingested (`D329`).
    targets: &'a [(NodePath, DocumentName)],
    /// Whether an `end` record closed the transaction (the applied case).
    ended: bool,
}

/// Groups `records` by transaction id, preserving first-seen (log) order.
///
/// D41 serializes transactions, so this is a short list with at most one incomplete
/// tail; a linear scan suffices and keeps the log's order, which matters when two
/// transactions touched the same document -- the later one is recovered last and wins.
fn group_by_transaction(records: &[LogRecord]) -> Vec<RecoveringTxn<'_>> {
    let mut txns: Vec<RecoveringTxn<'_>> = Vec::new();
    for record in records {
        let id = record.txn();
        let pos = match txns.iter().position(|t| &t.id == id) {
            Some(pos) => pos,
            None => {
                txns.push(RecoveringTxn {
                    id: id.clone(),
                    prepared: None,
                    targets: &[],
                    ended: false,
                });
                txns.len() - 1
            }
        };
        match record {
            LogRecord::Begin { targets, .. } => txns[pos].targets = targets,
            LogRecord::Prepare { prepared, .. } => txns[pos].prepared = Some(prepared),
            LogRecord::End { .. } => txns[pos].ended = true,
        }
    }
    txns
}

/// A store's identity, recorded in its genesis marker (D40).
///
/// canopy does not mint this value: the caller (the daemon composition root)
/// supplies it so the store instance can be told apart from another rooted
/// elsewhere. canopy only requires it be non-empty; its structure and uniqueness
/// are the caller's concern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShireId(String);

impl ShireId {
    /// Builds an identity from `raw`, rejecting the empty string.
    pub fn new(raw: impl Into<String>) -> Result<Self, StoreError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(StoreError::EmptyShireId);
        }
        Ok(ShireId(raw))
    }

    /// The identity as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ShireId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The configuration [`Store::init`] writes into a new store's genesis marker.
///
/// [`GenesisConfig::new`] takes the two runtime-variable values canopy will not
/// invent -- the [`ShireId`] and the creation time -- and defaults every physical
/// layout parameter. Each `with_*` setter keeps its value valid by construction.
///
/// **There is no digest-algorithm parameter.** SHA-256 is fixed in code (`D322`): SHA-512
/// produces a filename past the 255-character limit NTFS imposes independently of
/// `MAX_PATH`, and SHA-384 leaves no usable budget for a caller-chosen store root. A
/// parameter with one legal value is not a parameter.
///
/// [`with_shard_width`] is a sharding parameter in a format that deleted one. It is not the
/// same parameter returning: `D222` retired `shard_depth` because it sharded *caller-supplied
/// names*, whose distribution canopy cannot assume; this shards a hash, which is uniform by
/// construction (`D324`).
///
/// [`with_shard_width`]: GenesisConfig::with_shard_width
#[derive(Debug, Clone)]
pub struct GenesisConfig {
    blob_replica_count: usize,
    pointer_replica_count: usize,
    shard_width: usize,
    shire_id: ShireId,
    created_at_unix_ms: u64,
}

impl GenesisConfig {
    /// A configuration for a store identified by `shire_id`, created at
    /// `created_at_unix_ms` (a caller-supplied Unix-epoch millisecond stamp),
    /// with every layout parameter at its default.
    pub fn new(shire_id: ShireId, created_at_unix_ms: u64) -> Self {
        GenesisConfig {
            blob_replica_count: DEFAULT_BLOB_REPLICA_COUNT,
            pointer_replica_count: DEFAULT_POINTER_REPLICA_COUNT,
            shard_width: DEFAULT_SHARD_WIDTH,
            shire_id,
            created_at_unix_ms,
        }
    }

    /// Sets how many replicas of each blob to keep, which must be positive and no
    /// greater than [`MAX_REPLICA_COUNT`].
    ///
    /// **No odd constraint.** A blob is named by its digest and so validates on its own, so
    /// a bad copy is identified rather than outvoted and there is no tie to break (`D316`).
    /// The count is purely a durability choice: how many simultaneous losses to survive.
    pub fn with_blob_replica_count(mut self, count: usize) -> Result<Self, StoreError> {
        if count == 0 || count > MAX_REPLICA_COUNT {
            return Err(StoreError::InvalidBlobReplicaCount { count });
        }
        self.blob_replica_count = count;
        Ok(self)
    }

    /// The configured blob replica count.
    pub fn blob_replica_count(&self) -> usize {
        self.blob_replica_count
    }

    /// Sets how many replicas of each namespace pointer to keep, which must be a positive
    /// **odd** number no greater than [`MAX_REPLICA_COUNT`].
    ///
    /// Odd, because a pointer read votes (`D320`). Verification cannot distinguish a stale
    /// pointer from a current one -- a stale pointer is a valid older pointer -- so the
    /// namespace is the one place where copies genuinely cannot be told apart by examining
    /// them, and a majority needs to exist.
    ///
    /// **Choose this on criticality, not on size.** A blob replica is individually
    /// replaceable from any surviving good copy. The namespace is not replaceable from
    /// anything: it is the only description of what the store contains, `files/` is keyed by
    /// a one-way hash of the path and cannot describe itself, and the log is reset at
    /// quiescence and is not a history. Losing every pointer replica loses the store with
    /// every byte of content still on disk.
    pub fn with_pointer_replica_count(mut self, count: usize) -> Result<Self, StoreError> {
        if count == 0 || count.is_multiple_of(2) || count > MAX_REPLICA_COUNT {
            return Err(StoreError::InvalidPointerReplicaCount { count });
        }
        self.pointer_replica_count = count;
        Ok(self)
    }

    /// The configured pointer replica count.
    pub fn pointer_replica_count(&self) -> usize {
        self.pointer_replica_count
    }

    /// Sets the shard width in **leading characters of the rendered path hash**, which must
    /// be between 1 and [`MAX_SHARD_WIDTH`].
    ///
    /// A width, not a fan-out: 256 is not a power of 32, so a 256-way shard cannot be a
    /// prefix of a base32 rendering, and deriving one from the underlying bits would break
    /// the property that the shard directory is a *visible* prefix of the filename (`D322`).
    /// Fan-out is `32^width`.
    pub fn with_shard_width(mut self, width: usize) -> Result<Self, StoreError> {
        if width == 0 || width > MAX_SHARD_WIDTH {
            return Err(StoreError::InvalidShardWidth { width });
        }
        self.shard_width = width;
        Ok(self)
    }

    /// The configured shard width, in leading characters of the rendered path hash.
    pub fn shard_width(&self) -> usize {
        self.shard_width
    }

    /// The configured store identity.
    pub fn shire_id(&self) -> &ShireId {
        &self.shire_id
    }

    /// The configured creation timestamp (Unix-epoch milliseconds).
    pub fn created_at_unix_ms(&self) -> u64 {
        self.created_at_unix_ms
    }
}

/// The persisted genesis marker: the store's self-describing layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Genesis {
    format_version: u32,
    blob_replica_count: usize,
    pointer_replica_count: usize,
    shard_width: usize,
    shire_id: String,
    created_at_unix_ms: u64,
}

impl Genesis {
    fn from_config(config: GenesisConfig) -> Self {
        Genesis {
            format_version: GENESIS_FORMAT_VERSION,
            blob_replica_count: config.blob_replica_count,
            pointer_replica_count: config.pointer_replica_count,
            shard_width: config.shard_width,
            shire_id: config.shire_id.into_string(),
            created_at_unix_ms: config.created_at_unix_ms,
        }
    }
}

/// Just enough of a marker to decide whether this build may interpret the rest (`D325`).
///
/// A version check is only a refusal mechanism if it runs *before* the marker is parsed as
/// the current shape. Deserializing into [`Genesis`] first works only while every older
/// format is a superset of this one -- true when v2 dropped a field v1 had, false the moment
/// a version adds a required field, which is what v3 does. Parsing this first makes refusal
/// depend on the version alone, in both directions, which is what `D318` relies on.
#[derive(Debug, Deserialize)]
struct GenesisVersion {
    format_version: u32,
}

/// The immediate children of one node path: its documents and its child nodes
/// (`D222`, `BG-3`).
///
/// The two are **separate lists rather than one tagged list** because a caller almost
/// always wants one or the other -- a collection's members, or the tiers beneath a
/// prefix -- and folding them together would make every caller re-split them. Keeping
/// them apart also makes the one-namespace rule visible in the type: a name appears in
/// exactly one of the two, never both.
///
/// Documents are named relative to the path (a [`DocumentName`], since a document is a
/// leaf); child nodes are **full paths**, so a caller can descend without reassembling
/// anything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Children {
    documents: Vec<DocumentName>,
    nodes: Vec<NodePath>,
}

impl Children {
    /// The documents at this path, sorted.
    pub fn documents(&self) -> &[DocumentName] {
        &self.documents
    }

    /// The child nodes beneath this path, as full paths, sorted.
    pub fn nodes(&self) -> &[NodePath] {
        &self.nodes
    }

    /// Whether the path has neither documents nor child nodes.
    ///
    /// True both for a path nothing was ever written beneath and for one whose contents
    /// have all been removed -- the store keeps no distinction between them (`BG-3`).
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty() && self.nodes.is_empty()
    }

    /// Consumes the listing into its two parts.
    pub fn into_parts(self) -> (Vec<DocumentName>, Vec<NodePath>) {
        (self.documents, self.nodes)
    }
}

/// A handle to a Canopy document store rooted at a Shire directory (D40/D54).
///
/// Obtained from [`Store::init`] (create a new store) or [`Store::open`] (attach
/// to an existing one). The handle holds the [`Vfs`] seam (D55) through which all
/// its filesystem access flows and the validated genesis parameters. The
/// document read/write surface and the D41 single-writer monitor are added by
/// later CANOPY-M2 items; LAY-1 establishes the layout and its open-time
/// validation.
///
/// # The root must be on a local filesystem
///
/// Network filesystems (SMB/CIFS shares including mapped network drives, NFS mounts, and other
/// remote or distributed filesystems) are not supported. The store's guarantees rest on the
/// local operating system's lock, rename and fsync behaviour, which they do not reliably provide.
/// A networked root is neither detected nor refused, and may appear to work while losing writes
/// or corrupting the store.
#[derive(Clone)]
pub struct Store {
    vfs: Arc<dyn Vfs>,
    root: PathBuf,
    genesis: Genesis,
    /// Blob reads taken through this store, which is what advances the `D317` rotation
    /// epoch.
    ///
    /// `Arc` for the same reason the lock is: clones of a `Store` are one store, so they
    /// must sweep the replicas together rather than each keeping a private count that never
    /// reaches an epoch boundary.
    blob_reads: Arc<std::sync::atomic::AtomicU64>,
    /// Pointer reads taken through this store, which advances the namespace rotation epoch.
    ///
    /// `Arc` for the same reason `blob_reads` is.
    pointer_reads: Arc<std::sync::atomic::AtomicU64>,
    /// Durable modifications made through this store, which is what lets a reader tell a
    /// raced read from a damaged one (`D335`).
    ///
    /// `Arc` for the same reason `blob_reads` is: clones of a `Store` are one store, and a
    /// counter that each clone kept privately would report "nothing changed" while a sibling
    /// handle was writing.
    modifications: Arc<ModificationCount>,
    /// Blobs a read found a damaged replica of, waiting for a writer to repair them
    /// (`D339`).
    ///
    /// `Arc` for the same reason `blob_reads` is: the handle that reads and the handle the
    /// write monitor owns are the same store, and a backlog each clone kept privately would
    /// be filled by the reader and drained by nobody.
    repair_backlog: Arc<Mutex<RepairBacklog>>,
    /// Serializes writes across every clone of this store and every monitor spawned from
    /// one, so the single writer the OS lock grants this process is single in-process too.
    writer: Arc<WriterGate>,
    /// What is known durable about the commit log, shared by every clone so each sync is
    /// paid once per open rather than once per transaction.
    log_durability: Arc<crate::commit_log::LogDurability>,
    /// Directories created whose names could not be made durable, nor the directory removed.
    unsynced_dirs: Arc<Mutex<DirSyncDebt>>,
    /// What is known durable about the publication record's name, shared by every clone.
    publication: Arc<PublicationDurability>,
    /// The root's exclusive-writer lock (`D116`), held for as long as any handle
    /// to this store lives.
    ///
    /// `Arc` because `Store` is `Clone` and the clones are one writer, not
    /// several: re-acquiring per clone would deadlock against ourselves, so the
    /// lock is released only when the **last** handle drops. There is no `Drop`
    /// impl -- dropping the `Arc` is the release.
    _lock: Arc<dyn VfsLock>,
    /// The nonce source and commit observer the public write surface uses, shared by every
    /// clone because the clones are one writer.
    surface: Arc<surface::Surface>,
}

impl Store {
    /// The store's root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// How many durable modifications this store has made since it was opened (`D335`).
    ///
    /// Monotonic, in memory, and meaningful **only because `D116` gives a root exactly one
    /// writer**: there is no other process to miss. It counts modifications rather than
    /// transactions, so the non-transactional write path is visible here even though it
    /// journals nothing (`D332`) -- which is the whole point, since that path is the one a
    /// log-based answer cannot see.
    ///
    /// # What it is for: telling a raced read from a damaged one
    ///
    /// A pointer read samples replicas one at a time, so a write landing between samples
    /// leaves the reader holding values that never coexisted and no majority forms
    /// (`D333`). That is indistinguishable, from the error alone, from a store that really
    /// has diverged. Read this before the read and again after a failure: **if both samples
    /// are equal and even, nothing was written, and the disagreement is real**
    /// ([`Store::undisturbed`]). If it moved, or either sample is odd -- a modification was in
    /// progress -- the read was raced and reading again may settle it.
    ///
    /// The caller decides whether, when, and how often to read again. canopy classifies; it
    /// does not retry, and it does not sleep.
    ///
    /// # Why it is raised on both sides of a modification
    ///
    /// The count is raised once when a modification starts, leaving it odd, and once when it
    /// ends, leaving it even again -- including when it ends in an error. A writer that pauses
    /// part-way across the replicas is therefore visible as an odd count: a reader whose two
    /// samples straddle the pause sees them equal but odd, which is contention, not damage.
    /// Raising it only at the end would make that reader see an unmoved count and conclude
    /// "damage" about a store that was merely busy.
    ///
    /// It may rise more than once for one logical change; only *movement* and parity are
    /// meaningful.
    /// It resets on reopen, which is why it is only ever compared against itself within a
    /// single caller's read.
    pub fn modifications(&self) -> u64 {
        self.modifications
            .count
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Whether two [`modifications`](Store::modifications) samples bracket a span in which
    /// nothing was modified: they are equal, and no modification was in progress at either.
    pub const fn undisturbed(before: u64, after: u64) -> bool {
        before == after && before.is_multiple_of(2)
    }

    /// Creates `dir` and any missing ancestors below the store root, making each new
    /// directory's name durable in its parent.
    ///
    /// Syncing only the deepest directory makes *its* entries durable but not its own name
    /// or any newly created ancestor's, so a crash could drop a whole freshly created chain
    /// together with the entries the caller just made durable inside it.
    ///
    /// A retry decides what to create by what is missing, so a directory whose parent could
    /// not be synced would never be synced again once it exists -- by this process or, after
    /// a reopen, by any other. So the chain about to be created is first recorded durably in
    /// [`DIR_SYNC_DEBT_NAME`]; only then is each directory created and its parent synced,
    /// outermost first. A directory whose sync fails is removed if it can be and otherwise
    /// remembered, and the durable record means a reopen syncs it too. On success the record
    /// is truncated without a flush: a stale record only costs a reopen some needless syncs.
    fn create_dir_durably(&self, dir: &Path) -> Result<(), StoreError> {
        let mut debt = self
            .unsynced_dirs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.discharge_unsynced_dirs(&mut debt)?;
        let mut missing = Vec::new();
        let mut cursor = dir;
        while cursor != self.root && cursor.starts_with(&self.root) {
            match self.vfs.metadata(cursor) {
                Ok(_) => break,
                Err(error) if error.kind() == VfsErrorKind::NotFound => missing.push(cursor),
                Err(error) => return Err(StoreError::Vfs(error)),
            }
            let Some(parent) = cursor.parent() else {
                break;
            };
            cursor = parent;
        }
        if missing.is_empty() {
            return Ok(());
        }
        self.record_dir_intent(&mut debt, &missing)?;
        for created in missing.iter().rev() {
            match self.vfs.create_dir(created) {
                Ok(()) => {}
                Err(error) if error.kind() == VfsErrorKind::AlreadyExists => {}
                Err(error) => return Err(StoreError::Vfs(error)),
            }
            let parent = created
                .parent()
                .expect("a directory below the root has a parent");
            if let Err(error) = self.vfs.fsync(parent) {
                if self.vfs.remove_dir(created).is_err() {
                    debt.dirs.push(created.to_path_buf());
                }
                return Err(StoreError::Vfs(error));
            }
        }
        if debt.dirs.is_empty() {
            let _ = self.vfs.truncate(&self.root.join(DIR_SYNC_DEBT_NAME), 0);
        }
        Ok(())
    }

    /// Durably records that `dirs` are about to be created, before any of them is.
    fn record_dir_intent(&self, debt: &mut DirSyncDebt, dirs: &[&Path]) -> Result<(), StoreError> {
        let mut text = String::new();
        for dir in dirs {
            if let Some(relative) = dir.strip_prefix(&self.root).ok().and_then(Path::to_str) {
                text.push_str(relative);
                text.push('\n');
            }
        }
        let path = self.root.join(DIR_SYNC_DEBT_NAME);
        if debt.torn {
            // Cut back to the last complete record. The append's fsync below makes the
            // shortened length durable along with the new records.
            let bytes = match self.vfs.read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == VfsErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(StoreError::Vfs(error)),
            };
            let complete = bytes
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |at| at + 1);
            if complete < bytes.len() {
                self.vfs.truncate(&path, complete as u64)?;
            }
            debt.torn = false;
        }
        if let Err(error) = self.vfs.append(&path, text.as_bytes()) {
            debt.torn = true;
            return Err(StoreError::Vfs(error));
        }
        self.vfs.fsync(&path)?;
        if !debt.name_synced {
            self.vfs.fsync(&self.root)?;
            debt.name_synced = true;
        }
        Ok(())
    }

    /// Syncs the parent of every directory an earlier open may have created without making
    /// its name durable, then empties the record. Run by `open` before recovery.
    pub(crate) fn discharge_recorded_dirs(&self) -> Result<(), StoreError> {
        let path = self.root.join(DIR_SYNC_DEBT_NAME);
        let bytes = match self.vfs.read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == VfsErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(StoreError::Vfs(error)),
        };
        let mut debt = self
            .unsynced_dirs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Only newline-terminated records count: an unterminated tail is an append that never
        // completed, so nothing it named was created. A record that is not a plain relative
        // path below the root is not one this store wrote, and is not followed out of it.
        let text = String::from_utf8_lossy(&bytes);
        let complete = text.rfind('\n').map_or("", |at| &text[..at]);
        for line in complete.split('\n') {
            let relative = Path::new(line);
            if !line.is_empty()
                && relative
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                debt.dirs.push(self.root.join(relative));
            }
        }
        self.discharge_unsynced_dirs(&mut debt)?;
        if !bytes.is_empty() {
            self.vfs.truncate(&path, 0)?;
            self.vfs.fsync(&path)?;
        }
        Ok(())
    }

    /// Syncs the parent of each directory in `debt` that still exists.
    fn discharge_unsynced_dirs(&self, debt: &mut DirSyncDebt) -> Result<(), StoreError> {
        while let Some(dir) = debt.dirs.last() {
            match self.vfs.metadata(dir) {
                // An ancestor that is now a file means the directory, and the intent that
                // named it, are gone for good: a later write replaced the subtree.
                Err(error) if is_absent(error.kind()) => {}
                Err(error) => return Err(StoreError::Vfs(error)),
                Ok(_) => {
                    let parent = dir
                        .parent()
                        .expect("a directory below the root has a parent");
                    self.vfs.fsync(parent)?;
                }
            }
            debt.dirs.pop();
        }
        Ok(())
    }

    /// Holds this store's writer gate until the guard drops, as a writer does but without the
    /// fence check, so a test can occupy the gate directly.
    #[cfg(test)]
    pub(crate) fn exclusive(&self) -> WriterGuard {
        self.writer.enter()
    }

    /// Holds this store's writer gate until the guard drops, for a write: refused while a
    /// transaction is unresolved, because writing past it would bury what recovery must
    /// finish. Re-entrant on one thread.
    ///
    /// The outermost entry settles the publication record first, so every operation on the
    /// store -- a write of either kind, a check, a collection, a repair, a scrub -- starts from
    /// a store with no publication pending, and none needs to remember to settle.
    pub(crate) fn writable(&self) -> Result<WriterGuard, StoreError> {
        let guard = self.writer.enter();
        self.refuse_if_unresolved()?;
        if self.writer.held_depth() == 1 {
            self.settle_publication()?;
        }
        Ok(guard)
    }

    pub(crate) fn refuse_if_unresolved(&self) -> Result<(), StoreError> {
        if self.writer.is_unresolved() {
            return Err(StoreError::RecoveryRequired {
                root: self.root.clone(),
            });
        }
        Ok(())
    }

    /// Whether a transaction through any handle to this store failed after it may have
    /// reached its commit point. Writes are refused until the store is reopened.
    pub fn recovery_required(&self) -> bool {
        self.writer.is_unresolved()
    }

    /// Marks a modification in progress until the guard drops, which also marks it ended:
    /// the count is odd in between (`D335`). Take it before the first visible change.
    ///
    /// Nested calls extend the outermost modification rather than starting their own. A count
    /// already odd at the outermost start was left so by an unresolved transaction, and stays
    /// odd.
    pub(crate) fn begin_modification(&self) -> ModificationGuard {
        use std::sync::atomic::Ordering;
        if self.modifications.depth.fetch_add(1, Ordering::AcqRel) == 0
            && self
                .modifications
                .count
                .load(Ordering::Acquire)
                .is_multiple_of(2)
        {
            self.modifications.count.fetch_add(1, Ordering::AcqRel);
        }
        ModificationGuard {
            modifications: Arc::clone(&self.modifications),
            writer: Arc::clone(&self.writer),
        }
    }

    /// Records that a read found `reference` short of its full replication (`D339`).
    ///
    /// Only the blob's identity is kept, never the read's tally of which replicas were bad:
    /// the repair re-examines the blob when it runs, so a finding cannot go stale between
    /// being noticed and being acted on.
    pub(crate) fn note_damaged_blob(&self, reference: BlobRef) {
        self.repair_backlog
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .note(reference);
    }

    /// How many blobs are waiting to be repaired, and how many findings were dropped
    /// because the backlog was full (`D339`).
    pub fn repair_backlog(&self) -> (usize, u64) {
        let backlog = self
            .repair_backlog
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        (backlog.len(), backlog.dropped())
    }

    /// Drains the backlog without repairing, so a test can see *which* findings it kept.
    ///
    /// `D341` turns on which end is evicted when the backlog is full, and the public
    /// `(len, dropped)` pair cannot distinguish the two policies -- both report the same
    /// numbers. Repairing for real would need the blobs to exist on disk, which is beside
    /// the point for a test about the bookkeeping.
    #[cfg(test)]
    pub(crate) fn take_repair_backlog(&self) -> std::collections::VecDeque<BlobRef> {
        self.repair_backlog
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// The number of replicas kept of each blob, as recorded in the genesis marker
    /// (`D316`, `D320`). Unconstrained in parity: blobs are verified, not voted on.
    pub fn blob_replica_count(&self) -> usize {
        self.genesis.blob_replica_count
    }

    /// The number of replicas kept of each namespace pointer, as recorded in the genesis
    /// marker (`D320`). Always odd: pointer reads take a majority.
    pub fn pointer_replica_count(&self) -> usize {
        self.genesis.pointer_replica_count
    }

    /// The shard width in leading characters of the rendered path hash, as recorded in the
    /// genesis marker (`D322`). Fan-out is `32^width`.
    pub fn shard_width(&self) -> usize {
        self.genesis.shard_width
    }

    /// The store's identity, as recorded in the genesis marker.
    pub fn shire_id(&self) -> &str {
        &self.genesis.shire_id
    }

    /// The store's creation timestamp (Unix-epoch milliseconds), as recorded in
    /// the genesis marker.
    pub fn created_at_unix_ms(&self) -> u64 {
        self.genesis.created_at_unix_ms
    }

    /// Replays the commit log at open so the store is consistent (D46/D53/D54, TXN-4).
    ///
    /// An empty or absent log is the O(1) clean-quiesced fast path: nothing was in
    /// flight, so replay is skipped. Otherwise each transaction is resolved by its
    /// records -- D41 serializes transactions, so at most the log's tail is incomplete:
    ///
    /// - `prepare` + `end`: **applied** -- the swaps landed; residual staging (a crash
    ///   between the `end` append and the sweep) is discarded, and the supersession
    ///   collection is re-run (`D328`), since a crash can also land between `end` and it.
    /// - `prepare` without `end`: **roll forward** -- the commit point was reached, so
    ///   re-run the idempotent renames from the `prepare` swap set, record `end`, then
    ///   sweep.
    /// - `begin` without `prepare`: **roll back** -- the commit point was never reached
    ///   and no committed copy was touched (renames happen only after `prepare`), so the
    ///   staging temps are discarded.
    ///
    /// Recovery compares no document versions -- the log alone decides each
    /// transaction's fate, so there is no fence (D54): a rolled-forward conditional swap
    /// re-applies without re-checking its precondition, which already passed before the
    /// `prepare` was written. The log is left as written (roll-forward appends its
    /// `end`); TXN-3's quiescence reset, not recovery, is what empties it.
    fn recover(&self) -> Result<(), StoreError> {
        let records = self.commit_log().read()?;
        if records.is_empty() {
            return Ok(());
        }
        let txns = group_by_transaction(&records);

        // Pass 1 settles every transaction, moving the namespace to its final state.
        for txn in &txns {
            if txn.ended {
                self.sweep_transaction_staging(&txn.id)?;
            } else if let Some(prepared) = txn.prepared {
                self.roll_forward(&txn.id, prepared)?;
            } else {
                self.sweep_transaction_staging(&txn.id)?;
            }
        }

        // Pass 2 collects, and must not start until pass 1 has finished (`D330`).
        //
        // The collection keeps what the namespace points at (`D328`), which is the right
        // question only once the namespace has stopped moving. Interleaving the two passes
        // asks it while a later transaction's roll-forward is still pending: an earlier
        // transaction's collection sees the old pointer, discards the content the pending
        // roll-forward is about to point at, and the roll-forward then installs a pointer to
        // a blob that recovery itself deleted.
        //
        // Re-run even for transactions that completed, because completing is not evidence the
        // collection ran -- `end` is appended before it. The pass is idempotent, so a second
        // run over an already-collected transaction removes nothing.
        for txn in &txns {
            match txn.prepared {
                Some(prepared) => self.collect_superseded(prepared)?,
                // Never prepared, so the only content it can have left is what its `begin`
                // announced it was about to ingest (`D329`).
                None => {
                    for (node, document) in txn.targets {
                        self.collect_document_blobs(node, document)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Completes a rolled-forward transaction (TXN-4): re-runs the idempotent pointer
    /// moves from `swaps`, records `end`, then sweeps the transaction's staging.
    ///
    /// A `prepare` record means every body this transaction writes was ingested and is
    /// durable (`D327`), so each swap's pointer is either still to be moved or already
    /// moved; the namespace apply treats the latter as a completed no-op, making the replay
    /// safe however far the interrupted apply had progressed.
    fn roll_forward(&self, txn: &Nonce, prepared: &PreparedSet) -> Result<(), StoreError> {
        for swap in prepared.swaps() {
            self.apply_swap_to_namespace(swap, txn)?;
        }
        for path in prepared.node_removals() {
            self.apply_node_removal(path)?;
        }
        self.commit_log()
            .append(&LogRecord::End { txn: txn.clone() })?;
        self.sweep_transaction_staging(txn)?;
        Ok(())
    }

    /// The `D53` commit log for this store.
    ///
    /// A thin handle rebuilt on demand; it holds no state beyond the [`Vfs`] and the
    /// log path, so a fresh one per call is equivalent to a stored one.
    pub(crate) fn commit_log(&self) -> CommitLog {
        CommitLog::new(self.vfs.clone(), &self.root).sharing(Arc::clone(&self.log_durability))
    }

    /// Empties the commit log at quiescence (D53, TXN-3).
    ///
    /// The D41 single writer calls this at its quiescent moment -- queue empty, no
    /// transaction open -- so the recovery journal is truncated to empty whenever the
    /// store drains. Because the log is discardable at quiescence (the namespace over the
    /// content store is the durable truth), the next startup sees an empty log and skips replay,
    /// the O(1)-startup commitment. Absent or already-empty logs are left untouched.
    ///
    /// A log holding a `prepare` with no `end` is refused with
    /// [`StoreError::RecoveryRequired`]: that transaction passed its commit point, and the log
    /// is the only record recovery can finish it from.
    pub(crate) fn quiesce_commit_log(&self) -> Result<(), StoreError> {
        let _writer = self.writable()?;
        let log = self.commit_log();
        let records = log.read().map_err(StoreError::CommitLog)?;
        if group_by_transaction(&records)
            .iter()
            .any(|txn| txn.prepared.is_some() && !txn.ended)
        {
            return Err(StoreError::RecoveryRequired {
                root: self.root.clone(),
            });
        }
        log.reset().map_err(StoreError::CommitLog)
    }

    /// Whether the D53 recovery journal is empty -- the O(1)-clean-start observable.
    ///
    /// An empty (or absent) commit log means a warm [`open`](Store::open) had nothing
    /// to replay: the store was left quiescent by a graceful shutdown's proactive reset
    /// (D53/TXN-3, `quiesce_commit_log`). A non-empty log
    /// means the next open must replay it, forgoing the O(1) fast path. Exposed so a
    /// caller (for example a daemon's startup recovery) can observe that a restart
    /// took the clean path.
    pub fn commit_log_is_empty(&self) -> Result<bool, StoreError> {
        Ok(self
            .commit_log()
            .read()
            .map_err(StoreError::CommitLog)?
            .is_empty())
    }

    /// Removes a node's directory from every namespace replica, once its emptiness has been
    /// established in phase 1 (`BG-6`, `D328`).
    ///
    /// **Idempotent, for the same reason a document removal is:** `D53` recovery rolls a
    /// prepared transaction forward by replaying it, so an already-absent directory is
    /// success. The parent directory is fsynced so the removal's own dir-entry change is
    /// durable, and an absent parent is likewise success -- it cannot hold the node.
    ///
    /// Uses a **non-recursive** remove, so this can never take a subtree even if the
    /// phase-1 emptiness check were somehow wrong: the operation itself refuses a
    /// non-empty directory, making `rm -r` unreachable rather than merely unrequested.
    /// A replica whose directory is unexpectedly non-empty surfaces as an error rather
    /// than being silently skipped, since that would mean the store disagrees with the
    /// check that authorised the removal.
    fn apply_node_removal(&self, path: &NodePath) -> Result<(), StoreError> {
        let namespace_dirs = (0..self.genesis.pointer_replica_count).map(|index| {
            namespace::namespace_partition_root(&self.root, index).join(path.relative_path())
        });

        for node_dir in namespace_dirs {
            // Already absent still syncs the parent, as `remove_pointer` does.
            match self.vfs.remove_dir(&node_dir) {
                Ok(()) => {}
                Err(e) if e.kind() == VfsErrorKind::NotFound => {}
                // Beneath a file is absent; the node being a file itself is not.
                Err(e)
                    if e.kind() == VfsErrorKind::NotADirectory
                        && !matches!(
                            self.vfs.metadata(&node_dir),
                            Ok(metadata) if metadata.file_type == FileType::File
                        ) => {}
                Err(e) => return Err(StoreError::Vfs(e)),
            }
            if let Some(parent) = node_dir.parent() {
                match self.vfs.fsync(parent) {
                    Ok(()) => {}
                    Err(e) if is_absent(e.kind()) => {}
                    Err(e) => return Err(StoreError::Vfs(e)),
                }
            }
        }
        Ok(())
    }

    /// Removes the staging a transaction left behind, in **both trees** (TXN-2, `D329`).
    /// Anything already gone is fine -- a swept or replayed transaction.
    ///
    /// Each tree stages differently, so each is swept differently: the namespace keeps a
    /// directory per nonce, and the content store a single temp file named for the nonce.
    /// The primitives sweep their own staging on the paths they complete; this is for the
    /// paths they did not, which is every crash.
    fn sweep_transaction_staging(&self, txn: &Nonce) -> Result<(), StoreError> {
        let namespace_staging = (0..self.genesis.pointer_replica_count).map(|index| {
            namespace::namespace_partition_root(&self.root, index)
                .join(STAGING_DIR)
                .join(txn.path_component().as_ref())
        });
        for staging in namespace_staging {
            match self.vfs.remove_dir_all(&staging) {
                Ok(()) => {}
                Err(e) if e.kind() == VfsErrorKind::NotFound => {}
                Err(e) => return Err(StoreError::Vfs(e)),
            }
        }
        self.sweep_blob_staging(txn, self.genesis.blob_replica_count);
        Ok(())
    }
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("root", &self.root)
            .field("shire_id", &self.genesis.shire_id)
            .field("created_at_unix_ms", &self.genesis.created_at_unix_ms)
            .finish_non_exhaustive()
    }
}
