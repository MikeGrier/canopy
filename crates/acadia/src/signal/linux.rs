// Copyright (c) Michael Grier.

//! [`EventFdSignal`]: an `eventfd`.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use super::{
    Callback, Registration, Signal, Watch, millis_rounded_up, run_callback, sealed,
    wait_until_deadline,
};

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

    fn watch<F>(&self, callback: F) -> io::Result<Watch>
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
}

/// The threads that run the callbacks of every [`EventFdSignal`] watch, waiting on one `epoll`
/// set. Made on the first watch and kept for the life of the process.
struct Service {
    epoll: OwnedFd,
    entries: Mutex<HashMap<u64, Arc<Entry>>>,
    next_token: AtomicU64,
    /// The threads started so far.
    workers: Mutex<usize>,
}

/// The most threads the service runs callbacks on. Each watch has its callback run by one thread
/// at a time, so this bounds how many watches run at once, not how many there are.
const MOST_WORKERS: usize = 4;

/// One watch's registration in the service's `epoll` set, under its token.
struct Entry {
    /// A duplicate of the signal's descriptor, so the registration outlives no descriptor it
    /// names.
    fd: OwnedFd,
    callback: Callback,
    state: Mutex<EntryState>,
    /// Notified when a run of the callback returns.
    idle: Condvar,
}

#[derive(Default)]
struct EntryState {
    removed: bool,
    /// The thread running the callback, if one is.
    running: Option<ThreadId>,
}

impl Service {
    fn get() -> io::Result<&'static Service> {
        static SERVICE: Mutex<Option<&'static Service>> = Mutex::new(None);
        let mut slot = SERVICE.lock().unwrap_or_else(PoisonError::into_inner);
        let service = match *slot {
            Some(service) => service,
            None => {
                // SAFETY: the call only creates a descriptor.
                let epoll = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
                if epoll < 0 {
                    return Err(io::Error::last_os_error());
                }
                let service: &'static Service = Box::leak(Box::new(Service {
                    // SAFETY: `epoll` is a descriptor just created and owned by nothing else.
                    epoll: unsafe { OwnedFd::from_raw_fd(epoll) },
                    entries: Mutex::new(HashMap::new()),
                    next_token: AtomicU64::new(0),
                    workers: Mutex::new(0),
                }));
                *slot = Some(service);
                service
            }
        };
        service.start_workers()?;
        Ok(service)
    }

    /// Starts threads up to the service's number; an error only when none is running, since
    /// fewer threads are slower but still run every callback.
    fn start_workers(&'static self) -> io::Result<()> {
        let wanted = thread::available_parallelism().map_or(1, |n| n.get().min(MOST_WORKERS));
        let mut workers = self.workers.lock().unwrap_or_else(PoisonError::into_inner);
        while *workers < wanted {
            match thread::Builder::new()
                .name("acadia-signal-watch".into())
                .spawn(move || self.serve())
            {
                Ok(_) => *workers += 1,
                Err(error) if *workers == 0 => return Err(error),
                Err(_) => break,
            }
        }
        Ok(())
    }

    fn register(&self, entry: &Arc<Entry>) -> io::Result<u64> {
        let token = self.next_token.fetch_add(1, Ordering::AcqRel);
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token, Arc::clone(entry));
        if let Err(error) = self.control(libc::EPOLL_CTL_ADD, entry.fd.as_raw_fd(), token) {
            self.entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&token);
            return Err(error);
        }
        Ok(token)
    }

    /// Registration is one-shot: a reported descriptor is reported to no other thread until it is
    /// armed again, which is what keeps one watch's runs from overlapping. It is also
    /// level-triggered, so a descriptor armed again while readable is reported again at once.
    fn control(&self, op: libc::c_int, fd: RawFd, token: u64) -> io::Result<()> {
        let mut event = libc::epoll_event {
            events: (libc::EPOLLIN | libc::EPOLLONESHOT) as u32,
            u64: token,
        };
        // SAFETY: both descriptors are open and `event` is a valid `epoll_event`.
        if unsafe { libc::epoll_ctl(self.epoll.as_raw_fd(), op, fd, &mut event) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn serve(&self) {
        loop {
            let mut event = libc::epoll_event { events: 0, u64: 0 };
            // SAFETY: the set is open for the life of the process and `event` has room for the
            // one event asked for.
            let reported = unsafe { libc::epoll_wait(self.epoll.as_raw_fd(), &mut event, 1, -1) };
            if reported < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                // The set is acadia's own and stays open, so its wait cannot fail; if it does,
                // no watch would be run again, which no caller could detect.
                std::process::abort();
            }
            if reported == 0 {
                continue;
            }
            let token = event.u64;
            let entry = self
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&token)
                .cloned();
            let Some(entry) = entry else {
                continue;
            };
            {
                let mut state = entry.state.lock().unwrap_or_else(PoisonError::into_inner);
                if state.removed {
                    continue;
                }
                state.running = Some(thread::current().id());
            }
            run_callback(&entry.callback);
            let mut state = entry.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.running = None;
            if !state.removed
                && self
                    .control(libc::EPOLL_CTL_MOD, entry.fd.as_raw_fd(), token)
                    .is_err()
            {
                // As for the wait: a registration that cannot be armed again would leave its
                // watch silently stopped.
                std::process::abort();
            }
            drop(state);
            entry.idle.notify_all();
        }
    }
}

/// An [`EventFdSignal`]'s watch: its entry in the service's `epoll` set.
pub(super) struct EpollWatch {
    service: &'static Service,
    token: u64,
    entry: Arc<Entry>,
}

impl Drop for EpollWatch {
    fn drop(&mut self) {
        let mut state = self
            .entry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Under the entry's lock, so a thread that has the descriptor reported but has not yet
        // started the callback finds it removed, and one that is running it does not arm it again.
        state.removed = true;
        // The descriptor is registered from `register` until here, so removing it cannot fail.
        let _ = self
            .service
            .control(libc::EPOLL_CTL_DEL, self.entry.fd.as_raw_fd(), self.token);
        self.service
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.token);
        // From within its own callback the run cannot be waited for; it is not run again.
        let current = thread::current().id();
        let _state = self
            .entry
            .idle
            .wait_while(state, |state| {
                state.running.is_some_and(|thread| thread != current)
            })
            .unwrap_or_else(PoisonError::into_inner);
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
