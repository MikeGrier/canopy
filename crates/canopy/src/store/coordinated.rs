// Copyright (c) Michael Grier.

//! A store's part in the coordination state: the participant that samples the modification count
//! and, in a handle that may write, the hold of writer exclusion that raises it (`D335`; canopy's
//! DESIGN-NOTES.md, "The modification count is the coordination state's").

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use acadia::coord::{
    Acquired, Attachment, CoordError, Coordination, ExclusionHold, ModCount, ModificationEnd,
    Notice, Purpose, RecoveryNeed, Trust,
};

use super::StoreError;
use super::writer::WriterGate;

/// The coordination state a store handle shares with its clones.
///
/// A handle that may write holds writer exclusion for as long as any clone lives, as it holds the
/// root's lock (`D116`), so the count it raises is the one count of the root.
pub(crate) struct Coordinated {
    /// Dropped before the participant, so the ownership record is cleared while the participant
    /// that wrote it is still there to clear it.
    hold: Option<Mutex<Box<dyn ExclusionHold>>>,
    // Exclusion ends before attachment; the attachment unregisters before releasing its tiers.
    _attachment: Mutex<Box<dyn Attachment>>,
    participant: Box<dyn Coordination>,
    /// Whether modifications are recovery's, which a store whose coordination state needs
    /// recovery may make before that recovery has completed.
    recovering: AtomicBool,
}

impl Coordinated {
    pub(crate) fn list(&self) -> Result<acadia::coord::Listing, StoreError> {
        Ok(self.participant.list()?)
    }

    pub(crate) fn implementation(&self) -> &acadia::coord::ImplementationId {
        self.participant.implementation()
    }

    /// A handle holding shared reader attachment, without writer exclusion.
    pub(crate) fn reader(
        participant: Box<dyn Coordination>,
        attachment: Box<dyn Attachment>,
    ) -> Self {
        Coordinated {
            hold: None,
            _attachment: Mutex::new(attachment),
            participant,
            recovering: AtomicBool::new(false),
        }
    }

    /// A handle that raises the count through `hold`, a hold of writer exclusion `participant`
    /// granted.
    pub(crate) fn writer(
        participant: Box<dyn Coordination>,
        attachment: Box<dyn Attachment>,
        hold: Box<dyn ExclusionHold>,
    ) -> Self {
        Coordinated {
            hold: Some(Mutex::new(hold)),
            _attachment: Mutex::new(attachment),
            participant,
            recovering: AtomicBool::new(false),
        }
    }

    pub(crate) fn may_write(&self) -> bool {
        self.hold.is_some()
    }

    fn hold(&self) -> Option<MutexGuard<'_, Box<dyn ExclusionHold>>> {
        self.hold
            .as_ref()
            .map(|hold| hold.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The count as the coordination state publishes it: odd while a modification is in progress,
    /// while a holder that died left one unfinished, and while the state cannot be vouched for.
    pub(crate) fn sample(&self) -> Result<u64, StoreError> {
        Ok(self.sample_count()?.get())
    }

    pub(crate) fn sample_count(&self) -> Result<ModCount, StoreError> {
        Ok(self.participant.sample()?)
    }

    pub(crate) fn notify(
        &self,
        sampled: ModCount,
        deadline: Instant,
        ring: Box<dyn FnOnce() + Send>,
    ) -> Result<Box<dyn Notice>, StoreError> {
        Ok(self.participant.notify(sampled, deadline, ring)?)
    }

    /// Makes the modifications begun from here on recovery's, or ordinary again.
    pub(crate) fn set_recovering(&self, recovering: bool) {
        self.recovering.store(recovering, Ordering::Release);
    }

    pub(crate) fn is_recovering(&self) -> bool {
        self.recovering.load(Ordering::Acquire)
    }

    /// What acquiring writer exclusion found, for a handle that holds it.
    pub(crate) fn acquired(&self) -> Option<Acquired> {
        self.hold().map(|hold| *hold.acquired())
    }

    /// Whether acquiring writer exclusion found the state needing recovery, which no recovery has
    /// yet completed.
    pub(crate) fn recovery_was_required(&self) -> bool {
        self.hold()
            .is_some_and(|hold| hold.acquired().recovery_required())
    }

    /// Whether the state acquiring writer exclusion found could be vouched for, so that what
    /// required recovery, if anything did, was a fence and not the state itself.
    pub(crate) fn state_was_vouched(&self) -> bool {
        self.hold()
            .is_some_and(|hold| hold.acquired().trust() == Trust::Vouched)
    }

    /// Whether recovery is required, and why, observed as one observation that changes nothing.
    pub(crate) fn discover(&self) -> Result<RecoveryNeed, StoreError> {
        Ok(self.participant.discover_recovery()?)
    }

    /// Completes recovery: publishes the even count and the trusted state, and clears the fence
    /// last, so a failure at any point leaves the fence set. Idempotent.
    pub(crate) fn complete_recovery(&self) -> Result<(), StoreError> {
        match self.hold() {
            Some(mut hold) => Ok(hold.complete_recovery()?),
            None => Ok(()),
        }
    }

    /// Begins a modification through the hold. The outermost one raises the count to odd before
    /// the caller's first effect.
    pub(crate) fn begin(&self, root: &std::path::Path) -> Result<(), StoreError> {
        let Some(mut hold) = self.hold() else {
            return Err(StoreError::ReadOnly {
                root: root.to_path_buf(),
            });
        };
        let purpose = if self.recovering.load(Ordering::Acquire) {
            Purpose::Recovery
        } else {
            Purpose::Ordinary
        };
        hold.begin_modification(purpose)
            .map_err(|error| match error {
                CoordError::RecoveryRequired => StoreError::RecoveryRequired {
                    root: root.to_path_buf(),
                },
                other => StoreError::Coordination(other),
            })
    }

    fn end(&self, end: ModificationEnd) -> Result<(), CoordError> {
        match self.hold() {
            Some(mut hold) => hold.end_modification(end),
            None => Ok(()),
        }
    }
}

/// Ends a modification begun by `Store::begin_modification`, however the modification ends.
#[must_use]
pub(crate) struct ModificationGuard {
    coordination: Arc<Coordinated>,
    writer: Arc<WriterGate>,
}

impl ModificationGuard {
    pub(crate) fn new(coordination: Arc<Coordinated>, writer: Arc<WriterGate>) -> Self {
        ModificationGuard {
            coordination,
            writer,
        }
    }
}

impl Drop for ModificationGuard {
    fn drop(&mut self) {
        // A transaction that ended unresolved leaves the count odd and the fence set, so no
        // reader treats the partial state it left as settled before recovery has finished it. A
        // closing raise that cannot be published does the same, and the handle then refuses
        // writes as it does after an unresolved transaction.
        let end = if self.writer.is_unresolved() && !self.coordination.is_recovering() {
            ModificationEnd::Unresolved
        } else {
            ModificationEnd::Settled
        };
        if self.coordination.end(end).is_err() {
            self.writer.mark_unresolved();
        }
    }
}
