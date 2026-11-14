// Copyright (c) Michael Grier.

//! [`WriterExclusion`] for the file-based implementation: the hold over the exclusion lock, with the
//! ownership record, the fence, the generation and the boot record kept under it.
//!
//! Acquiring takes the lock, sets the fence if an ownership record was left behind, remakes the boot
//! record untrusted if it is from another boot or missing, advances the generation, and writes this
//! holder's record last, so a death at any step leaves the previous record for the next acquirer. A
//! clean release removes the record while it still holds the lock and only then drops the lock, so a
//! record found under the lock always means its holder died. All of these files are replaced whole
//! and never synced: a restart, the one thing that could lose them, makes the state untrusted.

use std::fmt;

use crate::VfsErrorKind;
use crate::coord::{
    Acquired, CoordError, CoordResult, ExclusionHold, Generation, ModificationEnd, Purpose,
    RestartRecognition, Waiting, WriterExclusion,
};

use super::FileCoordination;
use super::count::raising::Modifications;
use super::exclusion::Exclusion;
use super::restart::Standing;

/// The generation's and the ownership record's encodings. Changing any value is a breaking change
/// to the state's format.
mod format {
    /// The bytes the generation file begins with, before the generation, little-endian.
    pub const GENERATION_MAGIC: [u8; 4] = *b"ACGN";
    /// The bytes an ownership record begins with, before the holder's process id, little-endian.
    pub const OWNER_MAGIC: [u8; 4] = *b"ACOW";
}

/// The file-based implementation's [`ExclusionHold`].
pub(crate) struct FileHold {
    acquired: Acquired,
    modifications: Modifications,
    /// Whether this hold has completed a recovery, which lifts what the acquisition found.
    recovered: bool,
    /// Whether this hold has set the fence, which refuses its own ordinary modifications too.
    fenced: bool,
    /// Taken by a release; dropped last, so the record is cleared while the lock is held.
    exclusion: Option<Exclusion>,
}

impl fmt::Debug for FileHold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileHold")
            .field("acquired", &self.acquired)
            .field("modifications", &self.modifications)
            .finish_non_exhaustive()
    }
}

impl FileCoordination {
    /// The generation, which may be unreadable only when the state is from another boot: within a
    /// boot it is replaced whole, and no participant from before a restart remembers one.
    fn read_generation(&self, remade: bool) -> CoordResult<Generation> {
        match self.vfs().read(&self.layout().generation()) {
            Ok(bytes) => bytes
                .strip_prefix(&format::GENERATION_MAGIC)
                .and_then(|value| value.try_into().ok())
                .map(|value| Generation::new(u64::from_le_bytes(value)))
                .or(remade.then_some(Generation::new(0)))
                .ok_or(CoordError::Malformed { what: "generation" }),
            Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(Generation::new(0)),
            Err(error) => Err(error.into()),
        }
    }

    fn write_generation(&self, generation: Generation) -> CoordResult<()> {
        let layout = self.layout();
        let mut bytes = format::GENERATION_MAGIC.to_vec();
        bytes.extend_from_slice(&generation.get().to_le_bytes());
        self.replace_whole(
            &layout.generation_staging(&super::staging_suffix()),
            &layout.generation(),
            &bytes,
        )
    }

    fn write_owner(&self) -> CoordResult<()> {
        let layout = self.layout();
        let mut bytes = format::OWNER_MAGIC.to_vec();
        bytes.extend_from_slice(&std::process::id().to_le_bytes());
        self.replace_whole(
            &layout.owner_staging(&super::staging_suffix()),
            &layout.owner(),
            &bytes,
        )
    }

    fn clear_owner(&self) -> CoordResult<()> {
        match self.vfs().remove_file(&self.layout().owner()) {
            Err(error) if error.kind() != VfsErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    fn fence_is_set(&self) -> CoordResult<bool> {
        Ok(self.vfs().exists(&self.layout().fence())?)
    }

    fn acquire_hold(&self, waiting: Waiting) -> CoordResult<FileHold> {
        let exclusion = self.acquire_exclusion(waiting)?;
        let holder_died = self.vfs().exists(&self.layout().owner())?;
        if holder_died {
            self.set_fence()?;
        }
        let trust = self.trust()?;
        let remade = self.boot_record_needs_remaking()?;
        if remade {
            self.record_boot(Standing::Untrusted)?;
        }
        let found = self.read_generation(remade)?;
        let advanced_to = Generation::new(found.get() + 1);
        self.write_generation(advanced_to)?;
        let fenced = self.fence_is_set()?;
        self.write_owner()?;
        Ok(FileHold {
            acquired: Acquired::new(found, advanced_to, holder_died, fenced, trust),
            modifications: Modifications::default(),
            recovered: false,
            fenced: false,
            exclusion: Some(exclusion),
        })
    }
}

impl FileHold {
    fn participant(&self) -> &FileCoordination {
        self.exclusion
            .as_ref()
            .expect("a hold has its exclusion until it is released")
            .participant()
    }

    fn recovery_required(&self) -> bool {
        self.fenced || (!self.recovered && self.acquired.recovery_required())
    }

    /// Clears the ownership record, then drops the lock. A participant that was abandoned does no
    /// bookkeeping, as a dead one cannot.
    fn release_cleanly(&mut self) -> CoordResult<()> {
        let Some(exclusion) = self.exclusion.take() else {
            return Ok(());
        };
        let participant = exclusion.participant();
        let cleared = match participant.alive() {
            Ok(()) => participant.clear_owner(),
            Err(_) => Ok(()),
        };
        drop(exclusion);
        cleared
    }
}

impl ExclusionHold for FileHold {
    fn acquired(&self) -> &Acquired {
        &self.acquired
    }

    fn begin_modification(&mut self, purpose: Purpose) -> CoordResult<()> {
        if purpose == Purpose::Ordinary && self.recovery_required() {
            return Err(CoordError::RecoveryRequired);
        }
        let participant = self.participant().same_participant();
        participant.begin_modification(&mut self.modifications)
    }

    fn end_modification(&mut self, end: ModificationEnd) -> CoordResult<()> {
        let participant = self.participant().same_participant();
        let ended = participant.end_modification(&mut self.modifications, end);
        if end == ModificationEnd::Unresolved || ended.is_err() {
            self.fenced = true;
        }
        ended
    }

    fn complete_recovery(&mut self) -> CoordResult<()> {
        if self.modifications.is_open() {
            return Err(CoordError::ModificationOpen);
        }
        let participant = self.participant();
        let published = participant.read_count()?;
        participant.publish_count(published.value.next_multiple_of(2), Standing::Vouched)?;
        participant.record_boot(Standing::Vouched)?;
        match participant.vfs().remove_file(&participant.layout().fence()) {
            Err(error) if error.kind() != VfsErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        self.recovered = true;
        self.fenced = false;
        Ok(())
    }

    fn release(mut self: Box<Self>) -> CoordResult<()> {
        self.release_cleanly()
    }
}

impl Drop for FileHold {
    fn drop(&mut self) {
        let _ = self.release_cleanly();
    }
}

impl WriterExclusion for FileCoordination {
    fn acquire(&self, waiting: Waiting) -> CoordResult<Box<dyn ExclusionHold>> {
        Ok(Box::new(self.acquire_hold(waiting)?))
    }
}
