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
//! (D40), and a single-writer monitor (D41). It knows nothing of tickets, IDs, or
//! dispositions -- that domain belongs to the embedding application. The crate
//! is transport-agnostic and proto-free (D51): no gRPC, tonic, or `.proto`.
//!
//! The value types are in place -- the namespace path ([`NodePath`]), the opaque
//! document body ([`DocumentBody`]), and the transaction shape ([`SwapSet`]) -- over an
//! on-disk layout ([`Store::init`] / [`Store::open`] above the `.shire` marker) that keeps
//! content-verified blobs and the namespace of pointers naming them in separately
//! replicated trees (`D314`..`D316`, `D337`). A document is written whole by
//! [`Store::write_document_replicated`], which stages and atomically renames each replica
//! under a caller-supplied [`Nonce`] (D40/D8), and read by [`Store::read_document`]: the
//! namespace quorum names a blob and the blob is verified against the digest in its own
//! name, so a bad copy is identified rather than outvoted (`D316`, `D320`). All writes funnel
//! through the single-writer [`WriteMonitor`] actor, which applies a caller-supplied
//! read-modify-write [`transform`](WriteMonitor::transform) (no lost update) or a
//! compare-and-set (D41/D54, LAY-5) while reads bypass it lock-free. The batch sibling of
//! the read is [`scrub_content`](Store::scrub_content), which walks the namespace and
//! repairs from a verified copy (`D331`).
//! The D53 transaction commit begins with its journal: an append-only NDJSON commit
//! log at the store root records `begin`/`prepare`/`end` records, each a self-delimited
//! line so a torn final append is discarded on read (TXN-1). The two-phase apply path
//! ([`apply_transaction`](Store::apply_transaction), TXN-2), its quiescence truncation,
//! and the open-time recovery replay (TXN-4) that drive it are in place.
//!
//! The generic store surface an embedding application binds to (D54, TXN-6) is:
//! atomic single-document update ([`update_document`](Store::update_document), the
//! one-element swap set), atomic multi-document transaction
//! ([`apply_transaction`](Store::apply_transaction), a swap set applied all-or-nothing),
//! content-verified read of a named document ([`read_document`](Store::read_document)),
//! and namespace traversal ([`list_documents`](Store::list_documents)). Anything
//! domain-specific belongs to the embedding application, layered on top.
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

#![warn(unnameable_types)]

mod body;
mod commit_log;
mod monitor;
mod name;
mod naming;
mod pointer;
mod read;
mod store;
mod transaction;

pub use body::{ContentDigest, DocumentBody};
pub use commit_log::CommitLogError;
pub use monitor::{CommitObserver, MonitorError, NonceSource, WriteMonitor, WriteOutcome};
pub use name::{DocumentName, DocumentNameError, NodePath, NodePathError, Nonce, NonceError};
pub use naming::{Artifact, BlobName, BlobNameError, BlobRef, PathHash};
pub use pointer::{Pointer, PointerError};
pub use read::{Contended, Contention, Read, read_once, read_settled, read_within};
pub use store::{
    BacklogRepairReport, BlobRead, BlobVerification, Children, ContentScrubReport,
    DEFAULT_BLOB_REPLICA_COUNT, DEFAULT_POINTER_REPLICA_COUNT, DEFAULT_SHARD_WIDTH,
    GENESIS_FORMAT_VERSION, GenesisConfig, LOCK_NAME, MARKER_NAME, MAX_REPLICA_COUNT,
    MAX_SHARD_WIDTH, PointerRead, QUARANTINE_LIMIT, REPAIR_BACKLOG_LIMIT, RejectedPrecondition,
    RepairOutcome, ShireId, Store, StoreError, TransactionOutcome,
};
pub use transaction::{Effect, Precondition, Swap, SwapSet, SwapSetError};

#[cfg(test)]
mod tests;
