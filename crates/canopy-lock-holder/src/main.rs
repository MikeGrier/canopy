// Copyright (c) Michael Grier.

//! A writer attachment and an explicit hold of coordination exclusion until the process is killed.
//!
//! This exists as a real `[[bin]]` rather than a thread or a `#[cfg(test)]` helper because
//! the property under test is what the **operating system** does when a lock's owner dies
//! without running any destructor. Only a separate process that is terminated can witness
//! that, and only a `[[bin]]` gets a `CARGO_BIN_EXE_*` path for an integration test to
//! spawn.
//!
//! Usage: `canopy-lock-holder <root>`. The root must already be an initialised store whose
//! lock is free. On success it prints one line -- `locked` -- and then blocks reading stdin;
//! the parent is expected to kill it. Any failure is reported on stderr with a non-zero exit,
//! so the parent never mistakes "could not acquire" for "acquired and waiting".
//!
//! It also exits when stdin reaches end-of-file. A parent that dies before it can kill the
//! holder closes its end of the pipe as it goes, so the holder cannot outlive the test run
//! and keep a scratch root locked.

use std::fmt;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::Arc;

use acadia::coord::file::FileFactory;
use acadia::coord::{CoordinationFactory, Waiting};
use acadia::{OsFs, Vfs};
use canopy::{Store, StoreOptions};

#[cfg(test)]
mod tests;

/// The handshake the parent waits for before asserting anything.
const READY: &str = "locked";

/// Everything the holder reports: the handshake to `out`, failures to `err`.
struct Reporter<O, E> {
    out: O,
    err: E,
}

impl<O: Write, E: Write> Reporter<O, E> {
    /// Flushed, because the parent blocks on it and this process never drains a buffer.
    fn ready(&mut self) -> std::io::Result<()> {
        writeln!(self.out, "{READY}")?;
        self.out.flush()
    }

    fn fail(&mut self, message: fmt::Arguments<'_>) -> ExitCode {
        let _ = writeln!(self.err, "{message}");
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let mut report = Reporter {
        out: std::io::stdout(),
        err: std::io::stderr(),
    };
    let Some(root) = std::env::args().nth(1) else {
        return report.fail(format_args!("usage: canopy-lock-holder <root>"));
    };

    let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());
    // Attachment precedes exclusion and outlives it, including during orderly teardown.
    let _store = match Store::open(
        Arc::clone(&vfs),
        &root,
        StoreOptions::without_nonce_source(canopy::Agent::new(
            "canopy-tests",
            "test",
            "Canopy test fixture",
            1,
        )),
    ) {
        Ok((store, _)) => store,
        Err(e) => return report.fail(format_args!("open failed: {e}")),
    };

    let participant = match FileFactory::new().open(vfs, std::path::Path::new(&root)) {
        Ok(participant) => participant,
        Err(error) => return report.fail(format_args!("coordination failed: {error}")),
    };
    let _hold = match participant.acquire(Waiting::Wait) {
        Ok(hold) => hold,
        Err(error) => return report.fail(format_args!("exclusion failed: {error}")),
    };

    if report.ready().is_err() {
        return ExitCode::FAILURE;
    }

    // Blocks until the parent closes the pipe (or dies), or kills this process first.
    let mut sink = [0u8; 64];
    let mut stdin = std::io::stdin().lock();
    loop {
        match stdin.read(&mut sink) {
            Ok(0) => return ExitCode::SUCCESS,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return ExitCode::FAILURE,
        }
    }
}
