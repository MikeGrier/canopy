// Copyright (c) Michael Grier.

//! The locks of a [`MemFs`]: which holder has which bytes of which file, in which mode, and the
//! simulated processes those holders belong to.
//!
//! The table is never locked while the tree is: an operation that checks it takes what it needs
//! and lets go before taking the tree, and taking a lock holds the table and then the tree, so
//! the two locks are always taken in that order.

use std::sync::Condvar;

use super::*;

/// What an operation does to the bytes it touches, which decides the holds that refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Access {
    Read,
    Write,
}

/// The lock table of one filesystem, and the condition a waiting lock sleeps on until a
/// release lets it in.
#[derive(Debug, Default)]
pub(super) struct LockShared {
    table: Mutex<LockTable>,
    released: Condvar,
}

/// Every lock held in one filesystem, by every simulated process over it.
#[derive(Debug, Default)]
struct LockTable {
    /// The holds on each file, keyed by normalized components so a lock does not depend on how
    /// a caller spelled the path.
    holds: HashMap<Vec<OsString>, Vec<Hold>>,
    /// The last hold id handed out. Never reused, so a guard whose hold was already released by
    /// an abandonment can never release a later holder's.
    last_hold: u64,
    /// The last simulated process id handed out; the first process is `0`.
    last_process: u64,
}

/// One holder's lock on `[start, end)` of a file.
#[derive(Debug, Clone, Copy)]
pub(super) struct Hold {
    id: u64,
    process: u64,
    mode: LockMode,
    start: u64,
    end: u64,
}

impl LockTable {
    fn excludes(&self, parts: &[OsString], mode: LockMode, start: u64, end: u64) -> bool {
        self.holds
            .get(parts)
            .is_some_and(|holds| holds.iter().any(|hold| hold.excludes(mode, start, end)))
    }
}

impl Hold {
    fn overlaps(&self, start: u64, end: u64) -> bool {
        self.start < end && start < self.end
    }

    /// Whether a lock of `mode` on `[start, end)` may not be held alongside this one.
    fn excludes(&self, mode: LockMode, start: u64, end: u64) -> bool {
        self.overlaps(start, end)
            && (self.mode == LockMode::Exclusive || mode == LockMode::Exclusive)
    }

    /// Whether this hold refuses `access` to `[start, end)`, as a Windows lock refuses every
    /// handle but its own: an exclusive lock refuses reads and writes, a shared one writes.
    fn refuses(&self, access: Access, start: u64, end: u64) -> bool {
        self.overlaps(start, end) && (access == Access::Write || self.mode == LockMode::Exclusive)
    }
}

/// The holds on one file, taken before an operation reads the tree so that it can check the
/// bytes it touches once it knows them.
#[derive(Debug, Default)]
pub(super) struct Holds(Vec<Hold>);

impl Holds {
    /// Refuses `access` to `[start, end)` with [`VfsErrorKind::Locked`] if a hold covers it.
    pub(super) fn check(
        &self,
        access: Access,
        start: u64,
        end: u64,
        op: &'static str,
        path: &Path,
    ) -> VfsResult<()> {
        if self.0.iter().any(|hold| hold.refuses(access, start, end)) {
            return Err(err(
                VfsErrorKind::Locked,
                op,
                path,
                "a lock covers these bytes",
            ));
        }
        Ok(())
    }
}

/// A held [`MemFs`] lock, released on drop unless its process was abandoned first.
///
/// Holds the table rather than the filesystem so releasing needs no borrow of the `MemFs` it
/// came from.
#[derive(Debug)]
struct MemHold {
    shared: Arc<LockShared>,
    parts: Vec<OsString>,
    id: u64,
}

impl VfsLock for MemHold {}

impl Drop for MemHold {
    fn drop(&mut self) {
        let mut table = self
            .shared
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(holds) = table.holds.get_mut(&self.parts) {
            holds.retain(|hold| hold.id != self.id);
            if holds.is_empty() {
                table.holds.remove(&self.parts);
            }
        }
        drop(table);
        self.shared.released.notify_all();
    }
}

impl MemFs {
    /// A second simulated process over this same filesystem: it sees the same tree and
    /// contends for the same locks, and [`abandon`](MemFs::abandon) on it releases its locks
    /// alone.
    #[must_use]
    pub fn another_process(&self) -> MemFs {
        let mut table = self.table();
        table.last_process += 1;
        MemFs {
            inner: Arc::clone(&self.inner),
            locks: Arc::clone(&self.locks),
            process: table.last_process,
        }
    }

    /// Simulates this process dying: every lock it holds is released at once, as the operating
    /// system releases a dead process's locks, and no guard it took runs any release of its own.
    ///
    /// Guards still in hand stay valid values, and dropping one later releases nothing, so a
    /// test can abandon a process without first dropping the handles that would have erased
    /// what it means to observe. The tree is untouched: a death loses no durable or written
    /// data, which is [`simulate_crash`](MemFs::simulate_crash)'s business.
    pub fn abandon(&self) {
        self.table().holds.retain(|_, holds| {
            holds.retain(|hold| hold.process != self.process);
            !holds.is_empty()
        });
        self.locks.released.notify_all();
    }

    fn table(&self) -> MutexGuard<'_, LockTable> {
        self.locks
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The holds on the file `parts` names, for an operation to check once it knows what bytes
    /// it touches.
    pub(super) fn holds_on(&self, parts: &[OsString]) -> Holds {
        Holds(self.table().holds.get(parts).cloned().unwrap_or_default())
    }

    /// Takes a lock of `mode` on `span` of the file at `path`, creating the file if absent, and
    /// waiting while another hold conflicts if `wait`.
    pub(super) fn take_lock(
        &self,
        path: &Path,
        op: &'static str,
        mode: LockMode,
        span: LockSpan,
        wait: bool,
    ) -> VfsResult<Box<dyn VfsLock>> {
        let parts = split_path(path, op)?;
        let Some((start, end)) = span.bounds() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                op,
                path,
                "a range must be non-empty and end at or before LockSpan::MAX_END",
            ));
        };
        let mut table = self.table();
        while wait && table.excludes(&parts, mode, start, end) {
            table = self
                .locks
                .released
                .wait(table)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if table.excludes(&parts, mode, start, end) {
            return Err(err(
                VfsErrorKind::Locked,
                op,
                path,
                "already locked in this filesystem",
            ));
        }
        self.create_if_absent(&parts, op, path)?;
        table.last_hold += 1;
        let id = table.last_hold;
        table.holds.entry(parts.clone()).or_default().push(Hold {
            id,
            process: self.process,
            mode,
            start,
            end,
        });
        Ok(Box::new(MemHold {
            shared: Arc::clone(&self.locks),
            parts,
            id,
        }))
    }

    /// Creates an empty file at `parts` if nothing is there, as opening a lock's file does.
    fn create_if_absent(&self, parts: &[OsString], op: &'static str, path: &Path) -> VfsResult<()> {
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(VfsErrorKind::InvalidInput, op, path, "empty path"));
        };
        let mut guard = self.tree();
        let Inner {
            live,
            live_data,
            refs,
            live_refs,
            next_inode,
            dirty,
            ..
        } = &mut *guard;
        let dir = dir_of_mut(live, parents, op, path)?;
        match dir.get(name) {
            Some(Node::Dir(_)) => Err(err(VfsErrorKind::IsADirectory, op, path, "is a directory")),
            Some(Node::File(_)) => Ok(()),
            None => {
                let id = next_id(next_inode, op, path)?;
                live_data.insert(id, Vec::new());
                dir.insert(name.clone(), Node::File(id));
                *refs.entry(id).or_default() += 1;
                *live_refs.entry(id).or_default() += 1;
                mark(dirty, parents, name);
                Ok(())
            }
        }
    }
}
