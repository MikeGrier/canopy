// Copyright (c) Michael Grier.

//! [`ModificationCount`] and [`CountNotification`]: sampling the modification count, and being
//! told when it moves (canopy's DESIGN-NOTES.md, "The modification count, the generation and
//! reads, modelled in `Coordination.tla`", and "A change to the coordination state is delivered as
//! a notification").

use std::fmt;
use std::time::Instant;

use super::error::CoordResult;
use super::types::ModCount;

/// Sampling the modification count, which only the holder of writer exclusion raises, through its
/// [`ExclusionHold`](super::ExclusionHold).
pub trait ModificationCount {
    /// Samples the published count: one read of a value published atomically, taking no lock and
    /// waiting for no writer. A participant whose first use of the coordination state this is
    /// first makes that state its own, which can wait for another participant doing the same --
    /// never for a writer -- and takes no lock in any later sample.
    ///
    /// A sample is never assembled from parts published separately, and never stale: a sample
    /// taken after observing any filesystem effect of a modification reflects that modification's
    /// opening raise, and a cached value is invalidated synchronously with the publication that
    /// outdates it. State that cannot be vouched for samples odd.
    ///
    /// # Errors
    ///
    /// A failure of the filesystem.
    fn sample(&self) -> CoordResult<ModCount>;
}

/// A notification's registration, withdrawn when it is dropped.
///
/// Like [`VfsLock`](crate::VfsLock) it has no methods: its whole behaviour is its lifetime.
pub trait Notice: Send + Sync + fmt::Debug {}

/// Being told when the modification count moves, so that work waiting on a change holds no thread.
pub trait CountNotification {
    /// Runs `ring` once when the notification mechanism decides the caller should sample, unless
    /// the returned [`Notice`] is dropped first. A changed published count and the deadline are
    /// required triggers; an implementation may also deliver earlier hints, including a native
    /// event whose cause is not a count publication by this process.
    ///
    /// A notification is a hint: the caller samples again and decides from what it reads, so one
    /// that comes early or late costs time and never correctness, and the deadline bounds what a
    /// lost one costs. A count that cannot be sampled rings, so the caller meets the failure when it
    /// samples again. How the change is noticed is the implementation's -- a poller per process
    /// sampling with backoff only while a registration is outstanding, or a notification from the
    /// operating system -- and in neither does the registrant hold a thread. `ring` runs on a thread
    /// of the implementation's and must return promptly. Under `panic = "unwind"`, a panic in one
    /// ring does not stop others. Under `panic = "abort"`, the panic ends the process as the binary's
    /// panic strategy requires.
    ///
    /// # Errors
    ///
    /// A failure to make the registration.
    fn notify(
        &self,
        sampled: ModCount,
        deadline: Instant,
        ring: Box<dyn FnOnce() + Send>,
    ) -> CoordResult<Box<dyn Notice>>;
}
