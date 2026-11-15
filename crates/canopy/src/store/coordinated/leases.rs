// Copyright (c) Michael Grier.

use std::cell::Cell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};

#[cfg(test)]
mod tests;

thread_local! {
    static OPERATION_DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaseError {
    Draining,
    InsideOperation,
    Cancelled,
    Detached,
    Busy,
}

#[derive(Debug, Default)]
struct State {
    operations: HashMap<ThreadId, usize>,
    drain: Option<Arc<DrainToken>>,
    detached: bool,
    converting: bool,
}

#[derive(Debug)]
pub(crate) struct DrainToken;

#[derive(Debug, Default)]
pub(crate) struct Leases {
    state: Mutex<State>,
    ended: Condvar,
}

pub(crate) struct OperationLease {
    leases: Arc<Leases>,
    attachment: Option<Arc<super::Coordinated>>,
    owner: ThreadId,
    _not_send: PhantomData<*const ()>,
}

impl Leases {
    pub(crate) fn control_context() -> Result<(), LeaseError> {
        if OPERATION_DEPTH.with(|depth| depth.get() > 0) {
            Err(LeaseError::InsideOperation)
        } else {
            Ok(())
        }
    }

    pub(crate) fn enter(self: &Arc<Self>) -> Result<OperationLease, LeaseError> {
        let owner = thread::current().id();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.detached && !state.operations.contains_key(&owner) {
            return Err(LeaseError::Detached);
        }
        if state.drain.is_some() && !state.operations.contains_key(&owner) {
            return Err(LeaseError::Draining);
        }
        *state.operations.entry(owner).or_default() += 1;
        OPERATION_DEPTH.with(|depth| depth.set(depth.get() + 1));
        #[cfg(test)]
        self.ended.notify_all();
        Ok(OperationLease {
            leases: Arc::clone(self),
            attachment: None,
            owner,
            _not_send: PhantomData,
        })
    }

    pub(crate) fn begin(&self) -> Result<Arc<DrainToken>, LeaseError> {
        if OPERATION_DEPTH.with(|depth| depth.get() > 0) {
            return Err(LeaseError::InsideOperation);
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.detached {
            return Err(LeaseError::Detached);
        }
        if state.drain.is_some() {
            return Err(LeaseError::Draining);
        }
        let token = Arc::new(DrainToken);
        state.drain = Some(Arc::clone(&token));
        Ok(token)
    }

    pub(crate) fn enter_migration(self: &Arc<Self>) -> Result<OperationLease, LeaseError> {
        Self::control_context()?;
        let owner = thread::current().id();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.detached || !state.converting {
            return Err(LeaseError::Detached);
        }
        *state.operations.entry(owner).or_default() += 1;
        OPERATION_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Ok(OperationLease {
            leases: Arc::clone(self),
            attachment: None,
            owner,
            _not_send: PhantomData,
        })
    }

    pub(crate) fn wait(&self, token: &Arc<DrainToken>) -> Result<(), LeaseError> {
        if OPERATION_DEPTH.with(|depth| depth.get() > 0) {
            return Err(LeaseError::InsideOperation);
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if !state
                .drain
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, token))
            {
                return Err(LeaseError::Cancelled);
            }
            if state.operations.is_empty() {
                return Ok(());
            }
            state = self
                .ended
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub(crate) fn ready(&self, token: &Arc<DrainToken>) -> Result<bool, LeaseError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state
            .drain
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, token))
        {
            return Err(LeaseError::Cancelled);
        }
        Ok(state.operations.is_empty())
    }

    pub(crate) fn cancel(&self, token: &Arc<DrainToken>) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state
            .drain
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, token))
        {
            state.drain = None;
            self.ended.notify_all();
            return true;
        }
        false
    }

    pub(crate) fn draining(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain
            .is_some()
    }

    pub(crate) fn detach(&self, token: &Arc<DrainToken>) -> Result<(), LeaseError> {
        if OPERATION_DEPTH.with(|depth| depth.get() > 0) {
            return Err(LeaseError::InsideOperation);
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state
            .drain
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, token))
        {
            return Err(LeaseError::Cancelled);
        }
        if !state.operations.is_empty() {
            return Err(LeaseError::Busy);
        }
        state.drain = None;
        state.detached = true;
        state.converting = true;
        self.ended.notify_all();
        Ok(())
    }

    pub(crate) fn attached(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.detached = false;
        state.converting = false;
    }

    pub(crate) fn conversion_finished(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .converting = false;
    }

    pub(crate) fn begin_attachment(&self) -> Result<(), LeaseError> {
        if OPERATION_DEPTH.with(|depth| depth.get() > 0) {
            return Err(LeaseError::InsideOperation);
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.detached || state.converting {
            return Err(LeaseError::Busy);
        }
        state.converting = true;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn wait_until_active(&self, operations: usize) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.operations.len() < operations {
            state = self
                .ended
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl OperationLease {
    pub(crate) fn keeping(mut self, attachment: Arc<super::Coordinated>) -> Self {
        self.attachment = Some(attachment);
        self
    }
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        let mut state = self
            .leases
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let depth = state
            .operations
            .get_mut(&self.owner)
            .expect("the operation owns a lease");
        *depth -= 1;
        if *depth == 0 {
            state.operations.remove(&self.owner);
        }
        OPERATION_DEPTH.with(|depth| depth.set(depth.get() - 1));
        self.leases.ended.notify_all();
    }
}
