// Copyright (c) Michael Grier.

//! [`EventSignal`]: a manual-reset event.

use std::io;
use std::os::windows::io::{AsHandle, BorrowedHandle};
use std::time::Duration;

use win_sync_sys::{Event, ResetMode, WaitOnlyEvent, WaitOutcome, wait_for_single_object};

use super::{Signal, sealed, wait_until_deadline};

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
