// Copyright (c) Michael Grier.

//! [`Registry`]: listing the attached participants (canopy's DESIGN-NOTES.md, "The registry of
//! attached handles, modelled in `Coordination.tla`").

use super::error::CoordResult;
use super::types::Listing;

/// Listing the registry of attached participants, for diagnosis.
///
/// Every change to an entry and every inspection of one is made under that entry's claim lock,
/// taken without waiting; an entry's liveness lock is held by its registrant for as long as the
/// registration lasts. A registrant holds the claim lock from before it takes the liveness lock
/// until its token is written, so no listing sees a new owner's lock beside an old owner's token.
/// No participant ever waits for a registry lock.
pub trait Registry {
    /// Lists the entries live now.
    ///
    /// It needs no attachment and takes no tier, so a participant can see who blocks an upgrade
    /// even while the upgrade holds the gate. An entry is reported live only if its liveness lock
    /// was held, by the registration its token names, when probed under its claim lock; a dead
    /// participant's entry is never reported live. An entry being claimed by someone else is
    /// counted unknown.
    ///
    /// # Errors
    ///
    /// A failure of the filesystem.
    fn list(&self) -> CoordResult<Listing>;
}
