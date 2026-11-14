// Copyright (c) Michael Grier.

//! [`Admission`], [`Attachment`] and [`Upgrade`]: the admission gate and the attachment tiers
//! (canopy's DESIGN-NOTES.md, "The admission gate and the attachment tiers, modelled in
//! `Coordination.tla`").

use std::fmt;

use super::error::CoordResult;
use super::types::{Access, HeldTiers, ReaderSafety, Registration, Waiting};

/// The admission gate and the attachment tiers.
///
/// Every participant takes them in one order -- the gate, then the reader tier, then the writer
/// tier, then writer exclusion -- never one against that order, and never waits for the gate while
/// it holds a tier. Holding attachment blocks no read and no write, only an acquirer that wants a
/// tier exclusively. A holder's death releases everything it held.
pub trait Admission {
    /// Ordinary admission: takes the gate shared, then the reader tier shared, releases the gate,
    /// then takes the writer tier shared if `access` is [`Access::Write`], waiting as `waiting`
    /// allows at each step.
    ///
    /// Holders of the gate in shared mode never block one another, and the gate is held only until
    /// the reader tier is, so no participant waits behind another's wait for the gate. The caller's
    /// admission check -- whether it speaks admissible schemas -- is made after this returns, under
    /// the tiers, so an upgrade cannot complete between the check and the attachment; a participant
    /// that is not admitted drops the attachment.
    ///
    /// # Errors
    ///
    /// [`WouldWait`](super::CoordError::WouldWait) with [`Waiting::Never`] where a step would
    /// wait, and a failure of the filesystem, either of which leaves nothing held.
    fn attach(&self, access: Access, waiting: Waiting) -> CoordResult<Box<dyn Attachment>>;
}

/// Holding an entry in the registry of attached participants.
///
/// An entry lives exactly as long as what holds it: it is removed before the tiers are released,
/// so a listing never shows a participant that holds nothing.
pub trait Registrant {
    /// Registers, with `payload` -- what the caller wants a listing to show of it, such as its
    /// agent, the schemas it speaks and how to find its process -- which the interface stores and
    /// returns and never interprets. A participant registers after its admission check and before
    /// its attach returns to its own caller.
    ///
    /// A participant that finds every entry held is attached without one, and that is recorded so
    /// that every listing says it may be incomplete until the host restarts. Registering again
    /// replaces the payload, as when the caller's draining state changes.
    ///
    /// # Errors
    ///
    /// A failure of the filesystem, and [`WouldWait`](super::CoordError::WouldWait) when the entry
    /// held is being inspected so its payload cannot be replaced now; the entry keeps the payload it
    /// had. Registration is best-effort: a caller that cannot register stays attached.
    fn register(&mut self, payload: &[u8]) -> CoordResult<Registration>;
}

/// An ordinary attachment, held for a participant's life, and released when dropped -- its
/// registry entry first, then its tiers.
pub trait Attachment: Registrant + Send + fmt::Debug {
    /// What the attachment allows.
    fn access(&self) -> Access;

    /// The tiers held.
    fn tiers(&self) -> HeldTiers;

    /// Converts this attachment into an upgrade's: unregisters, releases the shared tiers, then
    /// takes the gate exclusively, then the reader tier exclusively if `safety` is
    /// [`ReaderSafety::Unsafe`], then the writer tier exclusively, waiting as `waiting` allows at
    /// each step.
    ///
    /// The tiers do not convert atomically, so the participant is detached for that interval, and
    /// it never waits for the gate while holding a tier. The gate admits one upgrader at a time.
    /// For a reader-safe upgrade, the gate is released once the writer tier is held exclusively,
    /// so readers attach while new writers wait on the writer tier; a reader-unsafe upgrade keeps
    /// it until it is dropped, since no reader could attach meanwhile. The caller checks the schemas
    /// again once this returns, since another upgrade may have finished while it waited.
    ///
    /// # Errors
    ///
    /// [`WouldWait`](super::CoordError::WouldWait) with [`Waiting::Never`] where a step would
    /// wait, and a failure of the filesystem. Either returns promptly with the participant
    /// detached, and reattaching is an ordinary [`Admission::attach`].
    fn upgrade(
        self: Box<Self>,
        safety: ReaderSafety,
        waiting: Waiting,
    ) -> CoordResult<Box<dyn Upgrade>>;
}

/// An upgrade's hold of the tiers, released when dropped -- its registry entry first -- which
/// leaves the participant detached.
pub trait Upgrade: Registrant + Send + fmt::Debug {
    /// Whether the upgrade is safe for readers.
    fn safety(&self) -> ReaderSafety;

    /// The tiers held.
    fn tiers(&self) -> HeldTiers;
}
