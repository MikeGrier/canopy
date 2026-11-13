// Copyright (c) Michael Grier.

//! Test fixture: a process that takes one lock over `OsFs` and holds it until it is killed.
//!
//! Usage: `vfs-lock-holder <path> <shared|exclusive> <file|OFFSET+LEN> [wait]`. With `wait` it
//! takes the lock with `Vfs::lock`, waiting for it, and otherwise with `Vfs::try_lock`. On
//! success it prints one line -- `locked` -- and then blocks reading stdin; the parent is
//! expected to kill it, and it also exits when stdin reaches end-of-file, so it cannot outlive
//! a parent that died. Any failure is reported on stderr with a non-zero exit, so the parent
//! never mistakes "could not acquire" for "acquired and waiting".

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use acadia::{LockMode, LockSpan, OsFs, Vfs};

/// The handshake the parent waits for before asserting anything.
const READY: &str = "locked";

fn parse_mode(text: &str) -> Option<LockMode> {
    match text {
        "shared" => Some(LockMode::Shared),
        "exclusive" => Some(LockMode::Exclusive),
        _ => None,
    }
}

fn parse_span(text: &str) -> Option<LockSpan> {
    if text == "file" {
        return Some(LockSpan::File);
    }
    let (offset, len) = text.split_once('+')?;
    Some(LockSpan::Range {
        offset: offset.parse().ok()?,
        len: len.parse().ok()?,
    })
}

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

    fn fail(&mut self, message: std::fmt::Arguments<'_>) -> ExitCode {
        let _ = writeln!(self.err, "{message}");
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let mut report = Reporter {
        out: std::io::stdout(),
        err: std::io::stderr(),
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, mode, span, wait) = match args.as_slice() {
        [path, mode, span] => (path, mode, span, false),
        [path, mode, span, wait] if wait == "wait" => (path, mode, span, true),
        _ => {
            return report.fail(format_args!(
                "usage: vfs-lock-holder <path> <shared|exclusive> <file|OFFSET+LEN> [wait]"
            ));
        }
    };
    let (Some(mode), Some(span)) = (parse_mode(mode), parse_span(span)) else {
        return report.fail(format_args!("unrecognised mode {mode:?} or span {span:?}"));
    };

    // Bound and never dropped: holding the guard is the entire job, and the parent kills this
    // process rather than letting it unwind.
    let fs = OsFs::new();
    let taken = if wait {
        fs.lock(Path::new(path), mode, span)
    } else {
        fs.try_lock(Path::new(path), mode, span)
    };
    let _held = match taken {
        Ok(held) => held,
        Err(e) => return report.fail(format_args!("taking the lock failed: {e}")),
    };

    if report.ready().is_err() {
        return ExitCode::FAILURE;
    }

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
