// Copyright (c) Michael Grier.

//! [`Doorbell`]: the protocol of `spec/Doorbell.tla`, over a [`Signal`].
//!
//! Each step of the spec is one atomic operation of this code, named beside it. TLA+ steps are
//! sequentially consistent, and the code realizes them with acquire and release alone, because the
//! two sides meet on one location: a producer's `Test` and the consumer's `Clear` are both
//! read-modify-writes of the record, so one comes after the other. A `Test` that comes after reads
//! the record clear and raises the signal; a `Clear` that comes after acquires the producer's
//! publish, which the producer's `Test` released, so the consumer's check finds it. Were the clear a
//! store, each side would store and then load what the other stores, which acquire and release do
//! not order.

use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use acadia::{PlatformSignal, Signal, Watch};

#[cfg(all(canopy_loom, test))]
use super::sync::Weakening;
use super::sync::{AtomicBool, Ordering};

/// Wakes a consumer when producers publish work, with no system call for a ring while it is
/// already rung and none per batch for a consumer kept busy.
///
/// It owns no queue. A producer publishes into its own queue and then calls
/// [`ring`](Doorbell::ring); the consumer takes from the queue and calls
/// [`rearm`](Doorbell::rearm), [`wait`](Doorbell::wait), or [`pump`](Doorbell::pump), which runs
/// the whole of the consumer's side over a `take` it is given. What it guarantees is
/// `spec/DoorbellContract.tla`: every item published is taken, and a consumer waiting while
/// work is there is woken; a wake with nothing to take may happen.
///
/// A consumer on the platform's thread pool runs its side with [`watch`](Doorbell::watch), which
/// waits on the signal without lending it. Nothing but the doorbell changes its signal, and no safe
/// method lends the platform's object behind it, through which safe code could. A consumer's own
/// event loop -- a multiple-object wait, `epoll` -- takes the object from an `unsafe` accessor
/// whose contract is to only wait on it, and then runs its side as `pump` with no wait.
#[derive(Debug)]
pub struct Doorbell<S: Signal = PlatformSignal> {
    /// The record of being rung: set by the ring that raises the signal, cleared by the
    /// consumer after it has lowered the signal.
    rung: AtomicBool,
    signal: S,
    /// An ordering a loom check has weakened, to show that the check fails without it.
    #[cfg(all(canopy_loom, test))]
    weakened: Option<Weakening>,
}

impl<S: Signal> Doorbell<S> {
    /// A doorbell that has not been rung.
    pub fn new() -> io::Result<Self> {
        Ok(Doorbell {
            rung: AtomicBool::new(false),
            signal: S::new()?,
            #[cfg(all(canopy_loom, test))]
            weakened: None,
        })
    }

    /// A doorbell with one of its orderings weakened, for a loom check that must fail.
    #[cfg(all(canopy_loom, test))]
    pub(crate) fn weakened(weakened: Weakening) -> io::Result<Self> {
        Ok(Doorbell {
            weakened: Some(weakened),
            ..Self::new()?
        })
    }

    /// Rings: called by a producer after it has published its work.
    ///
    /// A ring while the doorbell is already rung only records it; the first since the consumer
    /// last rearmed raises the signal.
    pub fn ring(&self) -> io::Result<()> {
        if self.test() {
            self.raise()?;
        }
        Ok(())
    }

    /// `Test(p)`: sets the record and reports whether it was clear, so the signal must be raised.
    pub(crate) fn test(&self) -> bool {
        // Release carries the caller's publish to the clear that reads this; acquire orders a raise
        // after the reset of the consumer whose clear this reads.
        #[cfg(all(canopy_loom, test))]
        if self.weakened == Some(Weakening::TestWithoutRelease) {
            return !self.rung.swap(true, Ordering::Acquire);
        }
        !self.rung.swap(true, Ordering::AcqRel)
    }

    /// Whether the record is set, for a test to read a doorbell's state.
    #[cfg(all(test, not(canopy_loom)))]
    pub(crate) fn is_rung(&self) -> bool {
        self.rung.load(Ordering::Acquire)
    }

    /// `Raise(p)`.
    pub(crate) fn raise(&self) -> io::Result<()> {
        self.signal.raise()
    }

    /// Rearms: called by the consumer after a take, before it checks the queue again.
    ///
    /// It lowers the signal, then clears the record. The order is the protocol's: cleared
    /// first, a ring whose work was already taken could set the record again and then have its
    /// raise lowered, leaving the doorbell rung with the signal low and every later ring silent.
    pub fn rearm(&self) -> io::Result<()> {
        self.reset()?;
        self.clear();
        Ok(())
    }

    /// `Reset`.
    pub(crate) fn reset(&self) -> io::Result<()> {
        self.signal.lower()
    }

    /// `Clear`.
    pub(crate) fn clear(&self) {
        // A read-modify-write, not a store, so that it is ordered with every producer's `Test`.
        // Acquire brings the publish of a `Test` it reads to the caller's check; release orders the
        // reset before a `Test` that reads this.
        #[cfg(all(canopy_loom, test))]
        match self.weakened {
            Some(Weakening::ClearWithoutAcquire) => {
                self.rung.swap(false, Ordering::Release);
                return;
            }
            Some(Weakening::ClearAsStore) => {
                self.rung.store(false, Ordering::Release);
                return;
            }
            _ => {}
        }
        self.rung.swap(false, Ordering::AcqRel);
    }

    /// `Wake`, or the bound passing: waits until the doorbell is rung or `timeout` has passed,
    /// and reports whether it was rung. Called by the consumer after a check found nothing.
    pub fn wait(&self, timeout: Duration) -> io::Result<bool> {
        self.signal.wait(timeout)
    }

    /// Runs the consumer's side until `take` has found work or `timeout` has passed, and
    /// reports whether it found any.
    ///
    /// `take` takes everything its queue holds and reports whether there was anything. A pump
    /// takes, rearms, takes again until a take finds nothing, and returns if any take found
    /// work; otherwise it waits for a ring and goes round again. A `timeout` of zero never
    /// waits, which is the pump a consumer runs when a platform wait has already seen the
    /// doorbell rung.
    pub fn pump(&self, timeout: Duration, mut take: impl FnMut() -> bool) -> io::Result<bool> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            // `Take`.
            let mut took = take();
            // `Reset`, `Clear`.
            self.rearm()?;
            // `Check`, and `Retake` until a check finds nothing.
            while take() {
                took = true;
            }
            if took {
                return Ok(true);
            }
            let remaining = deadline.map_or(Duration::MAX, |d| {
                d.saturating_duration_since(Instant::now())
            });
            // `Wake`, or the bound passing.
            if remaining.is_zero() || !self.wait(remaining)? {
                return Ok(false);
            }
        }
    }
}

impl<S: Signal + 'static> Doorbell<S> {
    /// Runs the consumer's side on the platform's thread pool until the returned [`Watch`] is
    /// dropped: each time the doorbell is found rung, a pump that does not wait, over `take`.
    ///
    /// `take` is what [`pump`](Doorbell::pump) takes. Its runs never overlap, and it holds a
    /// thread of the pool while it runs, so it returns promptly. A pump that fails -- a signal
    /// that could not be lowered -- is reported to `failed` and the watch carries on, so a later
    /// ring tries again. Neither may panic: a panic on the pool aborts the process.
    ///
    /// # Errors
    ///
    /// Returns the error from making the registration with the thread pool.
    pub fn watch<T, F>(self: &Arc<Self>, take: T, failed: F) -> io::Result<Watch>
    where
        T: FnMut() -> bool + Send + 'static,
        F: Fn(io::Error) + Send + Sync + 'static,
    {
        let doorbell = Arc::clone(self);
        // Never contended: the runs of one watch never overlap.
        let take = Mutex::new(take);
        self.signal.watch(move || {
            let mut take = take.lock().unwrap_or_else(PoisonError::into_inner);
            if let Err(error) = doorbell.pump(Duration::ZERO, &mut *take) {
                failed(error);
            }
        })
    }
}

#[cfg(windows)]
impl Doorbell<acadia::EventSignal> {
    /// A new handle to the doorbell's event that can wait on it and cannot set or reset it, for a
    /// consumer's own native wait: a thread-pool wait, which takes it without further `unsafe`, or
    /// a multiple-object wait.
    ///
    /// The wait sees the signal's level, so a thread-pool callback that re-arms its wait before
    /// the consumer has rearmed the doorbell is called again at once.
    ///
    /// # Safety
    ///
    /// The caller must only wait on the handle. It must not change the event's level by any
    /// means -- through this handle, a duplicate of it, or anything else it can reach from it --
    /// and must keep any code it hands the handle to from doing so. A set or
    /// reset outside the doorbell's protocol can leave the doorbell rung with its signal low, so
    /// that no ring wakes the consumer; the protocol, and the model it is checked against,
    /// assume nothing but the doorbell changes the signal.
    ///
    /// # Errors
    ///
    /// Returns the error from duplicating the handle.
    pub unsafe fn native_wait_handle(&self) -> io::Result<acadia::WaitOnlyEvent> {
        self.signal.wait_only()
    }
}

#[cfg(target_os = "linux")]
impl Doorbell<acadia::EventFdSignal> {
    /// The doorbell's `eventfd`, for a consumer's own native wait: an `epoll` set or `poll`.
    ///
    /// The descriptor is readable while the doorbell's signal is raised, so a level-triggered
    /// registration reports it again until the consumer has rearmed the doorbell.
    ///
    /// # Safety
    ///
    /// The caller must only wait for the descriptor to be readable. It must not read from it or
    /// write to it by any means -- through this borrow, a duplicate, or a `File` made from one --
    /// and must keep any code it hands the descriptor to from doing so: a read lowers the signal
    /// and a write raises it, and a change outside the doorbell's protocol can leave the doorbell
    /// rung with its signal low, so that no ring wakes the consumer. The protocol, and the model
    /// it is checked against, assume nothing but the doorbell changes the signal.
    pub unsafe fn native_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        use std::os::fd::AsFd;
        self.signal.as_fd()
    }
}
