// Copyright (c) Michael Grier.

//! [`Admission`] for the file-based implementation: the gate is one lock file and the tiers are a
//! byte each of another, all taken with [`Vfs::lock`](crate::Vfs::lock) and kept in the
//! participant's table of holds, so a death -- or an abandonment -- releases them all at once. An
//! attachment's or an upgrade's registry entry is released before its tiers.

use crate::coord::{
    Access, Admission, Attachment, CoordResult, HeldTiers, ReaderSafety, Registrant, Registration,
    TierHold, Upgrade, Waiting,
};
use crate::{LockMode, LockSpan};

use super::layout::Layout;
use super::registry::Entry;
use super::{FileCoordination, HoldId};

mod conversion;
#[cfg(test)]
mod tests;

/// The locks one attachment or upgrade holds: its registry entry, released first, then the rest in
/// reverse of the order taken.
#[derive(Debug)]
struct Held {
    participant: FileCoordination,
    /// In the order taken.
    holds: Vec<HoldId>,
    entry: Option<Entry>,
}

impl Held {
    fn new(participant: &FileCoordination) -> Self {
        Held {
            participant: participant.same_participant(),
            holds: Vec::new(),
            entry: None,
        }
    }

    fn take(
        &mut self,
        path: &std::path::Path,
        mode: LockMode,
        span: LockSpan,
        waiting: Waiting,
    ) -> CoordResult<HoldId> {
        let hold = self.participant.take_lock(path, mode, span, waiting)?;
        self.holds.push(hold);
        Ok(hold)
    }

    fn release(&mut self, hold: HoldId) -> CoordResult<()> {
        self.holds.retain(|held| *held != hold);
        self.participant.release_lock_safely(hold);
        self.participant.alive()
    }

    fn register(&mut self, tiers: HeldTiers, payload: &[u8]) -> CoordResult<Registration> {
        self.participant
            .register_entry(&mut self.entry, tiers, payload)
    }

    fn release_all(&mut self) {
        if let Some(entry) = self.entry.take() {
            self.participant.unregister_entry(entry);
        }
        while let Some(hold) = self.holds.pop() {
            self.participant.release_lock_safely(hold);
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.participant.abandon_participant();
            return;
        }
        self.release_all();
    }
}

/// The file-based implementation's [`Attachment`].
#[derive(Debug)]
pub(crate) struct FileAttachment {
    access: Access,
    held: Held,
}

/// The file-based implementation's [`Upgrade`].
#[derive(Debug)]
pub(crate) struct FileUpgrade {
    safety: ReaderSafety,
    held: Held,
    gate: Option<HoldId>,
}

impl FileCoordination {
    fn attach_file(&self, access: Access, waiting: Waiting) -> CoordResult<FileAttachment> {
        self.alive()?;
        let layout = self.layout();
        self.vfs().create_dir_all(&layout.state_dir())?;
        let mut held = Held::new(self);
        let gate = held.take(
            &layout.gate_lock(),
            LockMode::Shared,
            LockSpan::File,
            waiting,
        )?;
        held.take(
            &layout.tiers_lock(),
            LockMode::Shared,
            Layout::reader_tier(),
            waiting,
        )?;
        held.release(gate)?;
        if access == Access::Write {
            held.take(
                &layout.tiers_lock(),
                LockMode::Shared,
                Layout::writer_tier(),
                waiting,
            )?;
        }
        Ok(FileAttachment { access, held })
    }
}

impl Admission for FileCoordination {
    fn attach(&self, access: Access, waiting: Waiting) -> CoordResult<Box<dyn Attachment>> {
        self.check_implementation()?;
        Ok(Box::new(self.attach_file(access, waiting)?))
    }
}

impl Attachment for FileAttachment {
    fn access(&self) -> Access {
        self.access
    }

    fn tiers(&self) -> HeldTiers {
        HeldTiers {
            reader: Some(TierHold::Shared),
            writer: (self.access == Access::Write).then_some(TierHold::Shared),
        }
    }

    fn begin_conversion(
        mut self: Box<Self>,
    ) -> CoordResult<Box<dyn crate::coord::UpgradeConversion>> {
        self.held.release_all();
        self.held.participant.alive()?;
        Ok(Box::new(conversion::FileConversion::new(
            &self.held.participant,
        )))
    }

    fn upgrade(
        mut self: Box<Self>,
        safety: ReaderSafety,
        waiting: Waiting,
    ) -> CoordResult<Box<dyn Upgrade>> {
        self.held.release_all();
        let participant = self.held.participant.same_participant();
        drop(self);
        participant.alive()?;
        let layout = participant.layout();
        let mut held = Held::new(&participant);
        let gate = held.take(
            &layout.gate_lock(),
            LockMode::Exclusive,
            LockSpan::File,
            waiting,
        )?;
        if safety == ReaderSafety::Unsafe {
            held.take(
                &layout.tiers_lock(),
                LockMode::Exclusive,
                Layout::reader_tier(),
                waiting,
            )?;
        }
        held.take(
            &layout.tiers_lock(),
            LockMode::Exclusive,
            Layout::writer_tier(),
            waiting,
        )?;
        if safety == ReaderSafety::Safe {
            held.release(gate)?;
        }
        Ok(Box::new(FileUpgrade {
            safety,
            held,
            gate: (safety == ReaderSafety::Unsafe).then_some(gate),
        }))
    }
}

impl Registrant for FileAttachment {
    fn register(&mut self, payload: &[u8]) -> CoordResult<Registration> {
        let tiers = self.tiers();
        self.held.register(tiers, payload)
    }
}

impl Registrant for FileUpgrade {
    fn register(&mut self, payload: &[u8]) -> CoordResult<Registration> {
        let tiers = self.tiers();
        self.held.register(tiers, payload)
    }
}

impl Upgrade for FileUpgrade {
    fn safety(&self) -> ReaderSafety {
        self.safety
    }

    fn tiers(&self) -> HeldTiers {
        HeldTiers {
            reader: (self.safety == ReaderSafety::Unsafe).then_some(TierHold::Exclusive),
            writer: Some(TierHold::Exclusive),
        }
    }

    fn admit_readers(&mut self) -> CoordResult<()> {
        self.held.participant.alive()?;
        if self.safety == ReaderSafety::Safe
            && let Some(gate) = self.gate.take()
        {
            self.held.release(gate)?;
        }
        Ok(())
    }
}
