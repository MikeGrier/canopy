// Copyright (c) Michael Grier.

use super::*;

#[cfg(test)]
mod tests;

/// An owned eventfd wait capability with no safe descriptor-extraction or I/O route.
///
/// Clones keep the same kernel object open. Waiting observes readability without consuming the
/// signal. The seal applies to this value, not to the original signal owner or unsealed aliases.
/// `Debug` exposes no descriptor number. Raw integration requires [`Self::as_waitable_fd`].
///
/// ```compile_fail
/// use acadia::{EventFdSignal, Signal};
/// use std::os::fd::AsFd;
/// let wait = EventFdSignal::new().unwrap().sealed_wait_only().unwrap();
/// let fd = wait.as_fd();
/// ```
///
/// ```compile_fail
/// use acadia::{EventFdSignal, Signal};
/// use std::os::fd::AsRawFd;
/// let wait = EventFdSignal::new().unwrap().sealed_wait_only().unwrap();
/// let fd = wait.as_raw_fd();
/// ```
///
/// ```compile_fail
/// use acadia::{EventFdSignal, Signal};
/// use std::os::fd::{AsFd, OwnedFd};
/// let wait = EventFdSignal::new().unwrap().sealed_wait_only().unwrap();
/// let owned: OwnedFd = wait.into();
/// ```
///
/// ```compile_fail
/// use acadia::{EventFdSignal, Signal};
/// let wait = EventFdSignal::new().unwrap().sealed_wait_only().unwrap();
/// let fd = wait.try_into_owned();
/// ```
///
/// ```compile_fail
/// use acadia::{EventFdSignal, Signal};
/// let wait = EventFdSignal::new().unwrap().sealed_wait_only().unwrap();
/// let fd = wait.as_waitable_fd();
/// ```
#[derive(Clone)]
pub struct SealedWaitOnlyFd {
    fd: Arc<OwnedFd>,
}

impl std::fmt::Debug for SealedWaitOnlyFd {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedWaitOnlyFd")
            .finish_non_exhaustive()
    }
}

impl EventFdSignal {
    /// Duplicates the eventfd into a sealed capability that safe code can only wait on.
    /// The owner retains its existing unsealed descriptor and control operations.
    pub fn sealed_wait_only(&self) -> io::Result<SealedWaitOnlyFd> {
        Ok(SealedWaitOnlyFd {
            fd: Arc::new(self.fd.try_clone()?),
        })
    }
}

impl SealedWaitOnlyFd {
    /// Waits for readability without reading the eventfd or changing its level.
    pub fn wait(&self, timeout: Duration) -> io::Result<bool> {
        super::wait_native(self.fd.as_fd(), timeout)
    }

    /// An opaque borrowed target for a safe mixed native wait.
    pub fn wait_object(&self) -> SealedWaitObject<'_> {
        SealedWaitObject {
            fd: self.fd.as_fd(),
        }
    }

    /// Registers the existing epoll service without lending a descriptor to its callback.
    /// Dropping the returned watch stops callbacks under [`Signal::watch`]'s contract.
    pub fn watch<F>(&self, callback: F) -> io::Result<Watch>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let service = Service::get()?;
        let entry = Arc::new(Entry {
            fd: self.fd.try_clone()?,
            callback: Arc::new(callback),
            state: Mutex::new(EntryState::default()),
            idle: Condvar::new(),
        });
        let token = service.register(&entry)?;
        Ok(Watch {
            _registration: Registration::Epoll(EpollWatch {
                service,
                token,
                entry,
            }),
        })
    }

    /// Borrows the descriptor for custom wait integration.
    ///
    /// # Safety
    ///
    /// Only wait for readability. Never read, write, close, change flags, or expose this
    /// descriptor or a derived duplicate to code that can change the eventfd's level. A read
    /// drains the signal and a write raises it; either can invalidate the owner's protocol.
    pub unsafe fn as_waitable_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// A borrowed poll target that preserves a sealed descriptor's unsafe-access boundary.
/// It can also carry an ordinary descriptor its caller already owns.
///
/// ```compile_fail
/// use acadia::{EventFdSignal, Signal};
/// use std::os::fd::AsFd;
/// let wait = EventFdSignal::new().unwrap().sealed_wait_only().unwrap();
/// let object = wait.wait_object();
/// let fd = object.as_fd();
/// ```
///
/// ```compile_fail
/// use acadia::{EventFdSignal, Signal};
/// use std::os::fd::AsRawFd;
/// let wait = EventFdSignal::new().unwrap().sealed_wait_only().unwrap();
/// let object = wait.wait_object();
/// let fd = object.as_raw_fd();
/// ```
#[derive(Clone, Copy)]
pub struct SealedWaitObject<'a> {
    fd: BorrowedFd<'a>,
}

impl std::fmt::Debug for SealedWaitObject<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedWaitObject")
            .finish_non_exhaustive()
    }
}

impl<'a> From<&'a SealedWaitOnlyFd> for SealedWaitObject<'a> {
    fn from(wait: &'a SealedWaitOnlyFd) -> Self {
        wait.wait_object()
    }
}

impl<'a> From<BorrowedFd<'a>> for SealedWaitObject<'a> {
    fn from(fd: BorrowedFd<'a>) -> Self {
        Self { fd }
    }
}

/// Waits for readability in a sealed/ordinary mixed set without consuming any descriptor.
/// Returns the first readable target's index, or `None` only after the timeout. An empty set
/// is invalid input; invalid, failed or hung-up descriptors are errors, not a quiet timeout.
pub fn wait_native_any(
    objects: &[SealedWaitObject<'_>],
    timeout: Duration,
) -> io::Result<Option<usize>> {
    if objects.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a wait needs at least one target",
        ));
    }
    let count: libc::nfds_t = objects
        .len()
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many wait targets"))?;
    let mut descriptors: Vec<_> = objects
        .iter()
        .map(|object| libc::pollfd {
            fd: object.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let mut found = None;
    wait_until_deadline(timeout, |remaining| {
        for descriptor in &mut descriptors {
            descriptor.revents = 0;
        }
        let millis = millis_rounded_up(remaining, i32::MAX as u64) as i32;
        let result = unsafe { libc::poll(descriptors.as_mut_ptr(), count, millis) };
        if result < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(false)
            } else {
                Err(error)
            };
        }
        for descriptor in &descriptors {
            if descriptor.revents & libc::POLLNVAL != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid wait descriptor",
                ));
            }
            if descriptor.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                return Err(io::Error::other("wait descriptor failed or hung up"));
            }
        }
        found = descriptors
            .iter()
            .position(|descriptor| descriptor.revents & libc::POLLIN != 0);
        Ok(found.is_some())
    })?;
    Ok(found)
}
