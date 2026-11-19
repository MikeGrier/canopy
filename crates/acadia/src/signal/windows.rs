// Copyright (c) Michael Grier.

//! [`EventSignal`]: a manual-reset event.

use std::io;
use std::os::windows::io::{AsHandle, BorrowedHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use win_sync_sys::{
    Event, ResetMode, SealedWaitOnlyEvent as RawSealedWaitOnlyEvent, WaitOnlyEvent, WaitOutcome,
    wait_for_single_object,
};
use windows_threadpool_sys::wait::{
    SealedThreadpoolWait, SealedWaitActivation, SealedWaitableHandle, ThreadpoolWait,
};

use super::{Callback, Registration, Signal, Watch, run_callback, sealed, wait_until_deadline};

#[cfg(test)]
mod sealed_tests;

/// A sealed wait capability whose reset mode is known to be manual.
#[derive(Clone)]
pub struct SealedWaitOnlyEvent {
    event: RawSealedWaitOnlyEvent,
}

impl std::fmt::Debug for SealedWaitOnlyEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedWaitOnlyEvent")
            .finish_non_exhaustive()
    }
}

impl SealedWaitOnlyEvent {
    pub fn wait_object(&self) -> win_sync_sys::WaitObject<'_> {
        self.event.wait_object()
    }

    /// Registers a Windows thread-pool wait without exposing the event handle or weakening its
    /// known manual-reset provenance.
    pub fn threadpool_wait<F>(self, callback: F) -> io::Result<SealedThreadpoolWait>
    where
        F: Fn(&SealedWaitActivation<'_>) + Send + Sync + 'static,
    {
        SealedThreadpoolWait::new(SealedWaitableHandle::from(self.event), callback, None)
    }
}

/// A [`Signal`] that is a Windows manual-reset event, a `win-sync-sys` [`Event`]: raised by
/// setting it, lowered by resetting it, and signalled to every wait while it is raised.
///
/// [`AsHandle`] gives the event to a platform wait -- `WaitForSingleObject`, a multiple-object
/// wait -- which sees exactly the level [`Signal::wait`] does. [`wait_only`](Self::wait_only)
/// gives it as a handle that can wait and cannot set or reset it, and [`event`](Self::event) as
/// an [`Event`], which can; a thread-pool wait takes either without `unsafe`.
#[derive(Debug)]
pub struct EventSignal {
    event: Event,
}

impl sealed::Sealed for EventSignal {}

impl EventSignal {
    /// The event, for a consumer that takes a `win-sync-sys` [`Event`], such as a thread-pool
    /// wait. A clone of it is the same event, so resetting one lowers this signal.
    pub fn event(&self) -> &Event {
        &self.event
    }

    /// A new handle to the event that can wait on it and cannot set or reset it: the kernel
    /// refuses both through it. It is what to lend a consumer that must not change the signal's
    /// level, such as a thread-pool wait a client registers.
    ///
    /// # Errors
    ///
    /// Returns the error from duplicating the handle.
    pub fn wait_only(&self) -> io::Result<WaitOnlyEvent> {
        self.event.duplicate_wait_only()
    }

    /// Duplicates the event into the published sealed type, whose handle is reachable only
    /// through an unsafe wait-only borrow. The owner retains its unsealed control operations.
    ///
    /// ```compile_fail
    /// use acadia::{EventSignal, Signal};
    /// use std::os::windows::io::{AsHandle, OwnedHandle};
    /// let wait = EventSignal::new().unwrap().sealed_wait_only().unwrap();
    /// let handle = wait.as_handle();
    /// ```
    ///
    /// ```compile_fail
    /// use acadia::{EventSignal, Signal};
    /// let wait = EventSignal::new().unwrap().sealed_wait_only().unwrap();
    /// let handle: std::os::windows::io::OwnedHandle = wait.into();
    /// ```
    ///
    /// ```compile_fail
    /// use acadia::{EventSignal, Signal};
    /// let wait = EventSignal::new().unwrap().sealed_wait_only().unwrap();
    /// let handle = wait.as_waitable_handle();
    /// ```
    ///
    /// ```compile_fail
    /// use windows_threadpool_sys::wait::SealedWaitActivation;
    /// fn extract(activation: &SealedWaitActivation<'_>) { let handle = activation.handle(); }
    /// ```
    ///
    /// ```compile_fail
    /// use windows_threadpool_sys::wait::SealedThreadpoolWait;
    /// fn extract(wait: SealedThreadpoolWait) { let handle = wait.into_handle(); }
    /// ```
    pub fn sealed_wait_only(&self) -> io::Result<SealedWaitOnlyEvent> {
        Ok(SealedWaitOnlyEvent {
            event: self.event.duplicate_sealed_wait_only()?,
        })
    }
}

impl Signal for EventSignal {
    fn new() -> io::Result<Self> {
        Ok(EventSignal {
            event: Event::new(ResetMode::Manual, false)?,
        })
    }

    fn raise(&self) -> io::Result<()> {
        self.event.set()
    }

    fn lower(&self) -> io::Result<()> {
        self.event.reset()
    }

    fn wait(&self, timeout: Duration) -> io::Result<bool> {
        wait_native(self, timeout)
    }

    fn watch<F>(&self, callback: F) -> io::Result<Watch>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let state = Arc::new(PoolState::default());
        let callback: Callback = Arc::new(callback);
        let wait = {
            let state = Arc::clone(&state);
            ThreadpoolWait::new(
                self.event.duplicate_wait_only()?.into(),
                move |activation| {
                    if state.stopped.load(Ordering::Acquire) {
                        return;
                    }
                    state.set_running(Some(thread::current().id()));
                    run_callback(&callback);
                    state.set_running(None);
                    // Armed again only after the callback returns, so runs never overlap; armed
                    // on a raised event, the wait completes at once, which is the level.
                    if !state.stopped.load(Ordering::Acquire) {
                        activation.rearm(None);
                    }
                },
                None,
            )?
        };
        wait.arm(None);
        Ok(Watch {
            _registration: Registration::Pool(PoolWatch {
                wait: Some(wait),
                state,
            }),
        })
    }
}

/// An [`EventSignal`]'s watch: a thread-pool wait on a wait-only handle to the event, which the
/// callback never sees.
pub(super) struct PoolWatch {
    wait: Option<ThreadpoolWait>,
    state: Arc<PoolState>,
}

#[derive(Default)]
struct PoolState {
    stopped: AtomicBool,
    /// The thread running the callback, if one is.
    running: Mutex<Option<ThreadId>>,
}

impl PoolState {
    fn set_running(&self, thread: Option<ThreadId>) {
        *self.running.lock().unwrap_or_else(PoisonError::into_inner) = thread;
    }
}

impl Drop for PoolWatch {
    fn drop(&mut self) {
        self.state.stopped.store(true, Ordering::Release);
        let Some(wait) = self.wait.take() else {
            return;
        };
        let current = thread::current().id();
        let from_own_callback = *self
            .state
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            == Some(current);
        if !from_own_callback {
            // Disarms, then waits for a running callback to return.
            drop(wait);
            return;
        }
        // Closing the wait waits for its callbacks, which from inside the callback would wait for
        // itself; so it is closed on a thread of its own once this run returns. It is passed
        // through a slot so that a thread that fails to start drops the slot and not the wait.
        let slot = Arc::new(Mutex::new(Some(wait)));
        let closer = Arc::clone(&slot);
        let started = thread::Builder::new()
            .name("acadia-signal-watch-close".into())
            .spawn(move || drop(closer.lock().unwrap_or_else(PoisonError::into_inner).take()));
        if started.is_err() {
            // No thread to close it on: with `stopped` set it is never armed again, so it is
            // left idle rather than closed.
            std::mem::forget(slot.lock().unwrap_or_else(PoisonError::into_inner).take());
        }
    }
}

/// The longest wait one call to the platform can name, one millisecond short of `INFINITE`; a
/// longer wait is made of several.
const LONGEST_PLATFORM_WAIT: Duration = Duration::from_millis(u32::MAX as u64 - 1);

/// A manual-reset event [`wait_native`] observes without changing it. Sealed to capabilities
/// known to come from a manual-reset event; a wait on an auto-reset event consumes its level.
pub trait NativeEvent: sealed::Sealed + AsHandle {}

impl NativeEvent for EventSignal {}

/// Waits with `WaitForSingleObject` until `event` is signalled or `timeout` has passed, and
/// reports whether it was signalled: the platform's own wait, for a caller that holds an event a
/// signal lends. Like [`Signal::wait`], it never reports `false` before `timeout` has passed.
///
/// An auto-reset event is refused, since waiting consumes its level:
///
/// ```compile_fail,E0277
/// use win_sync_sys::{Event, ResetMode};
/// let event = Event::new(ResetMode::Auto, true).unwrap();
/// let _ = acadia::wait_native(&event, std::time::Duration::ZERO);
/// ```
pub fn wait_native(event: &impl NativeEvent, timeout: Duration) -> io::Result<bool> {
    let handle = event.as_handle();
    wait_until_deadline(timeout, |remaining| {
        match wait_for_single_object(handle, Some(remaining.min(LONGEST_PLATFORM_WAIT)))? {
            WaitOutcome::Signalled => Ok(true),
            WaitOutcome::TimedOut => Ok(false),
            other => Err(io::Error::other(format!(
                "a wait on an event ended as {other:?}"
            ))),
        }
    })
}

/// Waits on a capability made from an [`EventSignal`]'s manual-reset event, without extracting
/// its handle or changing its level. As with [`Signal::wait`], a timeout is never reported before
/// its bound passes.
///
/// A reset-mode-blind capability from `win_sync_sys` is refused, because a wait on an auto-reset
/// event consumes its level:
///
/// ```compile_fail,E0308
/// use win_sync_sys::{Event, ResetMode};
/// let event = Event::new(ResetMode::Auto, true).unwrap();
/// let wait = event.duplicate_sealed_wait_only().unwrap();
/// let _ = acadia::wait_sealed(&wait, std::time::Duration::ZERO);
/// ```
pub fn wait_sealed(event: &SealedWaitOnlyEvent, timeout: Duration) -> io::Result<bool> {
    wait_until_deadline(timeout, |remaining| {
        match wait_for_single_object(&event.event, Some(remaining.min(LONGEST_PLATFORM_WAIT)))? {
            WaitOutcome::Signalled => Ok(true),
            WaitOutcome::TimedOut => Ok(false),
            other => Err(io::Error::other(format!(
                "a sealed event wait ended as {other:?}"
            ))),
        }
    })
}

impl AsHandle for EventSignal {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.event.as_handle()
    }
}
