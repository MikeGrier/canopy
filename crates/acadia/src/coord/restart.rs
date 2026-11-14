// Copyright (c) Michael Grier.

//! [`RestartRecognition`]: whether the coordination state can be vouched for.

use super::error::CoordResult;
use super::types::Trust;

/// Whether the coordination state has been maintained continuously.
///
/// The state survives the death of any participant. It is never trusted across a host restart,
/// which the implementation recognises from a boot identity the operating system provides, nor when
/// it was made, or made again -- a shared mapping vanishes with its last user -- until a recovery
/// completes on it. State that cannot be vouched for reads as odd to every sampler, and requires
/// recovery of the first acquirer of writer exclusion.
pub trait RestartRecognition {
    /// Whether the coordination state can be vouched for now.
    ///
    /// It changes nothing: state found to need making again is made by the operation that relies
    /// on it, untrusted.
    ///
    /// # Errors
    ///
    /// A failure of the filesystem or of the platform's boot identity.
    fn trust(&self) -> CoordResult<Trust>;
}
