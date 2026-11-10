// Copyright (c) Michael Grier.

//! The in-process half of "a store root has one writer" (`D41`, `D116`).
//!
//! The OS lock excludes a second *process*. Within one process, every `Store` clone and
//! every `WriteMonitor` spawned from one shares this gate, so their writes are serialized
//! too: a check-then-write (a conditional swap, a monitor's read-modify-write) cannot be
//! interleaved with another writer's. The gate is re-entrant per thread, because the
//! public write verbs call one another (a transaction ingests blobs and writes pointers),
//! and it is granted in arrival order, so a busy writer cannot starve another.

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
    /// Set once a transaction may have passed its commit point without completing. Every
    /// handle then refuses writes and leaves the journal alone until the store is reopened.
    unresolved: AtomicBool,
}

impl WriterGate {
    pub(crate) fn mark_unresolved(&self) {
        self.unresolved.store(true, Ordering::Release);
    }

    pub(crate) fn is_unresolved(&self) -> bool {
        self.unresolved.load(Ordering::Acquire)
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
            _not_send: PhantomData,
        }
    }
}

/// Holds the [`WriterGate`] until dropped. Not `Send`: the gate is held by a thread, so the
/// guard must be released on the thread that took it.
#[derive(Debug)]
pub(crate) struct WriterGuard {
    gate: Arc<WriterGate>,
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
            holder.owner = None;
            holder.serving += 1;
            drop(holder);
            // Every waiter re-checks its own ticket, so all must wake for the right one to.
            self.gate.released.notify_all();
        }
    }
}
