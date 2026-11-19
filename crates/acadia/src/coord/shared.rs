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

#[cfg(any(test, feature = "test-support"))]
pub use state::{SharedStep, SharedWords, StepHook, inspect};

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

    /// A factory whose participants tell `hook` of each step that changes the shared words,
    /// before it acts.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_hook(self, hook: Arc<dyn StepHook>) -> HookedSharedFactory {
        HookedSharedFactory { hook }
    }
}

/// Makes participants of the shared implementation that announce their steps to a hook.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone)]
pub struct HookedSharedFactory {
    hook: Arc<dyn StepHook>,
}

#[cfg(any(test, feature = "test-support"))]
impl std::fmt::Debug for HookedSharedFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HookedSharedFactory")
            .finish_non_exhaustive()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl CoordinationFactory for HookedSharedFactory {
    fn implementation(&self) -> &ImplementationId {
        &IMPLEMENTATION
    }
    fn open(&self, vfs: Arc<dyn Vfs>, root: &Path) -> CoordResult<Box<dyn Coordination>> {
        Ok(Box::new(SharedParticipant::new(
            vfs,
            root,
            Some(Arc::clone(&self.hook)),
        )))
    }
}

struct ParticipantInner {
    vfs: Arc<dyn Vfs>,
    root: PathBuf,
    state: OnceLock<Arc<SharedState>>,
    initializing: Mutex<()>,
    /// Held by an opening from the end of its setup wait; see `state::OpenGate`.
    opening: Mutex<()>,
    abandoned: Arc<AtomicBool>,
    creation: Arc<Mutex<HashMap<u64, Box<dyn VfsLock>>>>,
    next_creation: AtomicU64,
    #[cfg(any(test, feature = "test-support"))]
    hook: Option<Arc<dyn StepHook>>,
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
    fn new(
        vfs: Arc<dyn Vfs>,
        root: &Path,
        #[cfg(any(test, feature = "test-support"))] hook: Option<Arc<dyn StepHook>>,
    ) -> Self {
        SharedParticipant {
            inner: Arc::new(ParticipantInner {
                vfs,
                root: root.to_path_buf(),
                state: OnceLock::new(),
                initializing: Mutex::new(()),
                opening: Mutex::new(()),
                abandoned: Arc::new(AtomicBool::new(false)),
                creation: Arc::new(Mutex::new(HashMap::new())),
                next_creation: AtomicU64::new(1),
                #[cfg(any(test, feature = "test-support"))]
                hook,
            }),
        }
    }

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
        let _initializing = self.inner.lock(&self.inner.initializing)?;
        self.alive()?;
        if self.inner.state.get().is_none() {
            self.check_implementation()?;
            let gate = state::OpenGate {
                lock: &self.inner.opening,
                abandoned: Arc::clone(&self.inner.abandoned),
            };
            SharedState::open_gated(
                Arc::clone(&self.inner.vfs),
                &self.inner.root,
                Some(&gate),
                |state| {
                    let state = state.with_creation(Arc::downgrade(&self.inner.creation));
                    #[cfg(any(test, feature = "test-support"))]
                    let state = state.with_hook(self.inner.hook.clone());
                    let _ = self.inner.state.set(Arc::new(state));
                },
            )?;
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
        Ok(Box::new(SharedParticipant::new(
            vfs,
            root,
            #[cfg(any(test, feature = "test-support"))]
            None,
        )))
    }
}

impl Coordination for SharedParticipant {
    fn implementation(&self) -> &ImplementationId {
        &IMPLEMENTATION
    }
    fn abandon(&self) {
        self.inner.abandon();
    }
}

impl ParticipantInner {
    fn lock<'a, T>(&'a self, mutex: &'a Mutex<T>) -> CoordResult<std::sync::MutexGuard<'a, T>> {
        match mutex.lock() {
            Ok(guard) => Ok(guard),
            Err(poisoned) => {
                drop(poisoned);
                self.abandon();
                Err(CoordError::Abandoned)
            }
        }
    }

    fn abandon(&self) {
        self.abandoned.store(true, Ordering::Release);
        drop(
            self.opening
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let creation = {
            let mut creation = self
                .creation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *creation)
        };
        crate::coord::discard(creation.into_values());
        if let Some(state) = self.state.get() {
            state.abandon();
        }
    }
}

// The reservation is read on every call: another implementation may reserve the root later.
impl WriterExclusion for SharedParticipant {
    fn acquire(&self, waiting: Waiting) -> CoordResult<Box<dyn ExclusionHold>> {
        let state = self.state()?;
        self.check_implementation()?;
        state.acquire(waiting)
    }
}
impl Admission for SharedParticipant {
    fn attach(&self, access: Access, waiting: Waiting) -> CoordResult<Box<dyn Attachment>> {
        let state = self.state()?;
        self.check_implementation()?;
        state.attach(access, waiting)
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
