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
//!
//! [`Signal::watch`] waits on a thread pool without lending the object: acadia registers its own
//! object and runs the caller's callback each time it finds the signal raised.

use std::fmt;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
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

    /// Runs `callback` on a thread of the platform's thread pool each time the signal is found
    /// raised, until the returned [`Watch`] is dropped.
    ///
    /// The signal is a level, and so is the watch: a callback that returns with the signal still
    /// raised is run again at once, so a callback lowers the signal before it looks for the work
    /// the raise announced. Runs of one watch never overlap. The watch holds the signal's object
    /// open, and it is acadia that waits on it, so the object never reaches `callback`.
    ///
    /// On Windows the pool is the process's default thread pool; on Linux it is a pool of
    /// threads acadia starts on first use, waiting on one `epoll` set; for [`CondvarSignal`] it is
    /// a thread of the watch's own. A callback must return promptly, since it holds a thread the
    /// pool's other work may need, and must not panic: a panic aborts the process.
    ///
    /// # Errors
    ///
    /// Returns the error from making the registration, or from starting a thread for it.
    fn watch<F>(&self, callback: F) -> io::Result<Watch>
    where
        F: Fn() + Send + Sync + 'static;
}

/// A registration made by [`Signal::watch`], which runs its callback until it is dropped.
///
/// Dropping it stops the callback from being run again and waits for a run in progress to
/// return. Dropped from within its own callback, it cannot wait for that run; it returns at
/// once, and the callback is not run again after the run returns.
#[must_use = "dropping a `Watch` stops it"]
pub struct Watch {
    _registration: Registration,
}

#[expect(
    dead_code,
    reason = "each registration is held only for its `Drop`, which stops it"
)]
enum Registration {
    Thread(ThreadWatch),
    #[cfg(windows)]
    Pool(windows::PoolWatch),
    #[cfg(target_os = "linux")]
    Epoll(linux::EpollWatch),
}

impl fmt::Debug for Watch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Watch").finish_non_exhaustive()
    }
}

/// A watch's callback, shared with whatever runs it.
type Callback = Arc<dyn Fn() + Send + Sync>;

/// Runs a watch's callback, aborting the process if it panics, which is what the Windows thread
/// pool does and so what every platform does.
fn run_callback(callback: &Callback) {
    if catch_unwind(AssertUnwindSafe(|| callback())).is_err() {
        std::process::abort();
    }
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
    // Shared with the threads of its watches.
    level: Arc<Level>,
}

#[derive(Debug, Default)]
struct Level {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl sealed::Sealed for CondvarSignal {}

impl Signal for CondvarSignal {
    fn new() -> io::Result<Self> {
        Ok(Self::default())
    }

    fn raise(&self) -> io::Result<()> {
        *self
            .level
            .raised
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = true;
        self.level.changed.notify_all();
        Ok(())
    }

    fn lower(&self) -> io::Result<()> {
        *self
            .level
            .raised
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = false;
        Ok(())
    }

    fn wait(&self, timeout: Duration) -> io::Result<bool> {
        let raised = self
            .level
            .raised
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (raised, _) = self
            .level
            .changed
            .wait_timeout_while(raised, timeout, |raised| !*raised)
            .unwrap_or_else(PoisonError::into_inner);
        Ok(*raised)
    }

    fn watch<F>(&self, callback: F) -> io::Result<Watch>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let level = Arc::clone(&self.level);
        let stopped = Arc::new(AtomicBool::new(false));
        let callback: Callback = Arc::new(callback);
        let thread = {
            let stopped = Arc::clone(&stopped);
            thread::Builder::new()
                .name("acadia-signal-watch".into())
                .spawn(move || {
                    loop {
                        {
                            let raised =
                                level.raised.lock().unwrap_or_else(PoisonError::into_inner);
                            let _raised = level
                                .changed
                                .wait_while(raised, |raised| {
                                    !*raised && !stopped.load(Ordering::SeqCst)
                                })
                                .unwrap_or_else(PoisonError::into_inner);
                        }
                        if stopped.load(Ordering::SeqCst) {
                            return;
                        }
                        run_callback(&callback);
                    }
                })?
        };
        Ok(Watch {
            _registration: Registration::Thread(ThreadWatch {
                level: Arc::clone(&self.level),
                stopped,
                thread: Some(thread),
            }),
        })
    }
}

/// A [`CondvarSignal`]'s watch: a thread of its own, waiting on the signal's condition variable.
struct ThreadWatch {
    level: Arc<Level>,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ThreadWatch {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        // Taken under the lock so the thread, which tests `stopped` under it, cannot miss the
        // wake between its test and its wait.
        drop(
            self.level
                .raised
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        self.level.changed.notify_all();
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != thread::current().id()
        {
            // A panicking callback has already aborted the process, so the join cannot fail.
            let _ = thread.join();
        }
    }
}
