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
//! does.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{LockMode, LockSpan, Vfs, VfsErrorKind, VfsLock};

use super::{
    CoordError, CoordResult, Coordination, CoordinationFactory, ImplementationId, Waiting,
};

pub(crate) mod admission;
pub(crate) mod count;
mod creation;
pub(crate) mod exclusion;
pub(crate) mod hold;
pub(crate) mod layout;
mod notify;
pub(crate) mod registry;
pub(crate) mod reservation;
pub(crate) mod restart;

use layout::Layout;

/// Makes participants of the file-based implementation.
#[derive(Debug, Clone)]
pub struct FileFactory {
    id: ImplementationId,
    layout: fn(&Path) -> Layout,
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
        }
    }

    /// The second implementation the conformance suite uses: the file-based one with another
    /// identity and another creation exclusion, so that the cases between two implementations run.
    #[cfg(test)]
    pub(crate) fn alternate() -> Self {
        FileFactory {
            id: ImplementationId::from_static("acadia-file-alternate-1"),
            layout: Layout::alternate,
        }
    }

    /// A participant on the root `root` of `vfs`, as [`CoordinationFactory::open`] makes one.
    pub(crate) fn participant(&self, vfs: Arc<dyn Vfs>, root: &Path) -> FileCoordination {
        FileCoordination {
            id: self.id.clone(),
            vfs,
            layout: (self.layout)(root),
            holds: Arc::default(),
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
        let released = {
            let mut table = self.holds.table();
            table.abandoned = true;
            std::mem::take(&mut table.locks)
        };
        drop(released);
    }
}

/// A suffix no other staged file of the state has, in any process, for a file renamed over a
/// record once written.
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
    table: Mutex<HoldTable>,
}

#[derive(Default)]
struct HoldTable {
    abandoned: bool,
    next: u64,
    locks: HashMap<u64, Box<dyn VfsLock>>,
}

impl Holds {
    fn table(&self) -> MutexGuard<'_, HoldTable> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl FileCoordination {
    pub(crate) fn vfs(&self) -> &dyn Vfs {
        &*self.vfs
    }

    pub(crate) fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Refuses with [`CoordError::Abandoned`] once the participant is abandoned.
    pub(crate) fn alive(&self) -> CoordResult<()> {
        if self.holds.table().abandoned {
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
                        _ => CoordError::Vfs(error),
                    })?
            }
        };
        let mut table = self.holds.table();
        if table.abandoned {
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
        self.holds.table().locks.remove(&id.0)
    }

    /// Replaces `target` whole with `bytes`, written to `staging` and renamed over it, so a reader
    /// sees the previous file or this one and never part of either. A staged file left by a
    /// failure is removed if it can be.
    fn replace_whole(&self, staging: &Path, target: &Path, bytes: &[u8]) -> CoordResult<()> {
        let replaced = self
            .vfs
            .write(staging, bytes)
            .and_then(|()| self.vfs.rename(staging, target));
        if replaced.is_err() {
            let _ = self.vfs.remove_file(staging);
        }
        Ok(replaced?)
    }
}
