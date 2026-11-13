// Copyright (c) Michael Grier.

//! [`Doorbell`]: the protocol of `spec/Doorbell.tla`, over a [`Signal`].
//!
//! Each step of the spec is one atomic operation or fence of this code, named beside it. TLA+
//! steps are sequentially consistent, which the code realizes with a sequentially consistent
//! fence between a producer's publish and its test of the record, and between the consumer's
//! clear of the record and its check: the two sides each store, then load what the other
//! stores, so without both fences each could miss the other's store.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering, fence};
use std::time::{Duration, Instant};

use acadia::{PlatformSignal, Signal};

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
/// On Windows a doorbell over the platform's signal lends its event through `AsHandle`, and on
/// Linux its descriptor through `AsFd`, so a consumer can wait for it with a thread pool, a
/// multiple-object wait or `epoll` and then run its side as `pump` with no wait.
#[derive(Debug)]
pub struct Doorbell<S: Signal = PlatformSignal> {
    /// The record of being rung: set by the ring that raises the signal, cleared by the
    /// consumer after it has lowered the signal.
    rung: AtomicBool,
    signal: S,
}

impl<S: Signal> Doorbell<S> {
    /// A doorbell that has not been rung.
    pub fn new() -> io::Result<Self> {
        Ok(Doorbell {
            rung: AtomicBool::new(false),
            signal: S::new()?,
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
        // Orders the caller's publish before the load of the record.
        fence(Ordering::SeqCst);
        !self.rung.swap(true, Ordering::SeqCst)
    }

    /// Whether the record is set, for a test to read a doorbell's state.
    #[cfg(test)]
    pub(crate) fn is_rung(&self) -> bool {
        self.rung.load(Ordering::SeqCst)
    }

    /// The signal, for a test to register with a platform's wait. Kept from callers: lowering it
    /// outside the protocol can leave the doorbell rung with its signal low, so no ring wakes.
    #[cfg(all(test, windows))]
    pub(crate) fn signal(&self) -> &S {
        &self.signal
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
        self.rung.store(false, Ordering::SeqCst);
        // Orders the clear before the caller's check of the queue.
        fence(Ordering::SeqCst);
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

#[cfg(windows)]
impl std::os::windows::io::AsHandle for Doorbell<acadia::EventSignal> {
    fn as_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        self.signal.as_handle()
    }
}

#[cfg(target_os = "linux")]
impl std::os::fd::AsFd for Doorbell<acadia::EventFdSignal> {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.signal.as_fd()
    }
}
