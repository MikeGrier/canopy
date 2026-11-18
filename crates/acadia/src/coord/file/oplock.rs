// Copyright (c) Michael Grier.

//! [`CountNotification`](crate::coord::CountNotification) on Windows, notified by the operating
//! system instead of polling: a handle-caching opportunistic lock on the file holding the
//! published count, which the operating system breaks when a writer renames a fresh count over
//! it. The break arrives as an I/O completion on the thread pool, and the deadline as a pool
//! timer, so a waiting registration holds no thread and samples nothing.
//!
//! A read lock alone is not broken by the rename, and the break of a handle-caching one holds the
//! renaming writer until this side answers it. It is answered in the completion, at once, by
//! relinquishing the lock; a process that cannot run its pool's threads delays a writer's
//! publication until the operating system's break timeout passes, which is why a factory can be
//! made to poll instead ([`FileFactory::polled`](super::FileFactory::polled)).
//!
//! The break comes before the rename finishes, so a registration whose lock broke samples again
//! on short timers before it rings. Where the lock is not granted, or the root is not the
//! operating system's filesystem, [`register`] hands the ring back and the poller takes it.

use std::cell::Cell;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use windows_overlapped_io_sys::{Issued, Operation, Submitted, UnassociatedEndpoint};
use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, ERROR_OPLOCK_BREAK_IN_PROGRESS, HANDLE, NO_ERROR,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES, GetFileInformationByHandle,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_REQUEST_OPLOCK, OPLOCK_LEVEL_CACHE_HANDLE, OPLOCK_LEVEL_CACHE_READ,
    REQUEST_OPLOCK_CURRENT_VERSION, REQUEST_OPLOCK_INPUT_BUFFER, REQUEST_OPLOCK_INPUT_FLAG_ACK,
    REQUEST_OPLOCK_INPUT_FLAG_REQUEST, REQUEST_OPLOCK_OUTPUT_BUFFER,
};
use windows_threadpool_sys::io::{IoCompletion, ThreadpoolIo};
use windows_threadpool_sys::timer::ThreadpoolTimer;

use crate::coord::{ModCount, ModificationCount, Notice};

use super::{FileCoordination, Watcher};

type Ring = Box<dyn FnOnce() + Send>;

/// The oplock level requested: read and handle caching, the level the rename breaks.
const REQUESTED_LEVEL: u32 = OPLOCK_LEVEL_CACHE_READ | OPLOCK_LEVEL_CACHE_HANDLE;
/// The level an acknowledgement keeps: none, so the lock is wholly given up.
const RELINQUISH: u32 = 0;

/// The phase of a registration that is waiting for the lock to break or the deadline to pass.
const WAITING: u32 = 0;
/// The first settling interval after a break; each later one is twice the one before.
const SETTLE_FIRST: Duration = Duration::from_micros(250);
/// How many settling samples a break is given before it rings regardless: a rename that follows
/// its break finishes within a few milliseconds.
const SETTLE_STEPS: u32 = 6;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

thread_local! {
    /// Whether this thread is running one of this module's callbacks, from which a registration
    /// cannot be torn down in place: tearing it down waits for its callbacks.
    static IN_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

fn in_callback(work: impl FnOnce()) {
    let outer = IN_CALLBACK.replace(true);
    work();
    IN_CALLBACK.set(outer);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Request,
    Acknowledgement,
}

/// What one `FSCTL_REQUEST_OPLOCK` reads and writes, which the kernel owns until it completes.
struct Buffers {
    kind: Kind,
    input: REQUEST_OPLOCK_INPUT_BUFFER,
    output: std::cell::UnsafeCell<REQUEST_OPLOCK_OUTPUT_BUFFER>,
}

// SAFETY: the input is never written after construction, and the output is written only by the
// kernel while the operation is outstanding and never read by this code.
unsafe impl Sync for Buffers {}

/// Issues one `FSCTL_REQUEST_OPLOCK` on `io`'s handle: a request for the lock, or the
/// acknowledgement of its break.
fn issue(io: &ThreadpoolIo, kind: Kind) -> io::Result<()> {
    let (flags, level) = match kind {
        Kind::Request => (REQUEST_OPLOCK_INPUT_FLAG_REQUEST, REQUESTED_LEVEL),
        Kind::Acknowledgement => (REQUEST_OPLOCK_INPUT_FLAG_ACK, RELINQUISH),
    };
    let input_size = size_of::<REQUEST_OPLOCK_INPUT_BUFFER>();
    let output_size = size_of::<REQUEST_OPLOCK_OUTPUT_BUFFER>();
    // SAFETY: both buffers are plain integers, for which all zeroes is a valid value.
    let mut output: REQUEST_OPLOCK_OUTPUT_BUFFER = unsafe { std::mem::zeroed() };
    output.StructureVersion = REQUEST_OPLOCK_CURRENT_VERSION as u16;
    output.StructureLength = output_size as u16;
    let buffers = Arc::new(Buffers {
        kind,
        input: REQUEST_OPLOCK_INPUT_BUFFER {
            StructureVersion: REQUEST_OPLOCK_CURRENT_VERSION as u16,
            StructureLength: input_size as u16,
            RequestedOplockLevel: level,
            Flags: flags,
        },
        output: std::cell::UnsafeCell::new(output),
    });
    let input = &raw const buffers.input;
    let output = buffers.output.get();
    // SAFETY: the closure starts exactly one overlapped `DeviceIoControl` on the operation's own
    // `OVERLAPPED`, and reports `Pending` when a completion will arrive -- for the lock request
    // and for the acknowledgement, which on a handle not in skip-on-success mode delivers one
    // whether it finishes at once or not -- and `Err` when none will. The buffers are held by the
    // operation's payload, which lives until the completion is claimed, and the closure cannot
    // unwind.
    let submitted = unsafe {
        io.submit(Operation::new(buffers), |handle, overlapped| {
            let started = DeviceIoControl(
                handle.as_raw_handle(),
                FSCTL_REQUEST_OPLOCK,
                input.cast(),
                input_size as u32,
                output.cast(),
                output_size as u32,
                ptr::null_mut(),
                overlapped,
            );
            if started != 0 {
                return Ok(Issued::Pending);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(Issued::Pending)
            } else {
                Err(error)
            }
        })
    };
    match submitted {
        Submitted::Pending(_) => Ok(()),
        Submitted::Failed { error, .. } => Err(error),
        Submitted::Completed { .. } => Err(io::Error::other(
            "an oplock request completed with no completion to come",
        )),
    }
}

/// The volume and file a handle names, which two handles share only if they name one file.
fn identity(handle: HANDLE) -> io::Result<(u32, u64)> {
    // SAFETY: plain integers, for which all zeroes is a valid value.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `handle` is a live handle and `info` a valid out-parameter.
    if unsafe { GetFileInformationByHandle(handle, &raw mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let index = u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow);
    Ok((info.dwVolumeSerialNumber, index))
}

/// One registration's state, shared by its notice and the callbacks of its lock and its timer.
///
/// The callbacks hold it weakly, so the lock and the timer it owns do not keep it alive.
struct Shared {
    participant: FileCoordination,
    sampled: ModCount,
    deadline: Instant,
    ring: Mutex<Option<Ring>>,
    stopped: AtomicBool,
    /// [`WAITING`] until the lock breaks, then the number of the settling sample due next.
    phase: AtomicU32,
    io: Mutex<Option<ThreadpoolIo>>,
    timer: Mutex<Option<ThreadpoolTimer>>,
}

impl Shared {
    fn fire(&self) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let ring = lock(&self.ring).take();
        if let Some(ring) = ring {
            let _ = catch_unwind(AssertUnwindSafe(ring));
        }
    }

    /// Answers a break whose acknowledgement failed by closing the handle, the other answer, and
    /// rings once it is closed, so a ring that waits for the renaming writer cannot hold it.
    fn release_and_ring(&self) {
        let ring = if self.stopped.load(Ordering::Acquire) {
            None
        } else {
            lock(&self.ring).take()
        };
        stop_then(self, ring);
    }

    fn on_timer(&self) {
        match self.phase.load(Ordering::Acquire) {
            WAITING => self.fire(),
            step => self.settle(step),
        }
    }

    fn on_completion(&self, completion: &IoCompletion) {
        // SAFETY: every operation submitted through this registration's `ThreadpoolIo` carries
        // `Arc<Buffers>`, and each completion is claimed once, here.
        let operation = unsafe { completion.claim::<Arc<Buffers>>() };
        if operation.payload().kind == Kind::Acknowledgement {
            if completion.io_result() != NO_ERROR && !self.stopped.load(Ordering::Acquire) {
                self.release_and_ring();
            }
            return;
        }
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        match completion.io_result() {
            ERROR_OPERATION_ABORTED => {}
            NO_ERROR | ERROR_OPLOCK_BREAK_IN_PROGRESS => {
                self.phase.store(1, Ordering::Release);
                // The renaming writer waits for this, so it comes before anything else.
                let acknowledged = match &*lock(&self.io) {
                    Some(io) => issue(io, Kind::Acknowledgement).is_ok(),
                    None => true,
                };
                if !acknowledged {
                    self.release_and_ring();
                    return;
                }
                self.settle(1);
            }
            // A lock that failed after it was granted: the caller samples and meets the cause.
            _ => self.fire(),
        }
    }

    /// Rings once the count differs from what was sampled, or after the last settling sample or
    /// the deadline; otherwise samples again after a longer interval.
    fn settle(&self, step: u32) {
        let unmoved = self
            .participant
            .sample()
            .is_ok_and(|count| count == self.sampled);
        if !unmoved || step > SETTLE_STEPS || Instant::now() >= self.deadline {
            self.fire();
            return;
        }
        self.phase.store(step + 1, Ordering::Release);
        if let Some(timer) = &*lock(&self.timer) {
            timer.set_after(SETTLE_FIRST * (1 << (step - 1)));
        }
    }

    /// Whether the file the lock is held on is still the one published at the count's path, so
    /// that a later rename over it breaks the lock.
    fn lock_is_on_the_published_file(&self) -> bool {
        let Some(io) = &*lock(&self.io) else {
            return false;
        };
        let current = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .open(self.participant.layout().count());
        match (current, identity(io.handle().as_raw_handle())) {
            (Ok(current), Ok(held)) => identity(current.as_raw_handle()).is_ok_and(|id| id == held),
            _ => false,
        }
    }
}

impl Watcher for Shared {
    /// A participant whose operations are refused samples nothing, so its registrations ring and
    /// their callers meet the refusal.
    fn abandoned(&self) {
        self.phase.store(1, Ordering::Release);
        if let Some(timer) = &*lock(&self.timer) {
            timer.set_after(Duration::ZERO);
        }
    }
}

/// Registers with the operating system, or gives `ring` back for the poller.
///
/// The ring goes back whenever the registration cannot be made as the operating system's: the
/// filesystem is not the operating system's or the factory polls, the count has no file yet, the
/// lock is not granted, or the count has moved by the time the lock is held.
pub(super) fn register(
    participant: &FileCoordination,
    sampled: ModCount,
    deadline: Instant,
    ring: Ring,
) -> Result<Box<dyn Notice>, Ring> {
    if participant.polled || !participant.vfs().is_os_filesystem() {
        return Err(ring);
    }
    let Some(delay) = deadline
        .checked_duration_since(Instant::now())
        .filter(|delay| !delay.is_zero())
    else {
        return Err(ring);
    };
    let Ok(endpoint) = UnassociatedEndpoint::open(participant.layout().count(), true, false, 0)
    else {
        return Err(ring);
    };
    let shared = Arc::new(Shared {
        participant: participant.same_participant(),
        sampled,
        deadline,
        ring: Mutex::new(Some(ring)),
        stopped: AtomicBool::new(false),
        phase: AtomicU32::new(WAITING),
        io: Mutex::new(None),
        timer: Mutex::new(None),
    });
    let notice = OplockNotice {
        shared: Arc::clone(&shared),
    };
    let watcher: Weak<Shared> = Arc::downgrade(&shared);
    if participant.holds.watch(watcher) {
        shared.abandoned();
    }
    if hold_lock(&shared, endpoint, delay).is_ok() {
        return Ok(Box::new(notice));
    }
    stop(&shared);
    // A ring that already ran is not run again, and the registration stands as rung.
    match lock(&shared.ring).take() {
        Some(ring) => Err(ring),
        None => Ok(Box::new(notice)),
    }
}

/// Starts the deadline timer, takes the lock on `endpoint`'s file, and checks that the count
/// has not moved and the file is still the published one.
fn hold_lock(
    shared: &Arc<Shared>,
    endpoint: UnassociatedEndpoint,
    delay: Duration,
) -> io::Result<()> {
    let weak: Weak<Shared> = Arc::downgrade(shared);
    let timer = {
        let weak = weak.clone();
        ThreadpoolTimer::new(
            move |_| {
                if let Some(shared) = weak.upgrade() {
                    in_callback(|| shared.on_timer());
                }
            },
            None,
        )?
    };
    timer.set_after(delay);
    *lock(&shared.timer) = Some(timer);
    let io = ThreadpoolIo::new(
        endpoint,
        move |completion| {
            if let Some(shared) = weak.upgrade() {
                in_callback(|| shared.on_completion(completion));
            }
        },
        None,
    )?;
    // Held before the request is made, so a break cannot arrive to a registration with no lock.
    let mut slot = lock(&shared.io);
    let io = slot.insert(io);
    issue(io, Kind::Request)?;
    drop(slot);
    // The lock was granted on the file that was open, which a rename may already have replaced,
    // after which nothing would break it.
    let unmoved = shared
        .participant
        .sample()
        .is_ok_and(|count| count == shared.sampled);
    if unmoved && shared.lock_is_on_the_published_file() {
        Ok(())
    } else {
        Err(io::Error::other("the count moved while it was registered"))
    }
}

/// Stops a registration: its callbacks no longer ring, and its lock and timer are released.
fn stop(shared: &Shared) {
    stop_then(shared, None);
}

/// As [`stop`], then runs `ring` once the lock and timer are released.
fn stop_then(shared: &Shared, ring: Option<Ring>) {
    let run = |ring: Option<Ring>| {
        if let Some(ring) = ring {
            let _ = catch_unwind(AssertUnwindSafe(ring));
        }
    };
    shared.stopped.store(true, Ordering::Release);
    let timer = lock(&shared.timer).take();
    let io = lock(&shared.io).take();
    if !IN_CALLBACK.get() {
        drop(timer);
        drop(io);
        run(ring);
        return;
    }
    // Releasing them waits for their callbacks, and this is one; they are released on a thread of
    // their own, and left held if there is none to release them on.
    let slot = Arc::new(Mutex::new(Some((timer, io, ring))));
    let releaser = Arc::clone(&slot);
    let started = std::thread::Builder::new()
        .name("acadia-oplock-release".into())
        .spawn(move || {
            let taken = lock(&releaser).take();
            if let Some((timer, io, ring)) = taken {
                drop(timer);
                drop(io);
                run(ring);
            }
        });
    if started.is_err() {
        let taken = lock(&slot).take();
        if let Some((timer, io, ring)) = taken {
            std::mem::forget((timer, io));
            run(ring);
        }
    }
}

/// A registration made through [`register`], withdrawn when dropped.
struct OplockNotice {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for OplockNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OplockNotice").finish_non_exhaustive()
    }
}

impl Notice for OplockNotice {}

impl Drop for OplockNotice {
    fn drop(&mut self) {
        stop(&self.shared);
    }
}
