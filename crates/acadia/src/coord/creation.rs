// Copyright (c) Michael Grier.

//! [`Creation`]: creation exclusion and the creation reservation (canopy's DESIGN-NOTES.md,
//! "Creating a store root, modelled in `Creation.tla`").

use std::ffi::OsStr;

use super::error::CoordResult;
use super::types::{Created, CreationNonce};

/// What a creation makes: the caller's inputs, the creator's nonce, and the artifacts in order.
#[derive(Debug, Clone, Copy)]
pub struct CreationPlan<'a> {
    /// Every creation input, encoded canonically by the caller, so that identical inputs are
    /// identical bytes. The interface records them and compares them and never interprets them.
    pub inputs: &'a [u8],
    /// This creator's nonce, recorded if it publishes the reservation.
    pub nonce: CreationNonce,
    /// The names, directly beneath the root, of what creation makes, in the order it makes them;
    /// the last is the marker. None begins with [`RESERVED_PREFIX`](super::RESERVED_PREFIX), which
    /// names the coordination's own entries.
    pub artifacts: &'a [&'a OsStr],
}

/// The caller's half of creation: making each artifact.
pub trait ArtifactMaker {
    /// Makes the artifact `name` from `inputs`, and makes it and everything in it durable before
    /// returning; the coordination makes its directory entry durable.
    ///
    /// It may find the artifact partly made by a creation that was interrupted, and finishes it:
    /// making an artifact is idempotent.
    ///
    /// # Errors
    ///
    /// Any failure ends the creation, which a later call resumes.
    fn make(&mut self, name: &OsStr, inputs: &[u8]) -> CoordResult<()>;
}

/// Creation exclusion and the creation reservation.
///
/// Within one implementation, creating a root excludes every other creator and opener of it until
/// its marker is in place. Between implementations, whose creation exclusions need not exclude each
/// other, the reservation arbitrates: whoever publishes it owns the creation.
pub trait Creation {
    /// Creates the store root, or resumes a creation of it that was interrupted.
    ///
    /// The root is read: a marker refuses with
    /// [`AlreadyCreated`](super::CoordError::AlreadyCreated); anything beneath it that is neither an
    /// artifact of `plan` nor the coordination's own -- or an artifact with no reservation, which no
    /// creation leaves -- refuses with [`Occupied`](super::CoordError::Occupied); a reservation that names another implementation or
    /// records other inputs refuses with [`Reserved`](super::CoordError::Reserved). Then this
    /// implementation's creation exclusion is taken, waiting for it, and the root read again under
    /// it with the same refusals, so a creation that finished while this call waited is found.
    ///
    /// With no reservation, one is published -- this implementation, `plan.inputs` and
    /// `plan.nonce` -- complete or not at all even across a power loss, and made durable with its
    /// directory entry before any artifact exists; a reservation published first by another
    /// creator refuses this one. With a reservation this call may resume, it is made durable in
    /// case its creator died first. Each artifact is then made through `maker`, in order, and made
    /// durable before the next, the marker last. The exclusion is released once the marker is
    /// durable.
    ///
    /// # Errors
    ///
    /// [`InvalidPlan`](super::CoordError::InvalidPlan) for a plan with no artifacts or with a name
    /// that is not one plain name or has the reserved prefix; the refusals above, which change
    /// nothing in the root; a failure of `maker` or of the filesystem, which leaves a creation a
    /// later call resumes.
    fn create(
        &self,
        plan: &CreationPlan<'_>,
        maker: &mut dyn ArtifactMaker,
    ) -> CoordResult<Created>;

    /// Confirms that the root is a store this implementation coordinates, before an open relies on
    /// it: the reservation must name this implementation, and the marker -- the last of the
    /// artifacts, named by `marker` -- must exist, and is made durable with its directory entry,
    /// since its creator may have died between publishing it and making it durable.
    ///
    /// It waits for this implementation's creation exclusion, shared with other openers, so an open
    /// that meets a creation in progress waits for it to finish or die.
    ///
    /// # Errors
    ///
    /// [`ImplementationMismatch`](super::CoordError::ImplementationMismatch) for a root another
    /// implementation coordinates; [`NotCreated`](super::CoordError::NotCreated) for one with no
    /// marker, a creation that was never finished included, or with no reservation, which no
    /// creation through this interface leaves; [`InvalidPlan`](super::CoordError::InvalidPlan) for a
    /// marker name that could not be an artifact.
    fn confirm_created(&self, marker: &OsStr) -> CoordResult<()>;
}
