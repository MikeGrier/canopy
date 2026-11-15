// Copyright (c) Michael Grier.

use super::{clock, metrics::Sample};
use acadia::Vfs;
use acadia::coord::*;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Default)]
pub struct Metrics {
    pub requests: Mutex<std::collections::HashMap<std::thread::ThreadId, u64>>,
    pub samples: Mutex<Vec<Sample>>,
    pub worker: usize,
}

impl Metrics {
    pub fn request(&self, requested: u64) {
        self.requests
            .lock()
            .unwrap()
            .insert(std::thread::current().id(), requested);
    }
    pub fn clear_request(&self) {
        self.requests
            .lock()
            .unwrap()
            .remove(&std::thread::current().id());
    }
}

#[derive(Debug)]
pub struct Factory {
    pub inner: Arc<dyn CoordinationFactory>,
    pub metrics: Arc<Metrics>,
}

impl CoordinationFactory for Factory {
    fn implementation(&self) -> &ImplementationId {
        self.inner.implementation()
    }
    fn open(&self, vfs: Arc<dyn Vfs>, root: &Path) -> CoordResult<Box<dyn Coordination>> {
        Ok(Box::new(Participant {
            inner: self.inner.open(vfs, root)?,
            metrics: Arc::clone(&self.metrics),
        }))
    }
}

#[derive(Debug)]
struct Participant {
    inner: Box<dyn Coordination>,
    metrics: Arc<Metrics>,
}

impl Coordination for Participant {
    fn implementation(&self) -> &ImplementationId {
        self.inner.implementation()
    }
    fn abandon(&self) {
        self.inner.abandon();
    }
}
impl Creation for Participant {
    fn create(
        &self,
        plan: &CreationPlan<'_>,
        maker: &mut dyn ArtifactMaker,
    ) -> CoordResult<Created> {
        self.inner.create(plan, maker)
    }
    fn reserve(&self, inputs: &[u8], nonce: CreationNonce) -> CoordResult<Created> {
        self.inner.reserve(inputs, nonce)
    }
    fn confirm_created(&self, marker: &OsStr) -> CoordResult<()> {
        self.inner.confirm_created(marker)
    }
}
impl RestartRecognition for Participant {
    fn trust(&self) -> CoordResult<Trust> {
        self.inner.trust()
    }
}
impl RecoveryDiscovery for Participant {
    fn discover_recovery_with(&self, inspect: &mut dyn FnMut()) -> CoordResult<RecoveryNeed> {
        self.inner.discover_recovery_with(inspect)
    }
}
impl ModificationCount for Participant {
    fn sample(&self) -> CoordResult<ModCount> {
        self.inner.sample()
    }
}
impl CountNotification for Participant {
    fn notify(
        &self,
        sampled: ModCount,
        deadline: Instant,
        ring: Box<dyn FnOnce() + Send>,
    ) -> CoordResult<Box<dyn Notice>> {
        self.inner.notify(sampled, deadline, ring)
    }
}
impl Admission for Participant {
    fn attach(&self, access: Access, waiting: Waiting) -> CoordResult<Box<dyn Attachment>> {
        self.inner.attach(access, waiting)
    }
}
impl Registry for Participant {
    fn list(&self) -> CoordResult<Listing> {
        self.inner.list()
    }
}
impl WriterExclusion for Participant {
    fn acquire(&self, waiting: Waiting) -> CoordResult<Box<dyn ExclusionHold>> {
        let begun = Instant::now();
        let inner = self.inner.acquire(waiting)?;
        let acquired = clock::now().map_err(|error| {
            CoordError::Vfs(acadia::VfsError::new(
                acadia::VfsErrorKind::Other,
                "clock",
                error.to_string(),
            ))
        })?;
        let request = self
            .metrics
            .requests
            .lock()
            .unwrap()
            .get(&std::thread::current().id())
            .copied()
            .unwrap_or(0);
        Ok(Box::new(Hold {
            generation: inner.acquired().advanced_to().get(),
            inner: Some(inner),
            metrics: Arc::clone(&self.metrics),
            acquired,
            request,
            begun: Instant::now(),
            lock_wait: begun.elapsed().as_secs_f64(),
            opening: 0.0,
            closing: 0.0,
        }))
    }
}

#[derive(Debug)]
struct Hold {
    inner: Option<Box<dyn ExclusionHold>>,
    metrics: Arc<Metrics>,
    generation: u64,
    request: u64,
    acquired: u64,
    begun: Instant,
    lock_wait: f64,
    opening: f64,
    closing: f64,
}

impl Hold {
    fn finish(&mut self) -> CoordResult<()> {
        let Some(inner) = self.inner.take() else {
            return Ok(());
        };
        let result = inner.release();
        if self.request != 0 {
            self.metrics.samples.lock().unwrap().push(Sample {
                worker: self.metrics.worker,
                request: self.request,
                acquired: self.acquired,
                generation: self.generation,
                wait_seconds: self.acquired.saturating_sub(self.request) as f64 / 1e9,
                hold_seconds: self.begun.elapsed().as_secs_f64(),
                lock_wait_seconds: self.lock_wait,
                released: result.is_ok(),
                opening_seconds: self.opening,
                closing_seconds: self.closing,
            });
        }
        result
    }
}
impl ExclusionHold for Hold {
    fn acquired(&self) -> &Acquired {
        self.inner.as_ref().unwrap().acquired()
    }
    fn begin_modification(&mut self, purpose: Purpose) -> CoordResult<()> {
        let begun = Instant::now();
        let result = self.inner.as_mut().unwrap().begin_modification(purpose);
        self.opening += begun.elapsed().as_secs_f64();
        result
    }
    fn end_modification(&mut self, end: ModificationEnd) -> CoordResult<()> {
        let begun = Instant::now();
        let result = self.inner.as_mut().unwrap().end_modification(end);
        self.closing += begun.elapsed().as_secs_f64();
        result
    }
    fn complete_recovery(&mut self) -> CoordResult<()> {
        self.inner.as_mut().unwrap().complete_recovery()
    }
    fn release(mut self: Box<Self>) -> CoordResult<()> {
        self.finish()
    }
}
impl Drop for Hold {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
