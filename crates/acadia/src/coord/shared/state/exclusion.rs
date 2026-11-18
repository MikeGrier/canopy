// Copyright (c) Michael Grier.

use super::*;
use crate::coord::{
    Acquired, ExclusionHold, Generation, ModificationEnd, Purpose, RecoveryDiscovery, RecoveryNeed,
    RestartRecognition, Waiting, WriterExclusion,
};

mod admission;
mod notify;
mod registry;
#[cfg(test)]
mod tests;

/// Tells the participant's step hook of `step` before it acts, in a build with test support.
macro_rules! announce {
    ($participant:expr, $step:ident) => {
        #[cfg(any(test, feature = "test-support"))]
        {
            if let Some(hook) = &$participant.hook {
                hook.before(SharedStep::$step);
            }
        }
    };
}

impl SharedState {
    fn same_participant(&self) -> Self {
        Self {
            vfs: Arc::clone(&self.vfs),
            directory: self.directory.clone(),
            words: Arc::clone(&self.words),
            boot: self.boot.clone(),
            live: Arc::clone(&self.live),
            abandoned: Arc::clone(&self.abandoned),
            holds: Arc::clone(&self.holds),
            #[cfg(any(test, feature = "test-support"))]
            hook: self.hook.clone(),
        }
    }

    fn alive(&self) -> CoordResult<()> {
        self.not_abandoned()?;
        if !self.vfs.is_os_filesystem() && boot(self.vfs.as_ref())? != self.boot {
            return Err(CoordError::Abandoned);
        }
        Ok(())
    }

    /// The part of [`alive`](Self::alive) that makes no filesystem call, for use under the
    /// participant's mutexes once the whole check has been made outside them.
    fn not_abandoned(&self) -> CoordResult<()> {
        if self.abandoned.load(Ordering::Acquire) {
            return Err(CoordError::Abandoned);
        }
        Ok(())
    }

    fn take_lock(&self, path: &Path, mode: LockMode, waiting: Waiting) -> CoordResult<Kept> {
        self.alive()?;
        let lock = match waiting {
            Waiting::Wait => self.vfs.lock(path, mode, LockSpan::File),
            Waiting::Never => self.vfs.try_lock(path, mode, LockSpan::File),
        }
        .map_err(|error| {
            if error.kind() == VfsErrorKind::Locked {
                CoordError::WouldWait
            } else {
                error.into()
            }
        })?;
        self.alive()?;
        let mut held = self
            .holds
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.not_abandoned()?;
        let id = NEXT_INSTANCE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| failure("participant hold identity exhausted"))?;
        held.insert(id, lock);
        Ok(Kept {
            participant: self.same_participant(),
            id,
        })
    }

    fn hold_guard(
        &self,
        id: u64,
    ) -> CoordResult<std::sync::MutexGuard<'_, HashMap<u64, Box<dyn VfsLock>>>> {
        self.alive()?;
        let held = self
            .holds
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.not_abandoned()?;
        if !held.contains_key(&id) {
            return Err(CoordError::Abandoned);
        }
        Ok(held)
    }
}

struct Kept {
    participant: SharedState,
    id: u64,
}

impl Drop for Kept {
    fn drop(&mut self) {
        drop(
            self.participant
                .holds
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&self.id),
        );
    }
}

struct SharedHold {
    kept: Option<Kept>,
    acquired: Acquired,
    depth: u32,
    raised: Option<u64>,
    unresolved: bool,
}

impl std::fmt::Debug for SharedHold {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedHold")
            .field("acquired", &self.acquired)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

impl WriterExclusion for SharedState {
    fn acquire(&self, waiting: Waiting) -> CoordResult<Box<dyn ExclusionHold>> {
        let kept = self.take_lock(
            &self.directory.join("exclusion.lock"),
            LockMode::Exclusive,
            waiting,
        )?;
        let acquired = {
            let current_boot = boot(self.vfs.as_ref())?;
            if current_boot != self.boot {
                return Err(CoordError::Abandoned);
            }
            let _held = self.hold_guard(kept.id)?;
            let previous_owner = self
                .words
                .word(Word::Owner)
                .fetch_or(format::FALSE, Ordering::AcqRel);
            let holder_died = previous_owner != format::FALSE;
            if holder_died {
                announce!(self, SetFence);
                self.words
                    .word(Word::Fence)
                    .store(format::TRUE, Ordering::Release);
            }
            let mut snapshot = self.snapshot_at(&current_boot)?;
            if !snapshot.modification_count.is_multiple_of(2) {
                announce!(self, SetFence);
                self.words
                    .word(Word::Fence)
                    .store(format::TRUE, Ordering::Release);
                snapshot.fenced = true;
            }
            let mut found = self.words.word(Word::Generation).load(Ordering::Acquire);
            if found == u64::MAX {
                return Err(CoordError::Exhausted { what: "generation" });
            }
            if found
                >= self
                    .words
                    .word(Word::GenerationLimit)
                    .load(Ordering::Acquire)
            {
                let (base, limit) = Self::reserve_generations(self.vfs.as_ref(), &self.directory)?;
                found = base;
                self.words
                    .word(Word::GenerationLimit)
                    .store(limit, Ordering::Release);
            }
            let next = found
                .checked_add(1)
                .ok_or(CoordError::Exhausted { what: "generation" })?;
            announce!(self, Record);
            self.words.word(Word::Owner).store(next, Ordering::Release);
            self.words
                .word(Word::Generation)
                .store(next, Ordering::Release);
            Acquired::new(
                Generation::new(found),
                Generation::new(next),
                holder_died,
                self.fence_set(),
                snapshot.trust,
            )
        };
        Ok(Box::new(SharedHold {
            kept: Some(kept),
            acquired,
            depth: 0,
            raised: None,
            unresolved: false,
        }))
    }
}

impl SharedHold {
    fn release_cleanly(&mut self) -> CoordResult<()> {
        let Some(kept) = self.kept.take() else {
            return Ok(());
        };
        let participant = &kept.participant;
        let cleared = (|| {
            let _held = participant.hold_guard(kept.id)?;
            if self.depth > 0 {
                participant
                    .words
                    .word(Word::Fence)
                    .store(format::TRUE, Ordering::Release);
            }
            announce!(participant, ClearRecord);
            participant
                .words
                .word(Word::Owner)
                .compare_exchange(
                    self.acquired.advanced_to().get(),
                    format::FALSE,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .map_err(|_| failure("writer ownership changed beneath its hold"))?;
            Ok(())
        })();
        drop(kept);
        match cleared {
            Err(CoordError::Abandoned) => Ok(()),
            result => result,
        }
    }
}

impl ExclusionHold for SharedHold {
    fn acquired(&self) -> &Acquired {
        &self.acquired
    }

    fn begin_modification(&mut self, purpose: Purpose) -> CoordResult<()> {
        let kept = self.kept.as_ref().ok_or(CoordError::Abandoned)?;
        let participant = &kept.participant;
        let current_boot = boot(participant.vfs.as_ref())?;
        let _held = participant.hold_guard(kept.id)?;
        if purpose == Purpose::Ordinary && participant.snapshot_at(&current_boot)?.fenced {
            return Err(CoordError::RecoveryRequired);
        }
        let next_depth = self.depth.checked_add(1).ok_or(CoordError::Exhausted {
            what: "modification nesting",
        })?;
        if self.depth == 0 {
            announce!(participant, BeginRaise);
            self.unresolved = false;
            self.raised = None;
            let current = participant.words.word(Word::Count).load(Ordering::Acquire);
            if current == u64::MAX {
                return Err(CoordError::Exhausted {
                    what: "modification count",
                });
            }
            if current.is_multiple_of(2) {
                current.checked_add(2).ok_or(CoordError::Exhausted {
                    what: "modification count",
                })?;
                let odd = current + 1;
                announce!(participant, PublishRaise);
                participant
                    .words
                    .word(Word::Count)
                    .compare_exchange(current, odd, Ordering::AcqRel, Ordering::Acquire)
                    .map_err(|_| failure("modification count changed beneath its hold"))?;
                self.raised = Some(odd);
            }
        }
        self.depth = next_depth;
        Ok(())
    }

    fn end_modification(&mut self, end: ModificationEnd) -> CoordResult<()> {
        let kept = self.kept.as_ref().ok_or(CoordError::Abandoned)?;
        let participant = &kept.participant;
        let _held = participant.hold_guard(kept.id)?;
        if self.depth == 0 {
            return Err(CoordError::NoModificationOpen);
        }
        if end == ModificationEnd::Unresolved {
            announce!(participant, Unresolved);
            self.unresolved = true;
            participant
                .words
                .word(Word::Fence)
                .store(format::TRUE, Ordering::Release);
        }
        self.depth -= 1;
        if self.depth == 0
            && !self.unresolved
            && let Some(odd) = self.raised.take()
        {
            announce!(participant, BeginClose);
            let even = odd.checked_add(1).ok_or_else(|| {
                participant
                    .words
                    .word(Word::Fence)
                    .store(format::TRUE, Ordering::Release);
                CoordError::Exhausted {
                    what: "modification count",
                }
            })?;
            announce!(participant, PublishClose);
            participant
                .words
                .word(Word::Count)
                .compare_exchange(odd, even, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| {
                    participant
                        .words
                        .word(Word::Fence)
                        .store(format::TRUE, Ordering::Release);
                    failure("modification count changed beneath its hold")
                })?;
        }
        Ok(())
    }

    fn complete_recovery(&mut self) -> CoordResult<()> {
        let kept = self.kept.as_ref().ok_or(CoordError::Abandoned)?;
        let participant = &kept.participant;
        let current_boot = boot(participant.vfs.as_ref())?;
        let _held = participant.hold_guard(kept.id)?;
        if self.depth > 0 {
            return Err(CoordError::ModificationOpen);
        }
        if current_boot != participant.boot || participant.boot.is_none() {
            return Err(CoordError::RecoveryRequired);
        }
        if participant.words.word(Word::Ready).load(Ordering::Acquire) != format::READY {
            return Err(CoordError::Malformed {
                what: "shared-state header",
            });
        }
        let current = participant.words.word(Word::Count).load(Ordering::Acquire);
        let even = if current.is_multiple_of(2) {
            current
        } else {
            current.checked_add(1).ok_or(CoordError::Exhausted {
                what: "modification count",
            })?
        };
        announce!(participant, PublishRecovered);
        // Vouched before the count: a sample that reads the even count synchronizes with its
        // publication and so sees the state vouched, and a death between the two leaves an odd
        // count beside vouched state, which the dead holder's ownership still fences.
        let vouched = participant.words.word(Word::Vouched);
        let was_vouched = vouched.swap(format::TRUE, Ordering::AcqRel);
        if participant
            .words
            .word(Word::Count)
            .compare_exchange(current, even, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            vouched.store(was_vouched, Ordering::Release);
            return Err(failure("recovery count changed beneath its hold"));
        }
        announce!(participant, ClearFence);
        participant
            .words
            .word(Word::Fence)
            .store(format::FALSE, Ordering::Release);
        self.unresolved = false;
        Ok(())
    }

    fn release(mut self: Box<Self>) -> CoordResult<()> {
        self.release_cleanly()
    }
}

impl Drop for SharedHold {
    fn drop(&mut self) {
        let _ = self.release_cleanly();
    }
}

impl RestartRecognition for SharedState {
    fn trust(&self) -> CoordResult<Trust> {
        Ok(self.snapshot()?.trust)
    }
}

impl RecoveryDiscovery for SharedState {
    fn discover_recovery_with(&self, inspect: &mut dyn FnMut()) -> CoordResult<RecoveryNeed> {
        // Shared, so it excludes every writer and no other prober.
        match self.take_lock(
            &self.directory.join("exclusion.lock"),
            LockMode::Shared,
            Waiting::Never,
        ) {
            Ok(_probe) => {
                {
                    let _held = self.hold_guard(_probe.id)?;
                    self.words
                        .word(Word::Owner)
                        .fetch_or(format::FALSE, Ordering::AcqRel);
                }
                let snapshot = self.snapshot()?;
                {
                    // Under the table `abandon` clears, so the probe is not released while it runs.
                    let _held = self.hold_guard(_probe.id)?;
                    inspect();
                }
                Ok(RecoveryNeed::new(
                    self.fence_set(),
                    match snapshot.trust {
                        Trust::Vouched => None,
                        Trust::Untrusted(reason) => Some(reason),
                    },
                    snapshot.owner != format::FALSE,
                ))
            }
            Err(CoordError::WouldWait) => {
                let snapshot = self.snapshot()?;
                Ok(RecoveryNeed::new(
                    self.fence_set(),
                    match snapshot.trust {
                        Trust::Vouched => None,
                        Trust::Untrusted(reason) => Some(reason),
                    },
                    false,
                ))
            }
            Err(error) => Err(error),
        }
    }
}
