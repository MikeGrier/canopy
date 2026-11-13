// Copyright (c) Michael Grier.

//! Transaction value types for the canopy store (D53/D54/D58).
//!
//! A [`Swap`] is one change to a `(node, document)` document -- a write of a new body,
//! or a removal -- optionally conditional on the current body's digest (a
//! compare-and-set, D54). A [`SwapSet`] is a set of swaps that D53 applies atomically as
//! one unit -- the shape of a multi-document transaction; a single-document update is
//! the degenerate one-element case. Writes and removals share the one type because a
//! transaction must be able to mix them: removing a queue entry and advancing the ticket
//! it belonged to have to land together.
//!
//! A swap addresses the same `(node, document)` leaf the store's read/write path uses
//! (D62): the [`NodePath`] names the directory and the [`DocumentName`] the file
//! within it, so a transaction can write two documents at one node (for example a
//! `record.json` and a companion document, D62) as two distinct swaps.
//!
//! These are value types only: they describe *what* a transaction changes. The D53
//! two-phase commit that applies a swap set (staging, the NDJSON commit log, the
//! idempotent renames) is the store I/O layer, not defined here. Per D54 there is
//! **no** per-document generation fence: a swap carries only the target, the new
//! body, and the optional expected-digest -- nothing canopy would have to version.

use std::collections::HashSet;
use std::fmt;

use crate::body::{ContentDigest, DocumentBody};
use crate::name::{DocumentName, NodePath};

/// The precondition a [`Swap`] places on the current state of its `(node, document)`
/// target before the transaction may apply it (D54).
///
/// A swap carries exactly one of three preconditions. canopy evaluates them all in the
/// D53 transaction's phase-1, before any log or disk mutation, so a mismatch rejects
/// the whole set at no fsync cost (D69).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precondition {
    /// No precondition: write the body regardless of the current state (a blind write).
    Unconditional,
    /// The document must currently exist with exactly this content digest -- the D54
    /// compare-and-set for the read-compute-then-write case.
    ExpectedDigest(ContentDigest),
    /// The document must currently be **absent** -- the conditional-create sibling of
    /// the CAS. The swap's body is written iff no document yet exists at the target, so
    /// two racing creators cannot both succeed.
    ExpectedAbsent,
}

/// What a [`Swap`] does to its `(node, document)` target (`D222`, `BG-5`).
///
/// A swap targets one document with one precondition; the effect is what differs. The
/// two live in one type rather than two because a transaction must be able to mix them
/// -- removing a queue entry *and* advancing the ticket it belonged to must land
/// together, which is `D53`'s whole purpose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Place this body at the target, creating or overwriting it.
    Write(DocumentBody),
    /// Remove the target.
    ///
    /// **Idempotent by specification**, not merely in practice: removing an
    /// already-absent document succeeds. `D53` recovery rolls a prepared transaction
    /// forward by replaying it, so a removal that is not repeatable would turn a
    /// crash between `prepare` and `end` into a permanent failure. A caller wanting
    /// "fail if it is not there" expresses that as a precondition, which is checked
    /// before the commit point where it can still reject the whole set.
    Remove,
}

/// One document change within a transaction: place `body` at, or remove,
/// `(node, document)` (D53).
///
/// A write is **unconditional** ([`Swap::new`]), **conditional on a digest**
/// ([`Swap::conditional`]) matching the current body (the D54 compare-and-set), or
/// **conditional on absence** ([`Swap::conditional_absent`], the conditional-create).
/// A removal is unconditional ([`Swap::remove`]) or digest-conditional
/// ([`Swap::remove_conditional`]). A swap carries nothing else; in particular there is
/// no generation/version fence (D54).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swap {
    node: NodePath,
    document: DocumentName,
    effect: Effect,
    precondition: Precondition,
}

impl Swap {
    /// An unconditional swap: write `body` at `(node, document)` regardless of the
    /// current body.
    pub fn new(node: impl Into<NodePath>, document: DocumentName, body: DocumentBody) -> Swap {
        Swap {
            node: node.into(),
            document,
            effect: Effect::Write(body),
            precondition: Precondition::Unconditional,
        }
    }

    /// A conditional swap: apply only if the current body's digest equals `expected`
    /// (the D54 compare-and-set); otherwise the transaction fails at apply time.
    pub fn conditional(
        node: impl Into<NodePath>,
        document: DocumentName,
        body: DocumentBody,
        expected: ContentDigest,
    ) -> Swap {
        Swap {
            node: node.into(),
            document,
            effect: Effect::Write(body),
            precondition: Precondition::ExpectedDigest(expected),
        }
    }

    /// A conditional-create swap: apply only if the document is currently **absent**
    /// (the conditional-create sibling of the CAS); otherwise the transaction fails at
    /// apply time. Two racing creators cannot both succeed.
    pub fn conditional_absent(
        node: impl Into<NodePath>,
        document: DocumentName,
        body: DocumentBody,
    ) -> Swap {
        Swap {
            node: node.into(),
            document,
            effect: Effect::Write(body),
            precondition: Precondition::ExpectedAbsent,
        }
    }

    /// An unconditional removal: delete `(node, document)` whatever it currently holds
    /// (`BG-5`).
    ///
    /// Succeeds when the document is **already absent** -- see [`Effect::Remove`] for
    /// why that is specified rather than incidental. A caller that needs "fail if it is
    /// not there" wants [`remove_conditional`](Swap::remove_conditional), whose
    /// precondition is checked before the commit point and can still reject the set.
    ///
    /// Removing a document does not remove the node that held it; an emptied node is
    /// removed explicitly (`BG-6`).
    pub fn remove(node: impl Into<NodePath>, document: DocumentName) -> Swap {
        Swap {
            node: node.into(),
            document,
            effect: Effect::Remove,
            precondition: Precondition::Unconditional,
        }
    }

    /// A conditional removal: delete `(node, document)` only if its current body's
    /// digest equals `expected`; otherwise the whole transaction is rejected (`BG-5`).
    ///
    /// The compare-and-set of removals -- "delete what I read, not whatever is there
    /// now" -- and the reason `Precondition` is reused rather than a second mechanism
    /// invented: the question a removal asks about current state is exactly the
    /// question a write asks.
    ///
    /// There is deliberately no absence-conditional removal: "delete it only if it is
    /// already gone" is a no-op whenever its precondition holds, so the API does not
    /// offer a way to write one.
    pub fn remove_conditional(
        node: impl Into<NodePath>,
        document: DocumentName,
        expected: ContentDigest,
    ) -> Swap {
        Swap {
            node: node.into(),
            document,
            effect: Effect::Remove,
            precondition: Precondition::ExpectedDigest(expected),
        }
    }

    /// The node path this swap changes.
    pub fn node(&self) -> &NodePath {
        &self.node
    }

    /// The document leaf this swap changes.
    pub fn document(&self) -> &DocumentName {
        &self.document
    }

    /// What this swap does to its target.
    pub fn effect(&self) -> &Effect {
        &self.effect
    }

    /// The new body this swap writes, or `None` for a removal.
    pub fn body(&self) -> Option<&DocumentBody> {
        match &self.effect {
            Effect::Write(body) => Some(body),
            Effect::Remove => None,
        }
    }

    /// Whether this swap removes its target rather than writing it.
    pub fn is_removal(&self) -> bool {
        matches!(self.effect, Effect::Remove)
    }

    /// The swap's precondition (D54).
    pub fn precondition(&self) -> Precondition {
        self.precondition
    }

    /// The expected current-body digest for a digest-conditional swap, or `None` for an
    /// unconditional or conditional-create swap.
    pub fn expected(&self) -> Option<ContentDigest> {
        match self.precondition {
            Precondition::ExpectedDigest(expected) => Some(expected),
            Precondition::Unconditional | Precondition::ExpectedAbsent => None,
        }
    }
}

/// A set of [`Swap`]s applied atomically as one unit (D53).
///
/// This is a multi-document transaction; a single-document update is the one-element
/// case ([`SwapSet::single`]). The targets in a set are distinct -- writing the same
/// `(node, document)` twice in one transaction is ambiguous -- so [`SwapSet::new`]
/// rejects a duplicate target. Two documents at the *same* node are distinct targets
/// and are permitted (D62). An empty set is permitted (a vacuously atomic no-op).
/// Insertion order is preserved; because the targets are distinct, that order does
/// not change the resulting state (each document is written once).
///
/// A set may also carry **node removals** ([`SwapSet::with_node_removals`], `BG-6`),
/// which land atomically with the document changes -- the point being that a caller can
/// remove a node's last document and the node itself in one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapSet {
    swaps: Vec<Swap>,
    node_removals: Vec<NodePath>,
}

impl SwapSet {
    /// Builds a swap set from `swaps`, rejecting a duplicate `(node, document)` target.
    pub fn new(swaps: impl IntoIterator<Item = Swap>) -> Result<SwapSet, SwapSetError> {
        SwapSet::with_node_removals(swaps, [])
    }

    /// Builds a set of document changes plus node removals, applied as one transaction
    /// (`BG-6`).
    ///
    /// Node removal is **`rmdir`, never `rm -r`**: a node is removed only if it holds no
    /// documents and no child nodes once this transaction's own changes are accounted
    /// for, and the check happens in phase 1 so a violation rejects the whole set before
    /// the commit point. That accounting is what makes the driving case work -- removing
    /// a node's last document and the node together -- and it is a genuinely different
    /// kind of precondition from a digest CAS, which asks about state *before* the
    /// transaction where this asks about state *after* it.
    ///
    /// Rejects a duplicate `(node, document)` target as [`SwapSet::new`] does, a node
    /// named for removal twice, and a node removed while the same transaction writes at
    /// or beneath it -- that last being contradictory rather than merely redundant, so it
    /// is refused rather than silently ordered.
    pub fn with_node_removals(
        swaps: impl IntoIterator<Item = Swap>,
        node_removals: impl IntoIterator<Item = NodePath>,
    ) -> Result<SwapSet, SwapSetError> {
        let swaps: Vec<Swap> = swaps.into_iter().collect();
        let mut seen: HashSet<(&NodePath, &str)> = HashSet::with_capacity(swaps.len());
        for swap in &swaps {
            if !seen.insert((&swap.node, swap.document.as_str())) {
                return Err(SwapSetError::DuplicateTarget {
                    node: swap.node.to_string(),
                    document: swap.document.as_str().to_owned(),
                });
            }
        }

        let node_removals: Vec<NodePath> = node_removals.into_iter().collect();
        let mut seen_nodes: HashSet<&NodePath> = HashSet::with_capacity(node_removals.len());
        for path in &node_removals {
            if !seen_nodes.insert(path) {
                return Err(SwapSetError::DuplicateNodeRemoval {
                    node: path.to_string(),
                });
            }
        }

        // A write at or beneath a node this transaction removes has no coherent outcome:
        // ordering it removal-first leaves the write's node re-created, and write-first
        // leaves the removal refused for non-emptiness. Neither is what a caller meant,
        // so the set is refused at construction -- before any store is even involved.
        for path in &node_removals {
            for swap in &swaps {
                if swap.is_removal() || !path.contains(&swap.node) {
                    continue;
                }
                return Err(SwapSetError::WriteBeneathRemovedNode {
                    node: path.to_string(),
                    write: swap.node.to_string(),
                    document: swap.document.as_str().to_owned(),
                });
            }
        }

        Ok(SwapSet {
            swaps,
            node_removals,
        })
    }

    /// A single-document transaction -- the degenerate one-element swap set, always valid.
    pub fn single(swap: Swap) -> SwapSet {
        SwapSet {
            swaps: vec![swap],
            node_removals: Vec::new(),
        }
    }

    /// The swaps in the set, in insertion order.
    pub fn swaps(&self) -> &[Swap] {
        &self.swaps
    }

    /// The nodes this transaction removes, in insertion order (`BG-6`).
    pub fn node_removals(&self) -> &[NodePath] {
        &self.node_removals
    }

    /// The number of swaps in the set.
    ///
    /// Counts document changes only; node removals are [their own
    /// list](SwapSet::node_removals).
    pub fn len(&self) -> usize {
        self.swaps.len()
    }

    /// Whether the set changes nothing at all -- no document swaps and no node removals.
    pub fn is_empty(&self) -> bool {
        self.swaps.is_empty() && self.node_removals.is_empty()
    }
}

/// Why a [`SwapSet`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapSetError {
    /// Two swaps named the same `(node, document)`; a transaction may write each
    /// target at most once.
    DuplicateTarget {
        /// The node of the target named more than once.
        node: String,
        /// The document of the target named more than once.
        document: String,
    },
    /// The same node was named for removal more than once (`BG-6`).
    DuplicateNodeRemoval {
        /// The node named more than once.
        node: String,
    },
    /// A transaction removes a node while also writing a document at or beneath it
    /// (`BG-6`), which has no coherent outcome and is refused rather than ordered.
    WriteBeneathRemovedNode {
        /// The node the transaction removes.
        node: String,
        /// The node the conflicting write targets (the removed node, or one beneath it).
        write: String,
        /// The document the conflicting write places.
        document: String,
    },
}

impl fmt::Display for SwapSetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SwapSetError::DuplicateTarget { node, document } => {
                write!(
                    f,
                    "swap set names the document {document:?} at node {node:?} more than once"
                )
            }
            SwapSetError::DuplicateNodeRemoval { node } => {
                write!(f, "swap set removes the node {node:?} more than once")
            }
            SwapSetError::WriteBeneathRemovedNode {
                node,
                write,
                document,
            } => write!(
                f,
                "swap set removes the node {node:?} while also writing the document \
                 {document:?} at {write:?}"
            ),
        }
    }
}

impl std::error::Error for SwapSetError {}
