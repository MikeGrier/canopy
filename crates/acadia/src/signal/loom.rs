// Copyright (c) Michael Grier.

//! `LoomSignal`: a signal made of loom's mutex and condition variable, so that loom, a checker of
//! the Rust memory model, can run a protocol written over [`Signal`] with the signal inside its
//! model. Built only with `--cfg canopy_loom`.

use std::fmt;
use std::io;
use std::time::Duration;

use ::loom::sync::{Condvar, Mutex};

use super::{Signal, Watch, sealed};

/// A signal loom can model.
///
/// A wait with a bound other than zero waits until the signal is raised, however long the bound:
/// loom has no clock, and a wait that never returns is what lets it report a lost wake, as a
/// deadlock. A wait with a zero bound reports the level without waiting.
pub struct LoomSignal {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl fmt::Debug for LoomSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoomSignal").finish_non_exhaustive()
    }
}

impl sealed::Sealed for LoomSignal {}

impl Signal for LoomSignal {
    fn new() -> io::Result<Self> {
        Ok(Self {
            raised: Mutex::new(false),
            changed: Condvar::new(),
        })
    }

    fn raise(&self) -> io::Result<()> {
        *self.raised.lock().expect("a loom model never poisons") = true;
        self.changed.notify_all();
        Ok(())
    }

    fn lower(&self) -> io::Result<()> {
        *self.raised.lock().expect("a loom model never poisons") = false;
        Ok(())
    }

    fn wait(&self, timeout: Duration) -> io::Result<bool> {
        let mut raised = self.raised.lock().expect("a loom model never poisons");
        if timeout.is_zero() {
            return Ok(*raised);
        }
        while !*raised {
            raised = self
                .changed
                .wait(raised)
                .expect("a loom model never poisons");
        }
        Ok(true)
    }

    fn watch<F>(&self, _callback: F) -> io::Result<Watch>
    where
        F: Fn() + Send + Sync + 'static,
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a loom signal has no thread pool to watch it from",
        ))
    }
}
