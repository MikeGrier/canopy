// Copyright (c) Michael Grier.

//! The locks of a [`MemFs`]: which holder has which bytes of which file, in which mode, and the
//! simulated processes those holders belong to.
//!
//! The table is always taken before the tree. An operation that checks the holds keeps the table
//! until its change to the tree is made, so a lock cannot be taken between its check and its
//! change; taking a lock likewise holds the table and then the tree.

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
    pub(super) released: Condvar,
    /// Run by every operation just after it takes the holds, while it keeps the table.
    #[cfg(test)]
    after_check: Mutex<Option<CheckHook>>,
    /// Run immediately before a conflicting lock wait releases the table.
    #[cfg(test)]
    before_wait: Mutex<Option<CheckHook>>,
}

#[cfg(test)]
#[derive(Clone)]
struct CheckHook(Arc<dyn Fn() + Send + Sync>);

#[cfg(test)]
impl std::fmt::Debug for CheckHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CheckHook")
    }
}

/// Every lock held in one filesystem, by every simulated process over it.
#[derive(Debug, Default)]
pub(super) struct LockTable {
    /// The holds on each file, keyed by the file rather than a name of it, so a lock follows its
    /// file through a rename as a native lock follows its handle.
    pub(super) holds: HashMap<u64, Vec<Hold>>,
    /// The last hold id handed out. Never reused, so a guard whose hold was already released by
    /// an abandonment can never release a later holder's.
    last_hold: u64,
    /// The last simulated process id handed out; the first process is `0`.
    pub(super) last_process: u64,
    /// All process identities through this one ended in the latest crash.
    pub(super) ended_through: Option<u64>,
    /// Process views abandoned after the latest crash.
    pub(super) ended: HashSet<u64>,
    #[cfg(test)]
    panic_on_lock_drop: HashSet<Vec<OsString>>,
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
    pub(super) fn running(&self, process: u64, op: &'static str, path: &Path) -> VfsResult<()> {
        if self
            .ended_through
            .is_some_and(|ended_through| process <= ended_through)
            || self.ended.contains(&process)
        {
            return Err(err(
                VfsErrorKind::ProcessEnded,
                op,
                path,
                "this process view has ended",
            ));
        }
        Ok(())
    }

    fn excludes(&self, file: u64, mode: LockMode, start: u64, end: u64) -> bool {
        self.holds
            .get(&file)
            .is_some_and(|holds| holds.iter().any(|hold| hold.excludes(mode, start, end)))
    }
}

impl Hold {
    /// Whether `[start, end)` shares a byte with this hold; an empty range shares none.
    fn overlaps(&self, start: u64, end: u64) -> bool {
        start < end && self.start < end && start < self.end
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
/// bytes it touches once it knows them. Keeps the table, so no lock is taken until it drops.
pub(super) struct Holds<'a> {
    table: MutexGuard<'a, LockTable>,
}

impl Holds<'_> {
    /// Refuses `access` to `[start, end)` with [`VfsErrorKind::Locked`] if a hold covers it.
    pub(super) fn check(
        &self,
        file: u64,
        access: Access,
        start: u64,
        end: u64,
        op: &'static str,
        path: &Path,
    ) -> VfsResult<()> {
        let holds = self.table.holds.get(&file).map_or(&[][..], Vec::as_slice);
        if holds.iter().any(|hold| hold.refuses(access, start, end)) {
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
    file: u64,
    id: u64,
    #[cfg(test)]
    panic_on_drop: bool,
}

impl VfsLock for MemHold {}

impl Drop for MemHold {
    fn drop(&mut self) {
        let mut table = self
            .shared
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(holds) = table.holds.get_mut(&self.file) {
            holds.retain(|hold| hold.id != self.id);
            if holds.is_empty() {
                table.holds.remove(&self.file);
            }
        }
        drop(table);
        self.shared.released.notify_all();
        #[cfg(test)]
        if self.panic_on_drop {
            panic!("configured lock destructor stopped");
        }
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
            process: AtomicU64::new(table.last_process),
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
        let mut table = self.table();
        let process = self.process.load(Ordering::Acquire);
        if table
            .ended_through
            .is_none_or(|ended_through| process > ended_through)
        {
            table.ended.insert(process);
        }
        table.holds.retain(|_, holds| {
            holds.retain(|hold| hold.process != process);
            !holds.is_empty()
        });
        drop(table);
        self.locks.released.notify_all();
    }

    pub(super) fn table(&self) -> MutexGuard<'_, LockTable> {
        self.locks
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The holds, for an operation to check once it knows the file and the bytes it touches, and
    /// to keep until its change is made.
    pub(super) fn holds(&self, op: &'static str, path: &Path) -> VfsResult<Holds<'_>> {
        let table = self.table();
        table.running(self.process.load(Ordering::Acquire), op, path)?;
        #[cfg(test)]
        self.run_after_check();
        Ok(Holds { table })
    }

    /// Runs `hook` in every operation of this filesystem just after it takes the holds it
    /// checks, while it keeps them.
    #[cfg(test)]
    pub(super) fn run_after_check(&self) {
        if let Some(hook) = self
            .locks
            .after_check
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            (hook.0)();
        }
    }

    #[cfg(test)]
    pub(crate) fn after_lock_check(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self
            .locks
            .after_check
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(CheckHook(Arc::new(hook)));
    }

    #[cfg(test)]
    pub(crate) fn panic_when_next_lock_drops(&self, path: &Path) {
        let parts = split_path(path, "panic_when_next_lock_drops").expect("valid lock path");
        self.table().panic_on_lock_drop.insert(parts);
    }

    #[cfg(test)]
    pub(crate) fn before_lock_wait(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self
            .locks
            .before_wait
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(CheckHook(Arc::new(hook)));
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
        create: bool,
    ) -> VfsResult<Box<dyn VfsLock>> {
        let mut table = self.table();
        let process = self.process.load(Ordering::Acquire);
        table.running(process, op, path)?;
        let parts = split_path(path, op)?;
        let Some((start, end)) = span.bounds() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                op,
                path,
                "a range must be non-empty and end at or before LockSpan::MAX_END",
            ));
        };
        // Opened first, as a native lock opens its handle: a waiter keeps the file it opened
        // whatever names it has by the time it is let in.
        let file = if create {
            self.create_if_absent(&parts, op, path)?
        } else {
            self.existing_file(&parts, op, path)?
        };
        while wait && table.excludes(file, mode, start, end) {
            #[cfg(test)]
            if let Some(hook) = self
                .locks
                .before_wait
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            {
                (hook.0)();
            }
            table = self
                .locks
                .released
                .wait(table)
                .unwrap_or_else(PoisonError::into_inner);
            table.running(process, op, path)?;
        }
        if table.excludes(file, mode, start, end) {
            return Err(err(
                VfsErrorKind::Locked,
                op,
                path,
                "already locked in this filesystem",
            ));
        }
        table.last_hold += 1;
        let id = table.last_hold;
        table.holds.entry(file).or_default().push(Hold {
            id,
            process,
            mode,
            start,
            end,
        });
        #[cfg(test)]
        let panic_on_drop = table.panic_on_lock_drop.remove(&parts);
        Ok(Box::new(MemHold {
            shared: Arc::clone(&self.locks),
            file,
            id,
            #[cfg(test)]
            panic_on_drop,
        }))
    }

    fn existing_file(&self, parts: &[OsString], op: &'static str, path: &Path) -> VfsResult<u64> {
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(VfsErrorKind::InvalidInput, op, path, "empty path"));
        };
        let guard = self.tree();
        let dir = dir_of(&guard.live, parents, op, path)?;
        match dir.get(name) {
            Some(Node::File(id)) => Ok(*id),
            Some(Node::Dir(_)) => Err(err(VfsErrorKind::IsADirectory, op, path, "is a directory")),
            None => Err(err(
                VfsErrorKind::NotFound,
                op,
                path,
                "lock file does not exist",
            )),
        }
    }

    /// Creates an empty file at `parts` if nothing is there, as opening a lock's file does, and
    /// returns the file.
    fn create_if_absent(
        &self,
        parts: &[OsString],
        op: &'static str,
        path: &Path,
    ) -> VfsResult<u64> {
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
            Some(Node::File(id)) => Ok(*id),
            None => {
                let id = next_id(next_inode, op, path)?;
                live_data.insert(id, Vec::new());
                dir.insert(name.clone(), Node::File(id));
                *refs.entry(id).or_default() += 1;
                *live_refs.entry(id).or_default() += 1;
                mark(dirty, parents, name);
                Ok(id)
            }
        }
    }
}
