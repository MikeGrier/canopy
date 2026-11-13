// Copyright (c) Michael Grier.

//! The atomics the ring is written with: the standard library's, or loom's under
//! `--cfg canopy_loom`, so that loom, a checker of the Rust memory model, runs the ring's own code.

#[cfg(canopy_loom)]
pub(crate) use loom::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(canopy_loom))]
pub(crate) use std::sync::atomic::{AtomicBool, Ordering};

/// An ordering of the doorbell weakened, so that a loom check can show the doorbell needs it.
#[cfg(all(canopy_loom, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Weakening {
    /// A producer's `Test` acquires and does not release.
    TestWithoutRelease,
    /// The consumer's `Clear` releases and does not acquire.
    ClearWithoutAcquire,
    /// The consumer's `Clear` is a releasing store, not a read-modify-write.
    ClearAsStore,
}
