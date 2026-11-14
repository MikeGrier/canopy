// Copyright (c) Michael Grier.

//! Writer exclusion for the file-based implementation: one lock file, held whole and exclusively.
//!
//! The lock is a [`Vfs::lock`](crate::Vfs::lock) lock, so it excludes every other holder on the
//! host -- another participant in this process as much as one in another process -- waits without
//! promising any order among waiters, and is released by the operating system when its holder's
//! process dies. It is kept in the participant's table of holds, so abandoning the participant
//! releases it as a death does.

use std::sync::Arc;

use crate::coord::{CoordResult, Waiting};
use crate::{LockMode, LockSpan};

use super::{FileCoordination, HoldId};

/// A hold of writer exclusion, released when dropped.
#[derive(Debug)]
pub(crate) struct Exclusion {
    participant: FileCoordination,
    hold: HoldId,
}

impl FileCoordination {
    /// Another handle on this same participant, sharing its table of holds, for a hold that must
    /// outlive the borrow it was taken through.
    pub(super) fn same_participant(&self) -> FileCoordination {
        FileCoordination {
            id: self.id.clone(),
            vfs: Arc::clone(&self.vfs),
            layout: self.layout.clone(),
            holds: Arc::clone(&self.holds),
        }
    }

    /// Acquires writer exclusion, waiting as `waiting` allows. Exclusion is not re-entrant: a
    /// participant that holds it and waits for it again waits for itself.
    pub(crate) fn acquire_exclusion(&self, waiting: Waiting) -> CoordResult<Exclusion> {
        self.alive()?;
        let layout = self.layout();
        self.vfs().create_dir_all(&layout.state_dir())?;
        let hold = self.take_lock(
            &layout.exclusion_lock(),
            LockMode::Exclusive,
            LockSpan::File,
            waiting,
        )?;
        Ok(Exclusion {
            participant: self.same_participant(),
            hold,
        })
    }
}

impl Exclusion {
    /// The participant that holds it.
    pub(crate) fn participant(&self) -> &FileCoordination {
        &self.participant
    }
}

impl Drop for Exclusion {
    fn drop(&mut self) {
        drop(self.participant.release_lock(self.hold));
    }
}
