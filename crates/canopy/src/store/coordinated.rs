// Copyright (c) Michael Grier.

//! A store's part in the coordination state: the participant that samples the modification count
//! and, in a handle that may write, the hold of writer exclusion that raises it (`D335`; canopy's
//! DESIGN-NOTES.md, "The modification count is the coordination state's").

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use acadia::coord::{
    Acquired, Attachment, CoordError, Coordination, ExclusionHold, Generation, ModCount,
    ModificationEnd, Notice, Purpose, RecoveryNeed, Trust, Waiting,
};

use super::StoreError;
use super::writer::WriterGate;

pub(crate) mod leases;

/// The coordination state a store handle shares with its clones.
///
/// Attachment lasts until the final clone drops; writer exclusion lasts only for one outermost operation.
pub(crate) struct Coordinated {
    /// Dropped before the participant, so the ownership record is cleared while the participant
    /// that wrote it is still there to clear it.
    hold: Mutex<Option<Box<dyn ExclusionHold>>>,
    writable: AtomicBool,
    leases: Arc<leases::Leases>,
    last_generation: Mutex<Option<Generation>>,
    // Exclusion ends before attachment; the attachment unregisters before releasing its tiers.
    _attachment: Mutex<Option<Box<dyn Attachment>>>,
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
            hold: Mutex::new(None),
            writable: AtomicBool::new(false),
            leases: Arc::default(),
            last_generation: Mutex::new(None),
            _attachment: Mutex::new(Some(attachment)),
            participant,
            recovering: AtomicBool::new(false),
        }
    }

    /// A writer attachment whose operations may acquire exclusion through its participant.
    pub(crate) fn writer(
        participant: Box<dyn Coordination>,
        attachment: Box<dyn Attachment>,
    ) -> Self {
        Coordinated {
            hold: Mutex::new(None),
            writable: AtomicBool::new(true),
            leases: Arc::default(),
            last_generation: Mutex::new(None),
            _attachment: Mutex::new(Some(attachment)),
            participant,
            recovering: AtomicBool::new(false),
        }
    }

    pub(crate) fn may_write(&self) -> bool {
        self.writable.load(Ordering::Acquire)
    }

    pub(crate) fn attach_again(
        &self,
        vfs: &Arc<dyn acadia::Vfs>,
        root: &std::path::Path,
        agent: &super::Agent,
        access: acadia::coord::Access,
    ) -> Result<(), StoreError> {
        let mut slot = self
            ._attachment
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut attachment = self.participant.attach(access, Waiting::Wait)?;
        let genesis = super::Store::read_admission_genesis(vfs, root)?;
        genesis.metadata.check_admission(agent, access)?;
        let payload = super::agent::registration_payload(agent, attachment.tiers(), false)?;
        let _ = attachment.register(&payload);
        *slot = Some(attachment);
        self.writable
            .store(access == acadia::coord::Access::Write, Ordering::Release);
        Ok(())
    }

    pub(crate) fn end_upgrade(&self, success: bool) -> Result<(), StoreError> {
        Ok(self.end(if success {
            ModificationEnd::Settled
        } else {
            ModificationEnd::Unresolved
        })?)
    }

    pub(crate) fn leases(&self) -> &Arc<leases::Leases> {
        &self.leases
    }

    pub(crate) fn register_draining(&self, agent: &super::Agent) -> Result<(), StoreError> {
        let mut attachment = self
            ._attachment
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(attachment) = attachment.as_mut() else {
            return Ok(());
        };
        let payload =
            super::agent::registration_payload(agent, attachment.tiers(), self.leases.draining())?;
        attachment.register(&payload)?;
        Ok(())
    }

    pub(crate) fn begin_conversion(
        &self,
    ) -> Result<Box<dyn acadia::coord::UpgradeConversion>, StoreError> {
        let attachment = self
            ._attachment
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or(StoreError::Coordination(CoordError::Malformed {
                what: "detached attachment",
            }))?;
        Ok(attachment.begin_conversion()?)
    }

    fn hold(&self) -> MutexGuard<'_, Option<Box<dyn ExclusionHold>>> {
        self.hold.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn acquire(&self) -> Result<(Acquired, bool), StoreError> {
        self.acquire_waiting(Waiting::Wait)
    }

    pub(crate) fn try_acquire(&self) -> Result<(Acquired, bool), StoreError> {
        self.acquire_waiting(Waiting::Never)
    }

    fn acquire_waiting(&self, waiting: Waiting) -> Result<(Acquired, bool), StoreError> {
        let mut slot = self.hold();
        if slot.is_none() {
            *slot = Some(self.participant.acquire(waiting)?);
        }
        let acquired = *slot.as_ref().expect("writer exclusion acquired").acquired();
        let mut last = self
            .last_generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = *last != Some(acquired.found());
        *last = Some(acquired.advanced_to());
        Ok((acquired, changed))
    }

    pub(crate) fn release(&self) {
        self.hold().take();
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
        self.hold().as_ref().map(|hold| *hold.acquired())
    }

    /// Whether the state acquiring writer exclusion found could be vouched for, so that what
    /// required recovery, if anything did, was a fence and not the state itself.
    pub(crate) fn state_was_vouched(&self) -> bool {
        self.hold()
            .as_ref()
            .is_some_and(|hold| hold.acquired().trust() == Trust::Vouched)
    }

    /// Whether recovery is required, and why, observed as one observation that changes nothing.
    pub(crate) fn discover(&self) -> Result<RecoveryNeed, StoreError> {
        Ok(self.participant.discover_recovery()?)
    }

    pub(crate) fn discover_with(
        &self,
        inspect: &mut dyn FnMut(),
    ) -> Result<RecoveryNeed, StoreError> {
        Ok(self.participant.discover_recovery_with(inspect)?)
    }

    /// Completes recovery: publishes the even count and the trusted state, and clears the fence
    /// last, so a failure at any point leaves the fence set. Idempotent.
    pub(crate) fn complete_recovery(&self) -> Result<(), StoreError> {
        match self.hold().as_mut() {
            Some(hold) => Ok(hold.complete_recovery()?),
            None => Ok(()),
        }
    }

    /// Begins a modification through the hold. The outermost one raises the count to odd before
    /// the caller's first effect.
    pub(crate) fn begin(&self, root: &std::path::Path) -> Result<(), StoreError> {
        let mut slot = self.hold();
        let Some(hold) = slot.as_mut() else {
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
        match self.hold().as_mut() {
            Some(hold) => hold.end_modification(end),
            None => Ok(()),
        }
    }

    pub(crate) fn fence_modification(&self) -> Result<(), StoreError> {
        Ok(self.end(ModificationEnd::Unresolved)?)
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
