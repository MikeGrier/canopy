// Copyright (c) Michael Grier.

//! [`EventFdSignal`]: an `eventfd`.

use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::time::Duration;

use super::{Signal, millis_rounded_up, sealed, wait_until_deadline};

/// A [`Signal`] that is a Linux `eventfd`, non-blocking and not in semaphore mode: readable
/// while its counter is not zero, so it is a level as an event is. A write raises it and a
/// read, which takes the whole counter, lowers it.
///
/// [`AsFd`] gives the descriptor to a platform wait -- `poll`, `epoll` -- which sees it readable
/// exactly while [`Signal::wait`] sees it raised.
#[derive(Debug)]
pub struct EventFdSignal {
    fd: OwnedFd,
}

impl sealed::Sealed for EventFdSignal {}

impl Signal for EventFdSignal {
    fn new() -> io::Result<Self> {
        // SAFETY: the call only creates a descriptor.
        let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a descriptor just created and owned by nothing else.
        Ok(EventFdSignal {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    fn raise(&self) -> io::Result<()> {
        let one: u64 = 1;
        // SAFETY: the descriptor is open for the life of `self` and the buffer is 8 bytes.
        let written = unsafe {
            libc::write(
                self.fd.as_raw_fd(),
                (&raw const one).cast(),
                size_of::<u64>(),
            )
        };
        if written < 0 {
            let error = io::Error::last_os_error();
            // A counter too full to take one more is raised already.
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(error);
            }
        }
        Ok(())
    }

    fn lower(&self) -> io::Result<()> {
        let mut count: u64 = 0;
        // SAFETY: as for `raise`.
        let read = unsafe {
            libc::read(
                self.fd.as_raw_fd(),
                (&raw mut count).cast(),
                size_of::<u64>(),
            )
        };
        if read < 0 {
            let error = io::Error::last_os_error();
            // A counter of zero has nothing to read: the signal is lowered already.
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(error);
            }
        }
        Ok(())
    }

    fn wait(&self, timeout: Duration) -> io::Result<bool> {
        wait_native(self.fd.as_fd(), timeout)
    }
}

/// Waits with `poll` until `fd` is readable or `timeout` has passed, and reports whether it was
/// readable: the platform's own wait, for a caller that holds only the descriptor a signal or a
/// doorbell lends through `AsFd`. Like [`Signal::wait`], it never reports `false` before
/// `timeout` has passed.
pub fn wait_native(fd: BorrowedFd<'_>, timeout: Duration) -> io::Result<bool> {
    wait_until_deadline(timeout, |remaining| {
        let millis = millis_rounded_up(remaining, i32::MAX as u64) as i32;
        let mut ready = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid `pollfd`, for a descriptor open for the whole call.
        match unsafe { libc::poll(&mut ready, 1, millis) } {
            0 => Ok(false),
            n if n > 0 => Ok(ready.revents & libc::POLLIN != 0),
            _ => {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    Ok(false)
                } else {
                    Err(error)
                }
            }
        }
    })
}

impl AsFd for EventFdSignal {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
