// Copyright (c) Michael Grier.

//! `canopy` -- the generic durable tree-document store for Canopy (D54).
//!
//! Identifiers such as `D54` or `TXN-2` in these docs name design decisions and work
//! items in the project's design records; they are kept for traceability and are not
//! needed to use the API.
//!
//! Stores a namespace of paths (mapping onto a directory hierarchy) whose nodes
//! hold opaque tree-structured documents that canopy replicates but never parses
//! (D27/D54). It offers atomic single-document updates and atomic multi-document
//! transactions (D53), N-copy quorum durability with an idempotent repair queue
//! (D40), and one writer at a time across every handle to a store (D41). It knows nothing
//! of tickets, IDs, or dispositions -- that domain belongs to the embedding application.
//! The crate is transport-agnostic and proto-free (D51): no gRPC, tonic, or `.proto`.
//!
//! A store is created with [`Store::init`] or opened with [`Store::open`], each taking
//! [`StoreOptions`] that supply the [`NonceSource`] every write draws its [`Nonce`] from
//! (`random-nonce` enables a ready-made random source). Content-verified blobs and the
//! namespace of pointers naming them are kept in separately replicated trees
//! (`D314`..`D316`, `D337`). A read ([`Store::read_document`]) takes the namespace quorum's
//! blob and verifies it against the digest in its own name, so a bad copy is identified
//! rather than outvoted (`D316`, `D320`). Reads take no lock; writes take turns at the
//! store's writer gate.
//!
//! The surface an embedding application binds to (D54):
//!
//! - **Logged writes**, journaled to the commit log and applied all-or-nothing:
//!   [`update_document`](Store::update_document), [`apply_transaction`](Store::apply_transaction),
//!   the read-modify-write [`transform`](Store::transform) (no lost update), and
//!   [`plan_and_apply`](Store::plan_and_apply), which plans swaps from what it reads and
//!   commits them with the writer gate held from the plan to the commit. Each reports the
//!   nonce it used in a [`Commit`].
//! - **The unlogged write** [`write_document_replicated`](Store::write_document_replicated)
//!   (`D332`).
//! - **Reads**: [`read_document`](Store::read_document), [`list_children`](Store::list_children),
//!   [`list_documents`](Store::list_documents), and the optimistic
//!   [`snapshot`](Store::snapshot), which re-runs a read until no write disturbed it.
//! - **Maintenance**: [`scrub_content`](Store::scrub_content),
//!   [`repair_reported_damage`](Store::repair_reported_damage), [`quiesce`](Store::quiesce),
//!   and the change feed installed by [`observe`](Store::observe).
//!
//! Anything domain-specific belongs to the embedding application, layered on top.
//!
//! # What canopy asks of whoever embeds it (`D340`)
//!
//! **Call [`scrub_content`](Store::scrub_content) regularly.** It is the only exhaustive
//! pass over the store, and a store that never gets one loses redundancy silently: reads
//! probe only what they happen to touch (`D317`), and the repair backlog they feed is
//! bounded and lossy on purpose (`D339`, `D341`). A bad replica under a document nobody
//! reads is rebuilt by the scrub or by nothing at all -- and until the last good copy is
//! gone, every read still succeeds, so neglect shows up as a sudden loss rather than as a
//! gradual warning.
//!
//! canopy cannot do this for you and does not try. A library has no clock, no scheduler and
//! no business owning a thread for maintenance (`D61`), so the cadence is yours to pick
//! and yours to keep; advising you of it is the most a library can do, and is therefore
//! something it must do.
//!
//! **Put the store root on a local filesystem.** Network filesystems -- SMB/CIFS shares
//! (including mapped network drives), NFS mounts, and other remote or distributed
//! filesystems -- are not supported. canopy's guarantees rest on the local operating system's
//! lock, rename and fsync behaviour, which they do not reliably provide, and canopy neither
//! detects nor refuses a networked root: one may appear to work while losing writes or
//! corrupting the store.

#![warn(unnameable_types)]

mod body;
mod commit_log;
mod feed;
mod name;
mod naming;
mod pointer;
#[cfg(any(feature = "random-nonce", test))]
mod random;
mod read;
mod store;
mod transaction;

pub use body::{ContentDigest, DocumentBody};
pub use commit_log::CommitLogError;
pub use feed::{CommitObserver, NonceSource};
pub use name::{DocumentName, DocumentNameError, NodePath, NodePathError, Nonce, NonceError};
pub use naming::{Artifact, BlobName, BlobNameError, BlobRef, PathHash};
pub use pointer::{Pointer, PointerError};
#[cfg(feature = "random-nonce")]
pub use random::{random_nonce, random_nonce_source};
pub use read::{Contended, Contention, Read, read_once, read_settled, read_within};
pub use store::{
    BacklogRepairReport, BlobRead, BlobVerification, Children, Commit, ContentScrubReport,
    DEFAULT_BLOB_REPLICA_COUNT, DEFAULT_POINTER_REPLICA_COUNT, DEFAULT_SHARD_WIDTH,
    GENESIS_FORMAT_VERSION, GenesisConfig, LOCK_NAME, MARKER_NAME, MAX_REPLICA_COUNT,
    MAX_SHARD_WIDTH, PointerRead, QUARANTINE_LIMIT, REPAIR_BACKLOG_LIMIT, RejectedPrecondition,
    RepairOutcome, SNAPSHOT_ATTEMPTS, ShireId, Store, StoreError, StoreOptions, StoreView,
    TransactionOutcome,
};
pub use transaction::{Effect, Precondition, Swap, SwapSet, SwapSetError};

#[cfg(test)]
mod tests;
