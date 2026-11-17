// Copyright (c) Michael Grier.

//! Clone-local serialization and thread nesting around a root's coordination exclusion.
//! Only the outermost entry acquires exclusion. The gate serializes one attachment's threads
//! in arrival order, and its last guard releases exclusion before admitting its next waiter.

use super::coordinated::Coordinated;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};

/// Who holds the gate, how deeply, and whose turn is next.
#[derive(Debug, Default)]
struct Holder {
    owner: Option<ThreadId>,
    depth: usize,
    /// The next ticket to hand to an arriving thread.
    next_ticket: u64,
    /// The ticket allowed to take the gate once it is free.
    serving: u64,
}

/// A re-entrant, per-thread, first-come-first-served gate shared by every handle to one
/// store.
#[derive(Debug, Default)]
pub(crate) struct WriterGate {
    holder: Mutex<Holder>,
    released: Condvar,
    /// Set once a transaction may have passed its commit point without completing, or opening
    /// found the coordination state requiring recovery. Every handle then refuses writes and
    /// leaves the journal alone until recovery completes.
    unresolved: AtomicBool,
}

impl WriterGate {
    pub(crate) fn mark_unresolved(&self) {
        self.unresolved.store(true, Ordering::Release);
    }

    pub(crate) fn is_unresolved(&self) -> bool {
        self.unresolved.load(Ordering::Acquire)
    }

    /// Lifts the refusal of writes, once recovery has finished what made it.
    pub(crate) fn clear_unresolved(&self) {
        self.unresolved.store(false, Ordering::Release);
    }

    /// How many nested entries the calling thread holds; zero when another thread, or none,
    /// holds the gate.
    pub(crate) fn held_depth(&self) -> usize {
        let holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        if holder.owner == Some(thread::current().id()) {
            holder.depth
        } else {
            0
        }
    }

    /// Takes the gate only when it is immediately available, without joining the wait list.
    pub(crate) fn try_enter(self: &Arc<Self>) -> Option<WriterGuard> {
        let me = thread::current().id();
        let mut holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        if holder.owner == Some(me) {
            holder.depth += 1;
        } else if holder.owner.is_none() && holder.serving == holder.next_ticket {
            holder.next_ticket += 1;
            holder.owner = Some(me);
            holder.depth = 1;
        } else {
            return None;
        }
        Some(WriterGuard {
            gate: Arc::clone(self),
            publication_settled: false,
            coordination: None,
            store: None,
            other_holder: false,
            lease: None,
            _not_send: PhantomData,
        })
    }

    /// Blocks until the calling thread holds the gate; nested entries do not block.
    pub(crate) fn enter(self: &Arc<Self>) -> WriterGuard {
        let me = thread::current().id();
        let mut holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        if holder.owner == Some(me) {
            holder.depth += 1;
        } else {
            let ticket = holder.next_ticket;
            holder.next_ticket += 1;
            while holder.owner.is_some() || holder.serving != ticket {
                holder = self
                    .released
                    .wait(holder)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            holder.owner = Some(me);
            holder.depth = 1;
        }
        WriterGuard {
            gate: Arc::clone(self),
            publication_settled: false,
            coordination: None,
            store: None,
            other_holder: false,
            lease: None,
            _not_send: PhantomData,
        }
    }
}

/// Holds the [`WriterGate`] until dropped. Not `Send`: the gate is held by a thread, so the
/// guard must be released on the thread that took it.
pub(crate) struct WriterGuard {
    gate: Arc<WriterGate>,
    pub(super) publication_settled: bool,
    pub(super) coordination: Option<Arc<Coordinated>>,
    pub(super) store: Option<super::Store>,
    pub(super) other_holder: bool,
    pub(super) lease: Option<super::coordinated::leases::OperationLease>,
    _not_send: PhantomData<*const ()>,
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        let mut holder = self
            .gate
            .holder
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        holder.depth -= 1;
        if holder.depth == 0 {
            drop(holder);
            if let Some(store) = &self.store {
                store.release_current_claims();
            }
            if let Some(coordination) = &self.coordination {
                coordination.release();
            }
            let mut holder = self
                .gate
                .holder
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            holder.owner = None;
            holder.serving += 1;
            drop(holder);
            // Every waiter re-checks its own ticket, so all must wake for the right one to.
            self.gate.released.notify_all();
        }
    }
}
