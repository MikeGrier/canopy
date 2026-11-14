// Copyright (c) Michael Grier.

//! `coord` -- the coordination interface: a layer above [`Vfs`] that decides who may act on a
//! store root and when.
//!
//! It is the contract every synchronization among the participants on one root goes through --
//! threads and processes alike -- together with the shared counters, because their ordering
//! guarantees are stated against each other and one implementation must provide both. A consumer
//! still performs every data operation through [`Vfs`] directly; this layer complements the seam
//! and does not wrap it, and nothing in the seam depends on it. The contract is canopy's design
//! (canopy's DESIGN-NOTES.md, "Several processes may open one store"), stated here in terms that
//! need nothing of canopy's: what only the caller can interpret crosses as bytes it supplies.
//!
//! # Parts
//!
//! - [`Creation`] -- creation exclusion and the creation reservation;
//! - [`RestartRecognition`] -- whether the state can be vouched for;
//! - [`ModificationCount`] -- sampling the count, which only an [`ExclusionHold`] raises;
//! - [`WriterExclusion`] -- exclusion, its ownership record, the fence and the generation;
//! - [`Admission`] -- the admission gate and the attachment tiers;
//! - [`Registry`] -- listing the attached participants;
//! - [`CountNotification`] -- being told when the count moves.
//!
//! # Participants
//!
//! A [`Coordination`] is one participant on one root, made by a [`CoordinationFactory`] from the
//! [`Vfs`] and root its caller uses for the data, since the contract orders the coordination state
//! with the caller's filesystem operations and is correct only for those. Two participants made in
//! one process are as separate as two processes: each holds its own locks, and each excludes the
//! other.
//!
//! # Implementations
//!
//! [`file`](mod@file) is built from [`Vfs`] primitives, so it runs over [`OsFs`](crate::OsFs),
//! [`MemFs`](crate::MemFs) and [`FaultyFs`](crate::FaultyFs) alike. Every implementation passes one
//! conformance suite.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use crate::Vfs;

mod admission;
mod count;
mod creation;
mod error;
mod exclusion;
pub mod file;
mod registry;
mod restart;
mod types;

pub use admission::{Admission, Attachment, Registrant, Upgrade};
pub use count::{CountNotification, ModificationCount, Notice};
pub use creation::{ArtifactMaker, Creation, CreationPlan};
pub use error::{CoordError, CoordResult};
pub use exclusion::{ExclusionHold, WriterExclusion};
pub use registry::Registry;
pub use restart::RestartRecognition;
pub use types::{
    Access, Acquired, Created, CreationNonce, Generation, HeldTiers, ImplementationId, ListedEntry,
    Listing, ModCount, ModificationEnd, Purpose, ReaderSafety, Registration, RegistryToken,
    TierHold, Trust, Untrusted, Waiting,
};

/// The prefix of every name directly beneath a root that belongs to the coordination, in every
/// implementation, so that a creator's reading of the root knows them for the coordination's own.
/// No artifact of creation's begins with it.
pub const RESERVED_PREFIX: &str = ".coord";

/// One participant on one root.
pub trait Coordination:
    Creation
    + RestartRecognition
    + ModificationCount
    + CountNotification
    + WriterExclusion
    + Admission
    + Registry
    + Send
    + Sync
    + fmt::Debug
{
    /// The implementation this participant belongs to.
    fn implementation(&self) -> &ImplementationId;

    /// Simulates this participant's death, for tests: every hold it has is released as the
    /// operating system releases a dead process's, and no clean-release bookkeeping runs -- an
    /// ownership record stays, a registry entry is left uncleared.
    ///
    /// Every later operation of the participant is refused with
    /// [`Abandoned`](CoordError::Abandoned), and dropping a hold it made releases nothing more.
    fn abandon(&self);
}

/// Makes participants of one implementation.
pub trait CoordinationFactory: Send + Sync + fmt::Debug {
    /// The implementation whose participants this makes.
    fn implementation(&self) -> &ImplementationId;

    /// A participant on the root `root` of `vfs`. Making one changes nothing in the root.
    ///
    /// # Errors
    ///
    /// A failure the implementation meets in making it.
    fn open(&self, vfs: Arc<dyn Vfs>, root: &Path) -> CoordResult<Box<dyn Coordination>>;
}
