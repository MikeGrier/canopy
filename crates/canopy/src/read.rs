// Copyright (c) Michael Grier.

//! The outcome of a document read, and the caller-side drivers for the one outcome that is
//! neither success nor failure (`D336`, `D338`).
//!
//! A read can be contended in two ways, and both are a healthy store under a live writer
//! rather than a fault. Resolving a name and fetching the content it names are two steps, and
//! `D328` has a write collect what it superseded -- so a write landing between them sweeps the
//! version a reader had already resolved. Separately, the namespace is sampled one replica at
//! a time, so a commit in flight across the samples can leave no majority at all.
//!
//! [`Store::read_document`] tells both apart from real loss and then **stops**, reporting
//! [`Read::Contended`]. It does not decide what to do about either.
//!
//! # Why the loop is out here rather than in the store
//!
//! A caller that must read a document will read again until it succeeds -- but it also needs
//! to own when to give up, how long to wait between attempts, and how cancellation reaches
//! the loop. A policy seam inside the store can express none of those well, and retry buried
//! at several levels of a system compounds into latency and cancellation behaviour nobody can
//! predict or interrupt.
//!
//! So the loop is an ordinary function over the public API, with nothing behind it: read it,
//! copy it, or replace it with four lines of your own that close over a cancellation token.
//! There is no trait to implement and no policy object to configure.

use crate::body::ContentDigest;
use crate::{DocumentBody, DocumentName, NodePath, Store, StoreError};

/// What a document read found.
///
/// Three outcomes rather than two, because "someone wrote while you were reading" is neither
/// a value nor a fault, and collapsing it into either would be a lie. Matching is exhaustive
/// on purpose: a caller that treats a benign race as a failed operation should have to say so
/// rather than get there by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// The document is there, and this is its body.
    Found(DocumentBody),
    /// A majority of pointer replicas agree the document is not there.
    ///
    /// A majority rather than unanimity, because absence is a candidate like any other
    /// (`D338`): mid-create, the replicas that hold nothing are holding the document's
    /// previous state, exactly as a mid-overwrite majority holds the previous pointer.
    Absent,
    /// A writer was working on this document while the read was in flight, and the read came
    /// away with nothing. Nothing is wrong; reading again will very likely succeed.
    Contended(Contended),
}

/// A read that a live writer denied an answer to, and what it was able to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contended {
    pub(crate) cause: Contention,
    pub(crate) modifications: u64,
}

/// Which of the two contended shapes a read hit.
///
/// Kept distinct because they are distinguishable and an operator staring at a log wants to
/// know which: they arise at different points and a store that only ever produces one of them
/// is telling you something different from one that produces both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contention {
    /// The read resolved a name, and a writer collected that version before the content could
    /// be fetched (`D333`).
    Superseded {
        /// The content this read had resolved and was fetching when it was swept.
        resolved: ContentDigest,
    },
    /// The namespace replicas held no majority -- neither for a pointer nor for absence --
    /// because a commit was in flight across them when they were sampled (`D320`).
    ///
    /// Unlike being superseded, this one resolves *nothing*: there is no version to name,
    /// because the replicas do not agree there is one.
    Undetermined {
        /// How many replicas held a well-formed pointer, no two of which commanded a
        /// majority.
        present: usize,
        /// How many replicas held no entry at all.
        missing: usize,
    },
}

impl Contended {
    /// Which shape of contention this was.
    pub fn cause(&self) -> Contention {
        self.cause
    }

    /// The store's modification count when the read gave up (`D335`).
    ///
    /// Compare successive values to see whether the writer is still going: a count that has
    /// stopped moving while reads keep coming back contended is the shape of a problem, not
    /// of contention.
    pub fn modifications(&self) -> u64 {
        self.modifications
    }
}

/// How many resolutions canopy's own writer-side reads take before reporting they were outrun.
///
/// The precondition check and the read-modify-write transforms run *as* the writer, and
/// `D116` gives a root one of those. Being superseded there would mean a second writer, which
/// is an anomaly worth surfacing rather than spinning on -- so this is small on purpose.
pub(crate) const WRITER_READ_ATTEMPTS: usize = 2;

/// Reads a document, resolving again for as long as `again` says to.
///
/// `again` is called only on [`Read::Contended`] and decides whether to make another attempt.
/// It is where a deadline, an attempt count, a delay, or a cancellation check belongs -- this
/// function contributes none of them and never sleeps.
///
/// Giving up yields [`StoreError::DocumentOvertaken`], which says the read kept being outrun,
/// not that anything is damaged.
pub fn read_settled<F>(
    store: &Store,
    node: &NodePath,
    document: &DocumentName,
    mut again: F,
) -> Result<Option<DocumentBody>, StoreError>
where
    F: FnMut(&Contended) -> bool,
{
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        match store.read_document(node, document)? {
            Read::Found(body) => return Ok(Some(body)),
            Read::Absent => return Ok(None),
            Read::Contended(contended) => {
                if !again(&contended) {
                    return Err(StoreError::DocumentOvertaken {
                        node: node.to_string(),
                        document: document.as_str().to_string(),
                        attempts,
                    });
                }
            }
        }
    }
}

/// Reads a document and accepts being outrun, without resolving again.
///
/// For a caller that would rather fail than wait -- a readiness probe, or a sweep that will
/// come past again anyway.
pub fn read_once(
    store: &Store,
    node: &NodePath,
    document: &DocumentName,
) -> Result<Option<DocumentBody>, StoreError> {
    read_settled(store, node, document, |_| false)
}

/// Reads a document, resolving again up to `attempts` times in total.
///
/// The plainest useful policy, and the one to reach for when a caller has no deadline or
/// cancellation of its own to honour. It does not sleep: under a writer that never pauses,
/// this spends `attempts` reads quickly and then says so.
pub fn read_within(
    store: &Store,
    node: &NodePath,
    document: &DocumentName,
    attempts: usize,
) -> Result<Option<DocumentBody>, StoreError> {
    let mut remaining = attempts.saturating_sub(1);
    read_settled(store, node, document, |_| {
        let more = remaining > 0;
        remaining = remaining.saturating_sub(1);
        more
    })
}
