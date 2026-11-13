// Copyright (c) Michael Grier.

//! `Signal`: the one platform primitive of canopy's ring, a level that is raised, lowered and
//! waited on (canopy's design notes, "The ring: a portable protocol over one platform
//! primitive, behind a contract spec").
//!
//! A signal is a **level**, not an event count: raising a raised signal and lowering a lowered
//! one change nothing and report nothing, and a wait returns as soon as the signal is raised,
//! whenever it was raised, without lowering it. Everything that makes a doorbell correct --
//! which side raises, when it is lowered, the fences around it -- is the protocol above this,
//! written once for every implementation.
//!
//! [`PlatformSignal`] names the platform's implementation: `EventSignal`, a manual-reset
//! event, on Windows; `EventFdSignal`, an `eventfd`, on Linux; and [`CondvarSignal`], a mutex
//! and condition variable, elsewhere. `CondvarSignal` exists on every target, for tests and for
//! checkers that cannot run a kernel object. The Windows and Linux signals expose the object a
//! platform wait takes, through `AsHandle` and `AsFd`, so a caller can wait for one with a
//! thread pool, a multiple-object wait or `epoll`.

use std::fmt;
use std::io;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{EventFdSignal, wait_native};
#[cfg(windows)]
pub use windows::{EventSignal, wait_native};

/// The platform's signal, chosen by `cfg`.
#[cfg(windows)]
pub type PlatformSignal = EventSignal;
/// The platform's signal, chosen by `cfg`.
#[cfg(target_os = "linux")]
pub type PlatformSignal = EventFdSignal;
/// The platform's signal, chosen by `cfg`.
#[cfg(not(any(windows, target_os = "linux")))]
pub type PlatformSignal = CondvarSignal;

mod sealed {
    pub trait Sealed {}
}

/// A level that is raised, lowered and waited on.
///
/// Sealed: the implementations are this crate's, so a protocol written over `Signal` is argued
/// against these three and no other.
pub trait Signal: sealed::Sealed + Send + Sync + fmt::Debug + Sized {
    /// A new signal, lowered.
    fn new() -> io::Result<Self>;

    /// Raises the signal, waking every waiter. Raising a raised signal changes nothing.
    fn raise(&self) -> io::Result<()>;

    /// Lowers the signal. Lowering a lowered signal changes nothing.
    fn lower(&self) -> io::Result<()>;

    /// Waits until the signal is raised or `timeout` has passed, and reports whether it was
    /// raised. It returns at once if the signal is already raised, and never lowers it.
    ///
    /// It does not report `false` before `timeout` has passed, though it may return later than
    /// that by the platform's scheduling. A `timeout` too long for the platform to name is
    /// waited in parts.
    fn wait(&self, timeout: Duration) -> io::Result<bool>;
}

/// Waits in parts with `wait_once`, which takes the time still to wait and may return before it
/// has passed, until the signal is raised or `timeout` has passed.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
fn wait_until_deadline(
    timeout: Duration,
    mut wait_once: impl FnMut(Duration) -> io::Result<bool>,
) -> io::Result<bool> {
    let deadline = Instant::now().checked_add(timeout);
    loop {
        let remaining = deadline.map_or(Duration::MAX, |d| {
            d.saturating_duration_since(Instant::now())
        });
        if wait_once(remaining)? {
            return Ok(true);
        }
        if remaining.is_zero() {
            return Ok(false);
        }
    }
}

/// `duration` in whole milliseconds, rounded up so a wait never ends before it, and capped at
/// `max`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn millis_rounded_up(duration: Duration, max: u64) -> u64 {
    let millis =
        duration.as_millis() + u128::from(!duration.subsec_nanos().is_multiple_of(1_000_000));
    u64::try_from(millis).unwrap_or(u64::MAX).min(max)
}

/// A signal made of a mutex and a condition variable, on every target.
#[derive(Debug, Default)]
pub struct CondvarSignal {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl sealed::Sealed for CondvarSignal {}

impl Signal for CondvarSignal {
    fn new() -> io::Result<Self> {
        Ok(Self::default())
    }

    fn raise(&self) -> io::Result<()> {
        *self.raised.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
        Ok(())
    }

    fn lower(&self) -> io::Result<()> {
        *self.raised.lock().unwrap_or_else(PoisonError::into_inner) = false;
        Ok(())
    }

    fn wait(&self, timeout: Duration) -> io::Result<bool> {
        let raised = self.raised.lock().unwrap_or_else(PoisonError::into_inner);
        let (raised, _) = self
            .changed
            .wait_timeout_while(raised, timeout, |raised| !*raised)
            .unwrap_or_else(PoisonError::into_inner);
        Ok(*raised)
    }
}
