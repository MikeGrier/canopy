// Copyright (c) Michael Grier.

//! The file-based implementation: the coordination state in files beneath the root, guarded by
//! [`Vfs`] locks, so it runs over every backend that offers [`Vfs::try_lock`], [`Vfs::lock`] and
//! [`Vfs::publish_new`].
//!
//! The state is laid out in the `layout` module. Exclusions are OS locks, released by the operating system
//! when their holder dies; the ownership record, the fence, the generation and the count are files
//! written under them; the reservation is published with [`Vfs::publish_new`] and kept as the
//! record of which implementation coordinates the root.
//!
//! Every lock a participant holds is kept in its table of holds, so that
//! [`abandon`](Coordination::abandon) releases all of them without any bookkeeping, as a death
//! does. Every change a participant makes to the state's records is made through
//! `FileCoordination::publishing`, which an abandonment waits for and which refuses afterwards.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use crate::{LockMode, LockSpan, Vfs, VfsErrorKind, VfsLock, VfsResult};

use super::{
    CoordError, CoordResult, Coordination, CoordinationFactory, ImplementationId, Waiting,
};

pub(crate) mod admission;
pub(crate) mod count;
mod creation;
mod discovery;
pub(crate) mod exclusion;
pub(crate) mod hold;
pub(crate) mod layout;
mod notify;
#[cfg(windows)]
mod oplock;
pub(crate) mod registry;
pub(crate) mod reservation;
pub(crate) mod restart;

use layout::Layout;

/// Makes participants of the file-based implementation.
#[derive(Debug, Clone)]
pub struct FileFactory {
    id: ImplementationId,
    layout: fn(&Path) -> Layout,
    polled: bool,
}

impl FileFactory {
    /// The file-based implementation's identity.
    pub const IMPLEMENTATION: ImplementationId = ImplementationId::from_static("acadia-file-1");

    /// The factory of the file-based implementation.
    #[must_use]
    pub fn new() -> Self {
        FileFactory {
            id: Self::IMPLEMENTATION,
            layout: Layout::new,
            polled: false,
        }
    }

    /// This factory's participants take every [`CountNotification`](super::CountNotification)
    /// from the process's poller, never from the operating system.
    ///
    /// On Windows over the operating system's own filesystem a participant is otherwise notified by
    /// a lock the system breaks when a writer renames a fresh count over it, and the writer waits
    /// until the participant's process answers the break or the system's break timeout passes. A
    /// process whose thread pool cannot run, such as one stopped in a debugger, then delays the
    /// writer's publication. The poller never delays a writer, and costs a sample of the count at
    /// each of its intervals while a registration is outstanding.
    #[must_use]
    pub fn polled(mut self) -> Self {
        self.polled = true;
        self
    }

    /// The second implementation the conformance suite uses: the file-based one with another
    /// identity and another creation exclusion, so that the cases between two implementations run.
    #[cfg(any(test, feature = "test-support"))]
    pub fn alternate() -> Self {
        FileFactory {
            id: ImplementationId::from_static("acadia-file-alternate-1"),
            layout: Layout::alternate,
            polled: false,
        }
    }

    /// A participant on the root `root` of `vfs`, as [`CoordinationFactory::open`] makes one.
    pub(crate) fn participant(&self, vfs: Arc<dyn Vfs>, root: &Path) -> FileCoordination {
        FileCoordination {
            id: self.id.clone(),
            vfs,
            layout: (self.layout)(root),
            holds: Arc::default(),
            polled: self.polled,
        }
    }
}

impl Default for FileFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl CoordinationFactory for FileFactory {
    fn implementation(&self) -> &ImplementationId {
        &self.id
    }

    fn open(&self, vfs: Arc<dyn Vfs>, root: &Path) -> CoordResult<Box<dyn Coordination>> {
        Ok(Box::new(self.participant(vfs, root)))
    }
}

/// A participant of the file-based implementation.
pub struct FileCoordination {
    id: ImplementationId,
    vfs: Arc<dyn Vfs>,
    layout: Layout,
    /// Shared by every handle on this participant, such as a hold it gives out.
    holds: Arc<Holds>,
    /// Whether count notifications are the poller's alone; see [`FileFactory::polled`].
    polled: bool,
}

impl fmt::Debug for FileCoordination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileCoordination")
            .field("id", &self.id)
            .field("root", &self.layout.root())
            .finish_non_exhaustive()
    }
}

impl Coordination for FileCoordination {
    fn implementation(&self) -> &ImplementationId {
        &self.id
    }

    fn abandon(&self) {
        self.abandon_participant();
    }
}

impl FileCoordination {
    pub(crate) fn abandon_participant(&self) {
        let turn = self.holds.release.enter();
        let (released, watchers) = {
            let mut table = self.holds.table_recovering();
            self.holds.abandoned.store(true, Ordering::Release);
            (
                std::mem::take(&mut table.locks),
                std::mem::take(&mut table.watchers),
            )
        };
        crate::coord::discard(released.into_values());
        drop(turn);
        for watcher in watchers.iter().filter_map(Weak::upgrade) {
            watcher.abandoned();
        }
    }
}

/// A staging candidate distinguished by process identifier and process-local sequence.
fn staging_suffix() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Names one lock in a participant's table of holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HoldId(u64);

#[derive(Default)]
struct Holds {
    release: super::ReleaseGate,
    table: Mutex<HoldTable>,
    /// Set only while the table is held, so a check made under the table is serialized with the
    /// abandonment; read without it where an operation must not wait for one in progress.
    abandoned: AtomicBool,
}

#[derive(Default)]
struct HoldTable {
    next: u64,
    locks: HashMap<u64, Box<dyn VfsLock>>,
    /// Registrations notified by the operating system, which an abandonment rings: nothing
    /// breaks for a participant whose own operations are refused.
    watchers: Vec<Weak<dyn Watcher>>,
}

/// Something outstanding on a participant that its abandonment must be told of.
trait Watcher: Send + Sync {
    fn abandoned(&self);
}

impl Holds {
    fn table(&self) -> CoordResult<MutexGuard<'_, HoldTable>> {
        match self.table.lock() {
            Ok(table) => Ok(table),
            Err(_) => {
                self.abandoned.store(true, Ordering::Release);
                Err(CoordError::Abandoned)
            }
        }
    }

    fn table_recovering(&self) -> MutexGuard<'_, HoldTable> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_abandoned(&self) -> bool {
        self.abandoned.load(Ordering::Acquire)
    }

    /// Has `watcher` told when the participant is abandoned; reports whether it already is.
    #[cfg(windows)]
    fn watch(&self, watcher: Weak<dyn Watcher>) -> bool {
        match self.table() {
            Ok(mut table) => {
                table.watchers.retain(|held| held.strong_count() > 0);
                table.watchers.push(watcher);
                self.is_abandoned()
            }
            Err(_) => true,
        }
    }
}

impl FileCoordination {
    pub(crate) fn vfs(&self) -> &dyn Vfs {
        &*self.vfs
    }

    pub(crate) fn layout(&self) -> &Layout {
        &self.layout
    }

    #[cfg(test)]
    pub(crate) fn observe_release_wait(&self, sender: std::sync::mpsc::Sender<()>) {
        self.holds.release.on_wait.set(sender).unwrap();
    }

    #[cfg(test)]
    pub(crate) fn insert_test_lock(&self, lock: Box<dyn VfsLock>) -> HoldId {
        let mut table = self.holds.table_recovering();
        table.next += 1;
        let id = table.next;
        table.locks.insert(id, lock);
        HoldId(id)
    }

    /// Refuses with [`CoordError::Abandoned`] once the participant is abandoned. Takes no lock, so
    /// it never waits for a change in progress.
    pub(crate) fn alive(&self) -> CoordResult<()> {
        if self.holds.is_abandoned() {
            return Err(CoordError::Abandoned);
        }
        Ok(())
    }

    /// Takes a lock of `mode` on `span` of `path`, waiting as `waiting` allows, and keeps it in
    /// the table of holds.
    pub(crate) fn take_lock(
        &self,
        path: &Path,
        mode: LockMode,
        span: LockSpan,
        waiting: Waiting,
    ) -> CoordResult<HoldId> {
        self.alive()?;
        let lock = match waiting {
            Waiting::Wait => self.vfs.lock(path, mode, span)?,
            Waiting::Never => {
                self.vfs
                    .try_lock(path, mode, span)
                    .map_err(|error| match error.kind() {
                        VfsErrorKind::Locked => CoordError::WouldWait,
                        VfsErrorKind::ProcessEnded => CoordError::Abandoned,
                        _ => CoordError::Vfs(error),
                    })?
            }
        };
        let mut table = self.holds.table()?;
        if self.holds.is_abandoned() {
            drop(table);
            drop(lock);
            return Err(CoordError::Abandoned);
        }
        table.next += 1;
        let id = table.next;
        table.locks.insert(id, lock);
        Ok(HoldId(id))
    }

    /// Takes the lock `id` names out of the table, for the caller to drop once its bookkeeping
    /// is done, or `None` if the participant was abandoned, which released it already.
    pub(crate) fn release_lock(&self, id: HoldId) -> Option<Box<dyn VfsLock>> {
        match self.holds.table() {
            Ok(mut table) => table.locks.remove(&id.0),
            Err(_) => {
                self.abandon_participant();
                None
            }
        }
    }

    pub(crate) fn release_lock_safely(&self, id: HoldId) {
        let _turn = self.holds.release.enter();
        if let Some(lock) = self.release_lock(id)
            && crate::coord::catch_contained(|| drop(lock)).is_none()
        {
            self.abandon_participant();
        }
    }

    /// Runs `publish`, one change to the state on disk, unless the participant is abandoned. The
    /// table of holds is kept for its duration, so an abandonment waits for it, and after one
    /// returns the participant changes nothing more, as a dead process changes nothing: its locks
    /// may by then belong to another participant.
    pub(crate) fn publishing<T>(
        &self,
        publish: impl FnOnce(&dyn Vfs) -> VfsResult<T>,
    ) -> CoordResult<T> {
        let table = self.holds.table()?;
        if self.holds.is_abandoned() {
            return Err(CoordError::Abandoned);
        }
        let published = publish(&*self.vfs);
        drop(table);
        Ok(published?)
    }

    /// Replaces `target` whole with `bytes`, written to `staging` and renamed over it, so a reader
    /// sees the previous file or this one and never part of either. A staged file left by a
    /// failure is removed if it can be.
    fn replace_whole(&self, staging: &Path, target: &Path, bytes: &[u8]) -> CoordResult<()> {
        self.publishing(|vfs| {
            let replaced = vfs
                .write(staging, bytes)
                .and_then(|()| vfs.rename(staging, target));
            if replaced.is_err() {
                let _ = vfs.remove_file(staging);
            }
            replaced
        })
    }
}
