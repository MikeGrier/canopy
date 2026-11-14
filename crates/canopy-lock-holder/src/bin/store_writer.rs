// Copyright (c) Michael Grier.

//! Test fixture for a store written by another process: opens a store root as its writer, writes
//! one document, and then holds the store until its stdin closes.
//!
//! Usage: `canopy-store-writer <root> <node> <document> <body> [pause]`. It reports each step
//! as one line on stdout -- `opened` once it holds the root, `paused` and then `wrote` -- and
//! any failure on stderr with a non-zero exit, so the parent never mistakes a failure for a
//! step. With `pause` it stops at the first rename into the namespace, so with the store's
//! replicas only some have moved and the write is part-way through, and waits for a line on
//! stdin before it goes on. An end of file at that point exits it, so a parent that dies cannot
//! leave it holding the root.
//!
//! It is a `[[bin]]` for the reason `canopy-lock-holder` is: a writer that is killed runs no
//! destructor, and only a separate process that is terminated shows what a reader finds then.

use std::io::{BufRead, Read, Write};
use std::process::ExitCode;
use std::sync::Arc;

use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, OsFs, PathMatcher, Vfs, When};
use canopy::{DocumentBody, DocumentName, NodePath, Nonce, Store, StoreOptions};

/// The directory of the root that holds the namespace's replicas.
const NAMESPACE_DIR: &str = "namespace";

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn fail(message: &str) -> ExitCode {
    let _ = writeln!(std::io::stderr(), "{message}");
    ExitCode::FAILURE
}

/// Blocks until the parent sends a line, and exits the process if the parent has gone away.
fn wait_for_go() {
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => std::process::exit(2),
        Ok(_) => {}
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, node, document, body, rest @ ..] = args.as_slice() else {
        return fail("usage: canopy-store-writer <root> <node> <document> <body> [pause]");
    };
    let pause = rest.first().is_some_and(|word| word == "pause");

    let gate = FaultGate::closed();
    let mut policy = FaultPolicy::new();
    if pause {
        policy = policy.inject_gated(
            FaultOp::Rename,
            PathMatcher::predicate(|path| {
                path.components()
                    .any(|component| component.as_os_str() == NAMESPACE_DIR)
            }),
            Fault::observe_with(|_, _| {
                say("paused");
                wait_for_go();
            }),
            When::Once,
            gate.clone(),
        );
    }
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(OsFs::new(), policy));

    let mut minted = 0u64;
    let options = StoreOptions::with_nonce_source(
        canopy::Agent::new("canopy-tests", "test", "Canopy test fixture", 1),
        move || {
            minted += 1;
            Nonce::new(&format!("writer-{}-{minted}", std::process::id())).expect("a valid nonce")
        },
    );
    let store = match Store::open(vfs, root, options) {
        Ok((store, found)) => {
            if (found.required() || found.unresolved_work() == Some(true))
                && let Err(error) = store.recover()
            {
                return fail(&format!("recovery failed: {error}"));
            }
            store
        }
        Err(error) => return fail(&format!("open failed: {error}")),
    };
    say("opened");

    let (Ok(node), Ok(document)) = (NodePath::parse(node), DocumentName::new(document)) else {
        return fail("the node or the document is not a valid name");
    };
    gate.open();
    if let Err(error) = store.write_document_replicated(
        node,
        &document,
        &DocumentBody::new(body.as_bytes().to_vec()),
    ) {
        return fail(&format!("write failed: {error}"));
    }
    gate.close();
    say("wrote");

    // Holds the store until the parent closes the pipe, or kills this process first.
    let mut sink = [0u8; 64];
    let mut stdin = std::io::stdin().lock();
    loop {
        match stdin.read(&mut sink) {
            Ok(0) => return ExitCode::SUCCESS,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return ExitCode::FAILURE,
        }
    }
}
