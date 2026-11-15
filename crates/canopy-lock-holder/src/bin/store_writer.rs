// Copyright (c) Michael Grier.

//! Cross-process writes, failures and caller-driven recovery with explicit parent handshakes.
//!
//! Usage: `canopy-store-writer <root> <node> <document> <body> [mode] [version] [schema arguments]`.
//! Modes and the session protocol are in [README.md](../../README.md). Checkpoints flush a
//! line to the parent before waiting on stdin; EOF at a paused checkpoint exits the process,
//! so a parent that dies cannot leave it holding exclusion.
//!
//! It is a `[[bin]]` for the reason `canopy-lock-holder` is: a writer that is killed runs no
//! destructor, and only a separate process that is terminated shows what a reader finds then.

use std::io::{BufRead, Read, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, OsFs, PathMatcher, Vfs, When};
use canopy::{
    DocumentBody, DocumentName, NodePath, Nonce, Recovery, Store, StoreError, StoreOptions, Swap,
    SwapSet, WriteOutcome,
};

/// The directory of the root that holds the namespace's replicas.
const NAMESPACE_DIR: &str = "namespace";

#[path = "store_writer/upgrade.rs"]
mod upgrade;

struct Reporter {
    out: Mutex<Box<dyn Write + Send>>,
    err: Mutex<Box<dyn Write + Send>>,
}

fn reporter() -> &'static Reporter {
    static REPORTER: OnceLock<Reporter> = OnceLock::new();
    REPORTER.get_or_init(|| Reporter {
        out: Mutex::new(Box::new(std::io::stdout())),
        err: Mutex::new(Box::new(std::io::stderr())),
    })
}

impl Reporter {
    fn line(&self, message: &str, error: bool) {
        let sink = if error { &self.err } else { &self.out };
        let mut sink = sink.lock().unwrap();
        let _ = writeln!(sink, "{message}");
        let _ = sink.flush();
    }
}

fn say(line: &str) {
    reporter().line(line, false);
}

fn fail(message: &str) -> ExitCode {
    reporter().line(message, true);
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

fn checked<T>(outcome: Result<WriteOutcome<T>, StoreError>) -> Result<T, String> {
    let outcome = outcome.map_err(|error| error.to_string())?;
    outcome
        .agent_stamp
        .map_err(|error| format!("agent stamp failed: {error}"))?;
    Ok(outcome.value)
}

fn write(
    store: &Store,
    node: &NodePath,
    document: &DocumentName,
    bytes: &[u8],
    logged: bool,
) -> Result<(), String> {
    let content = DocumentBody::new(bytes.to_vec());
    if logged {
        let swaps = SwapSet::new([
            Swap::new(node.clone(), document.clone(), content.clone()),
            Swap::new(
                node.clone(),
                DocumentName::new(&format!("{}-peer", document.as_str())).unwrap(),
                content,
            ),
        ])
        .map_err(|error| error.to_string())?;
        let commit = checked(store.apply_transaction(&swaps))?;
        if !commit.is_committed() {
            return Err("an unconditional transaction was rejected".into());
        }
    } else {
        checked(store.write_document_replicated(node.clone(), document, &content))?;
    }
    Ok(())
}

fn session(store: &Store, node: &NodePath, document: &str, text: &str) -> ExitCode {
    let mut sequence = 0u64;
    let mut recovery: Option<Recovery> = None;
    loop {
        let mut command = String::new();
        match std::io::stdin().lock().read_line(&mut command) {
            Ok(0) => return ExitCode::SUCCESS,
            Ok(_) => {}
            Err(error) => return fail(&format!("input failed: {error}")),
        }
        let result = match command.trim() {
            "write" | "transaction" => {
                let name = DocumentName::new(&format!("{document}-{sequence}")).unwrap();
                let result = write(
                    store,
                    node,
                    &name,
                    text.as_bytes(),
                    command.trim() == "transaction",
                );
                if result.is_ok() {
                    say(&format!("wrote:{sequence}"));
                    sequence += 1;
                }
                result
            }
            "recover" => checked(store.recover()).map(|_| say("recovered")),
            "resolve" => checked(store.resolve()).map(|_| say("resolved")),
            "quiesce" => {
                say("quiescing");
                checked(store.quiesce()).map(|_| say("quiesced"))
            }
            "scrub" => checked(store.scrub_content()).map(|_| say("scrubbed")),
            "step" => {
                if recovery.is_none() {
                    match store.recovery() {
                        Ok(run) => recovery = Some(run),
                        Err(error) => return fail(&format!("recovery failed: {error}")),
                    }
                }
                let run = recovery.as_mut().unwrap();
                let phase = run.progress().phase();
                checked(run.step())
                    .map(|progress| say(&format!("phase:{phase:?}:{}", progress.finished())))
            }
            "status" => store
                .properties()
                .map(|properties| say(&format!("version:{}", properties.agent_version)))
                .map_err(|error| error.to_string()),
            _ => Err("unknown command".into()),
        };
        if let Err(error) = result {
            match store.recovery_required() {
                Ok(true) => say("recovery-required"),
                Ok(false) => return fail(&error),
                Err(status) => return fail(&format!("recovery status failed: {status}")),
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, node, document, body, rest @ ..] = args.as_slice() else {
        return fail("usage: canopy-store-writer <root> <node> <document> <body> [mode] [version]");
    };
    let mode = rest.first().map(String::as_str).unwrap_or("write");
    let version = rest.get(1).map(String::as_str).unwrap_or("test");
    let schema_modes = matches!(
        mode,
        "upgrade-safe"
            | "upgrade-unsafe"
            | "upgrade-after-schema"
            | "resume-upgrade"
            | "reader"
            | "reader-wait"
            | "source-probe"
    );
    if rest.len() > if schema_modes { 6 } else { 2 }
        || !matches!(
            mode,
            "write"
                | "pause"
                | "pause-publication"
                | "pause-before"
                | "pause-after"
                | "transaction"
                | "pause-transaction"
                | "fail-transaction"
                | "scrub-paused"
                | "recover-step"
                | "session"
                | "foreign"
                | "upgrade-safe"
                | "upgrade-unsafe"
                | "upgrade-after-schema"
                | "resume-upgrade"
                | "reader"
                | "reader-wait"
                | "source-probe"
        )
    {
        return fail("unknown mode or extra arguments");
    }
    let plan = match upgrade::Plan::parse(rest, mode) {
        Ok(plan) => plan,
        Err(error) => return fail(&error),
    };
    let pause = matches!(mode, "pause" | "pause-publication" | "pause-transaction");
    let pause_before = mode == "pause-before";

    let gate = FaultGate::closed();
    let mut policy = FaultPolicy::new();
    if mode == "reader-wait" {
        policy = policy.inject_matching(
            FaultOp::Lock,
            PathMatcher::file_name("gate.lock"),
            Fault::observe_with(|_, _| say("admission-gate")),
            When::Once,
        );
    }
    if pause_before {
        policy = policy.inject_gated(
            FaultOp::Metadata,
            PathMatcher::file_name(".publication"),
            Fault::observe_with(|_, _| {
                say("paused");
                wait_for_go();
            }),
            When::Once,
            gate.clone(),
        );
    }
    if pause {
        let renames = AtomicUsize::new(0);
        let checkpoint = usize::from(mode != "pause");
        policy = policy.inject_gated(
            FaultOp::Rename,
            PathMatcher::predicate(|path| {
                path.components()
                    .any(|component| component.as_os_str() == NAMESPACE_DIR)
            }),
            Fault::observe_with(move |_, _| {
                if renames.fetch_add(1, Ordering::Relaxed) == checkpoint {
                    say("paused");
                    wait_for_go();
                }
            }),
            When::Always,
            gate.clone(),
        );
    }
    if mode == "fail-transaction" {
        policy = policy.inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(NAMESPACE_DIR),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        );
    }
    if mode == "pause-after" {
        policy = policy.inject_gated(
            FaultOp::RemoveDir,
            PathMatcher::has_component(".claims"),
            Fault::observe_with(|_, _| {
                say("paused");
                wait_for_go();
            }),
            When::Once,
            gate.clone(),
        );
    }
    if mode == "scrub-paused" {
        let entries = AtomicUsize::new(0);
        policy = policy.inject_gated(
            FaultOp::Lock,
            PathMatcher::file_name("exclusion.lock"),
            Fault::observe_with(move |_, _| {
                if entries.fetch_add(1, Ordering::Relaxed) == 2 {
                    say("paused");
                    wait_for_go();
                }
            }),
            When::Always,
            gate.clone(),
        );
    }
    if mode == "upgrade-after-schema" {
        policy = policy.inject_gated(
            FaultOp::RemoveFile,
            PathMatcher::file_name(canopy::UPGRADE_INTENT_NAME),
            Fault::observe_with(|_, _| {
                say("schema-advanced");
                wait_for_go();
            }),
            When::Once,
            gate.clone(),
        );
    }
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(OsFs::new(), policy));

    let mut minted = 0u64;
    let mut options = StoreOptions::with_nonce_source(
        canopy::Agent::new("canopy-tests", version, "Canopy test fixture", plan.source),
        move || {
            minted += 1;
            Nonce::new(&format!("writer-{}-{minted}", std::process::id())).expect("a valid nonce")
        },
    );
    if mode == "foreign" {
        options =
            options.with_coordination(Arc::new(acadia::coord::file::FileFactory::alternate()));
    }
    if matches!(
        mode,
        "reader" | "reader-wait" | "source-probe" | "resume-upgrade"
    ) {
        say("opening");
    }
    let opened = if matches!(mode, "reader" | "reader-wait") {
        Store::open_read_only(vfs, root, options).map(|store| (store, None))
    } else {
        Store::open(vfs, root, options).map(|(store, found)| (store, Some(found)))
    };
    let store = match opened {
        Ok((store, found)) => {
            if !schema_modes
                && !matches!(mode, "session" | "recover-step")
                && found
                    .is_some_and(|found| found.required() || found.unresolved_work() == Some(true))
                && let Err(error) = store.recover()
            {
                return fail(&format!("recovery failed: {error}"));
            }
            store
        }
        Err(StoreError::Coordination(acadia::coord::CoordError::ImplementationMismatch {
            ..
        })) if mode == "foreign" => {
            say("refused");
            return ExitCode::FAILURE;
        }
        Err(StoreError::SchemaMismatch { .. })
            if matches!(mode, "reader" | "reader-wait" | "source-probe") =>
        {
            say("schema-refused");
            return ExitCode::FAILURE;
        }
        Err(error) => return fail(&format!("open failed: {error}")),
    };
    say("opened");

    let (Ok(node), Ok(document)) = (NodePath::parse(node), DocumentName::new(document)) else {
        return fail("the node or the document is not a valid name");
    };
    gate.open();
    if matches!(
        mode,
        "upgrade-safe" | "upgrade-unsafe" | "upgrade-after-schema"
    ) {
        return upgrade::run(&store, &node, &document, body, plan);
    }
    if mode == "resume-upgrade" {
        if let Err(error) = upgrade::resume(&store, &node, &document, body) {
            return fail(&error);
        }
        say("recovered");
        return session(&store, &node, document.as_str(), body);
    }
    if matches!(mode, "reader" | "reader-wait" | "source-probe") {
        return session(&store, &node, document.as_str(), body);
    }
    if mode == "session" {
        return session(&store, &node, document.as_str(), body);
    }
    if mode == "recover-step" {
        let mut run = match store.recovery() {
            Ok(run) => run,
            Err(error) => return fail(&format!("recovery failed: {error}")),
        };
        while !run.progress().finished() {
            let phase = run.progress().phase();
            if let Err(error) = checked(run.step()) {
                return fail(&format!("step failed: {error}"));
            }
            say(&format!("phase:{phase:?}"));
            wait_for_go();
        }
        say("recovered");
    } else if mode == "scrub-paused" {
        if let Err(error) = checked(store.scrub_content()) {
            return fail(&error);
        }
        say("scrubbed");
    } else {
        let result = write(
            &store,
            &node,
            &document,
            body.as_bytes(),
            matches!(
                mode,
                "transaction" | "pause-transaction" | "fail-transaction"
            ),
        );
        if mode == "fail-transaction" {
            let required = match store.recovery_required() {
                Ok(required) => required,
                Err(error) => return fail(&format!("recovery status failed: {error}")),
            };
            if result.is_ok() || !required {
                return fail("the injected transaction failure did not fence the store");
            }
            say("failed");
        } else {
            if let Err(error) = result {
                return fail(&format!("write failed: {error}"));
            }
            say("wrote");
        }
    }
    gate.close();

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
