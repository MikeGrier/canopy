// Copyright (c) Michael Grier.

//! [`RecoveryDiscovery`]: whether recovery is required, and why (canopy's DESIGN-NOTES.md, "Recovery
//! and its discovery, modelled in `Coordination.tla`").

use super::error::CoordResult;
use super::types::RecoveryNeed;

/// Asking whether recovery is required, without being a holder of writer exclusion.
pub trait RecoveryDiscovery {
    /// Observes the fence, the restart state, the ownership record and whether writer exclusion is
    /// held, as one observation, and reports whether recovery is required and why.
    ///
    /// It reports abandoned work and never work in progress. Recovery is required when the fence is
    /// set, when the state cannot be vouched for, or when an ownership record names a holder and
    /// exclusion is free, which can only mean the holder died. A record beside held exclusion is a
    /// live holder's transaction and is not reported. A recovery in progress is reported, since the
    /// fence stays set until it finishes.
    ///
    /// Any participant may ask, one that holds nothing and one that holds exclusion itself included;
    /// a participant that holds exclusion is a live holder to itself, so its own record is not
    /// reported, and its fence and restart state are.
    ///
    /// It changes nothing in the coordination state: it does not clear a fence it finds set, remake
    /// state that cannot be vouched for or touch a record. An implementation that has to take
    /// exclusion for an instant to make its observation consistent releases it without the
    /// bookkeeping a release of a hold does.
    ///
    /// # Errors
    ///
    /// A failure of the filesystem or of the platform's boot identity.
    fn discover_recovery(&self) -> CoordResult<RecoveryNeed> {
        self.discover_recovery_with(&mut || {})
    }

    /// Calls `inspect` only when exclusion is free, while preventing a writer from acquiring it.
    /// The inspection may read caller-owned recovery records but must not mutate or acquire exclusion.
    /// An abandonment of this participant waits for the inspection to return, so the inspection must
    /// not call into this participant.
    fn discover_recovery_with(&self, inspect: &mut dyn FnMut()) -> CoordResult<RecoveryNeed>;
}
