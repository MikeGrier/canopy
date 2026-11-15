// Copyright (c) Michael Grier.

//! Shared-memory coordination state over local filesystems and the in-memory filesystem model.

mod creation;
mod state;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use super::{
    Access, Admission, Attachment, CoordError, CoordResult, Coordination, CoordinationFactory,
    CountNotification, ExclusionHold, ImplementationId, Listing, ModCount, ModificationCount,
    Notice, RecoveryDiscovery, RecoveryNeed, Registry, RestartRecognition, Trust, Waiting,
    WriterExclusion,
};
use crate::{LockMode, LockSpan, Vfs, VfsLock};

#[cfg(test)]
mod tests;

/// The shared implementation's stable identity. Changing it changes root admission.
pub static IMPLEMENTATION: ImplementationId = ImplementationId::from_static("acadia-shared-v1");

/// Shared-memory coordination, separate from the file-based implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct SharedFactory;

impl SharedFactory {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

struct ParticipantInner {
    vfs: Arc<dyn Vfs>,
    root: PathBuf,
    state: OnceLock<Arc<SharedState>>,
    initializing: Mutex<()>,
    abandoned: AtomicBool,
    creation: Mutex<HashMap<u64, Box<dyn VfsLock>>>,
    next_creation: AtomicU64,
}

struct SharedParticipant {
    inner: Arc<ParticipantInner>,
}

impl std::fmt::Debug for SharedParticipant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedCoordination")
            .field("root", &self.inner.root)
            .finish_non_exhaustive()
    }
}

impl SharedParticipant {
    fn alive(&self) -> CoordResult<()> {
        if self.inner.abandoned.load(Ordering::Acquire) {
            Err(CoordError::Abandoned)
        } else {
            Ok(())
        }
    }

    fn state(&self) -> CoordResult<&Arc<SharedState>> {
        self.alive()?;
        if let Some(state) = self.inner.state.get() {
            return Ok(state);
        }
        let _initializing = self
            .inner
            .initializing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.alive()?;
        if self.inner.state.get().is_none() {
            self.check_implementation()?;
            let state = SharedState::open(Arc::clone(&self.inner.vfs), &self.inner.root)?;
            if self.inner.abandoned.load(Ordering::Acquire) {
                state.abandon();
                return Err(CoordError::Abandoned);
            }
            let _ = self.inner.state.set(Arc::new(state));
        }
        let state = self
            .inner
            .state
            .get()
            .expect("initialization published state");
        if self.inner.abandoned.load(Ordering::Acquire) {
            state.abandon();
            return Err(CoordError::Abandoned);
        }
        Ok(state)
    }
}

impl CoordinationFactory for SharedFactory {
    fn implementation(&self) -> &ImplementationId {
        &IMPLEMENTATION
    }
    fn open(&self, vfs: Arc<dyn Vfs>, root: &Path) -> CoordResult<Box<dyn Coordination>> {
        Ok(Box::new(SharedParticipant {
            inner: Arc::new(ParticipantInner {
                vfs,
                root: root.to_path_buf(),
                state: OnceLock::new(),
                initializing: Mutex::new(()),
                abandoned: AtomicBool::new(false),
                creation: Mutex::new(HashMap::new()),
                next_creation: AtomicU64::new(1),
            }),
        }))
    }
}

impl Coordination for SharedParticipant {
    fn implementation(&self) -> &ImplementationId {
        &IMPLEMENTATION
    }
    fn abandon(&self) {
        self.inner.abandoned.store(true, Ordering::Release);
        self.inner
            .creation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        if let Some(state) = self.inner.state.get() {
            state.abandon();
        }
    }
}

impl WriterExclusion for SharedParticipant {
    fn acquire(&self, waiting: Waiting) -> CoordResult<Box<dyn ExclusionHold>> {
        self.state()?.acquire(waiting)
    }
}
impl Admission for SharedParticipant {
    fn attach(&self, access: Access, waiting: Waiting) -> CoordResult<Box<dyn Attachment>> {
        self.state()?.attach(access, waiting)
    }
}
impl Registry for SharedParticipant {
    fn list(&self) -> CoordResult<Listing> {
        self.state()?.list()
    }
}
impl RestartRecognition for SharedParticipant {
    fn trust(&self) -> CoordResult<Trust> {
        self.state()?.trust()
    }
}
impl RecoveryDiscovery for SharedParticipant {
    fn discover_recovery_with(&self, inspect: &mut dyn FnMut()) -> CoordResult<RecoveryNeed> {
        self.state()?.discover_recovery_with(inspect)
    }
}
impl ModificationCount for SharedParticipant {
    fn sample(&self) -> CoordResult<ModCount> {
        self.state()?.sample()
    }
}
impl CountNotification for SharedParticipant {
    fn notify(
        &self,
        sampled: ModCount,
        deadline: Instant,
        ring: Box<dyn FnOnce() + Send>,
    ) -> CoordResult<Box<dyn Notice>> {
        self.state()?.notify(sampled, deadline, ring)
    }
}

pub use state::{SharedState, StateSnapshot};
