// Copyright (c) Michael Grier.

//! [`EventSignal`]: a manual-reset event.

use std::io;
use std::os::windows::io::{AsHandle, BorrowedHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use win_sync_sys::{Event, ResetMode, WaitOnlyEvent, WaitOutcome, wait_for_single_object};
use windows_threadpool_sys::wait::ThreadpoolWait;

use super::{Callback, Registration, Signal, Watch, run_callback, sealed, wait_until_deadline};

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
        wait_native(self.event.as_handle(), timeout)
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

/// Waits with `WaitForSingleObject` until `handle` is signalled or `timeout` has passed, and
/// reports whether it was signalled: the platform's own wait, for a caller that holds only the
/// handle a signal or a doorbell lends through `AsHandle`. Like [`Signal::wait`], it never
/// reports `false` before `timeout` has passed.
pub fn wait_native(handle: BorrowedHandle<'_>, timeout: Duration) -> io::Result<bool> {
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

impl AsHandle for EventSignal {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.event.as_handle()
    }
}
