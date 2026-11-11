// Copyright (c) Michael Grier.

//! The two things an embedder plugs into a store: where its nonces come from, and who hears
//! about what it wrote.

use crate::body::DocumentBody;
use crate::name::{DocumentName, NodePath, Nonce};

/// A caller-supplied source of write nonces (D61).
///
/// The store calls it once per operation that writes, under a lock, so it is never called
/// concurrently and needs no synchronization of its own. Every clone of a store shares the
/// one source it was opened with.
pub type NonceSource = Box<dyn FnMut() -> Nonce + Send>;

/// An observer of committed document writes: canopy's change feed.
///
/// The store calls [`committed`](CommitObserver::committed) once per document that has
/// actually been written, for every logged commit and every
/// [`write_document_replicated`](crate::Store::write_document_replicated), and
/// [`removed`](CommitObserver::removed) for each removal a commit makes. Hooking the store's
/// write paths rather than their callers is the point: a caller-side hook has to be repeated
/// at every write site and is one forgotten site away from a silently incomplete feed. A
/// rejected commit wrote nothing, so it is not reported, and repairs, which restore content
/// rather than change it, are not reported either.
///
/// # What the feed promises
///
/// **Per-document order, and nothing more.** Writes to a given document are reported in
/// the order they were applied. No order is promised *between* documents, even though the
/// writer gate incidentally provides one: D41 specifies per-node serialization only, so
/// an observer that binds to the total order binds to an implementation accident.
///
/// The feed also carries no *prior* body. Supplying one would cost a read on every write
/// to serve an observer that may not exist, and an observer that needs a before-and-after
/// is better served by remembering what it last saw.
///
/// # What an observer owes
///
/// `committed` runs **on the writing thread**, inside the write and under the writer gate,
/// so it holds up every writer in the process for as long as it runs. An implementation
/// must therefore do bounded work and must not block, perform I/O, or write to the store.
/// It must also not panic: an unwinding observer would abandon the write it was told about
/// part-way, so a fallible observer swallows its own failures.
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
