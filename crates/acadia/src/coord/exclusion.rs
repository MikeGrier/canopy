// Copyright (c) Michael Grier.

//! [`WriterExclusion`] and [`ExclusionHold`]: writer exclusion, its ownership record, the fence, the
//! generation, and raising the modification count (canopy's DESIGN-NOTES.md, "Writer exclusion and
//! its ownership record, modelled in `Coordination.tla`").

use std::fmt;

use super::error::CoordResult;
use super::types::{Acquired, ModificationEnd, Purpose, Waiting};

/// Writer exclusion: at most one holder across every thread of every process on the root.
pub trait WriterExclusion {
    /// Acquires writer exclusion, waiting as `waiting` allows.
    ///
    /// No order among waiters is promised, and nothing may depend on one. Exclusion is not
    /// re-entrant: a participant that holds it and acquires again waits for itself. It is released
    /// when the hold is dropped or released, and when the holder's process dies.
    ///
    /// Acquiring is three steps, and a death between any two leaves the previous ownership record
    /// for the next acquirer: the lock is taken; if a record was left behind -- its holder died
    /// holding exclusion -- the fence is set; then the record is replaced by this holder's and the
    /// generation advanced. What was found is reported by [`ExclusionHold::acquired`]. An acquirer
    /// told recovery is required has changed no store data, and may release, run recovery, or
    /// begin a [`Purpose::Recovery`] modification.
    ///
    /// # Errors
    ///
    /// [`WouldWait`](super::CoordError::WouldWait) with [`Waiting::Never`] when another holds it,
    /// and a failure of the filesystem, either of which leaves exclusion not held.
    fn acquire(&self, waiting: Waiting) -> CoordResult<Box<dyn ExclusionHold>>;
}

/// A hold of writer exclusion.
///
/// Dropping it is a clean release, as [`ExclusionHold::release`] is, with any failure unreported.
/// A clean release clears the ownership record while it still holds the lock and only then drops
/// the lock, so a record found under the lock always means its holder died; a release whose
/// clearing fails drops the lock with the record in place, which costs the next acquirer a
/// recovery and never costs correctness.
pub trait ExclusionHold: Send + fmt::Debug {
    /// What the acquisition found.
    fn acquired(&self) -> &Acquired;

    /// Begins a modification: the outermost raises the count to odd before the caller's first
    /// effect, and one nested inside it changes nothing.
    ///
    /// A whole logical operation is one modification, however many filesystem steps it takes, so
    /// a reader can tell an operation in progress from a settled store.
    ///
    /// # Errors
    ///
    /// [`RecoveryRequired`](super::CoordError::RecoveryRequired) for [`Purpose::Ordinary`] while
    /// recovery is required; a failure to publish the opening raise, after which no effect may be
    /// made and the count is unchanged.
    fn begin_modification(&mut self, purpose: Purpose) -> CoordResult<()>;

    /// Ends the innermost open modification. The outermost one publishes the closing raise for
    /// [`ModificationEnd::Settled`], leaving the count even; for
    /// [`ModificationEnd::Unresolved`] it sets the fence and leaves the count odd, so that no
    /// snapshot accepts the partial result.
    ///
    /// # Errors
    ///
    /// A failure to publish the closing raise, which sets the fence and leaves the count odd, so
    /// recovery -- which publishes it again -- is required; the caller reports its operation's own
    /// result alongside. A call with no modification open is refused and changes nothing.
    fn end_modification(&mut self, end: ModificationEnd) -> CoordResult<()>;

    /// Records that recovery has completed: clears the fence and publishes a trusted even count.
    /// Only a completed recovery makes untrusted state trusted.
    ///
    /// # Errors
    ///
    /// Refused while a modification is open; a failure of the filesystem, which leaves recovery
    /// required.
    fn complete_recovery(&mut self) -> CoordResult<()>;

    /// Releases exclusion cleanly, reporting a failure to clear the ownership record.
    ///
    /// # Errors
    ///
    /// A failure to clear the record. Exclusion is released whatever the outcome.
    fn release(self: Box<Self>) -> CoordResult<()>;
}
