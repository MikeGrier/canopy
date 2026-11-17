// Copyright (c) Michael Grier.

//! `Coordination`'s bridge for its writers: acadia's contract, through both of its
//! implementations, driven along the model's writer behaviours (DESIGN-NOTES "The contract's
//! writer actions in the code").
//!
//! The replay is the contract's client: each of the model's writers is a thread that attaches,
//! acquires, opens and closes its operation, recovers and releases on a participant of its own,
//! over its own simulated process of one filesystem, while the cells, the log and the staging the
//! model gives a writer stay the client's own. A process is held before the first call of each
//! step -- a filesystem call, or for the shared implementation a step its hook announces -- until
//! the trace lets it take that step, and the contract's state is compared with the model's after
//! every step.

pub(super) mod process;
pub(super) mod projection;
mod replay;

use std::sync::Arc;

use acadia::coord::shared::SharedStep;
use acadia::coord::{ModificationEnd, Purpose, Waiting};
use acadia::{FaultOp, MemFs, Vfs};

use super::corpus::{self, Trace};
use process::{Class, Implementation};
use projection::Projection;
use replay::replay;

/// The configurations of writers alone, whose traces this bridge replays.
const WRITERS: [&str; 3] = [
    "Coordination-Writers",
    "Coordination-WritersTxn",
    "Coordination-WritersShared",
];

/// Whether the implementation did with `trace` what it must: follow it to the end, or leave it at
/// the step its `departs` names.
fn judge(trace: &Trace, implementation: Implementation) -> Result<(), String> {
    match (replay(trace, implementation), &trace.departs) {
        (Ok(()), None) => Ok(()),
        (Ok(()), Some(at)) => Err(format!(
            "the code followed it to its end; it must leave it at step {at}"
        )),
        (Err(departure), Some(at)) if trace.steps[departure.step].id == *at => Ok(()),
        (Err(departure), _) => Err(format!(
            "the code left it at {}: {}",
            trace.at(departure.step),
            departure.detail
        )),
    }
}

/// The implementations `trace` is a behaviour of. A count publication that fails is one the
/// file-based implementation alone can have, since the shared one's cannot fail. A crash leaves
/// the shared implementation's state fenced, since the state it rebuilds is, and cannot set the
/// file-based implementation's fence where it was clear.
fn implementations(trace: &Trace) -> Vec<Implementation> {
    let fails = trace
        .steps
        .iter()
        .any(|step| matches!(step.name.as_str(), "OpenFails" | "CloseFails"));
    let crashes = || {
        trace
            .steps
            .iter()
            .enumerate()
            .filter(|(_, step)| step.name == "Crash")
            .map(|(k, _)| (fenced(&trace.states[k]), fenced(&trace.states[k + 1])))
    };
    let mut implementations = Vec::new();
    if crashes().all(|(before, after)| before || !after) {
        implementations.push(Implementation::File);
    }
    if !fails && crashes().all(|(_, after)| after) {
        implementations.push(Implementation::Shared);
    }
    implementations
}

fn fenced(state: &serde_json::Map<String, serde_json::Value>) -> bool {
    state["fence"].as_bool().expect("the fence is a boolean")
}

/// The writer traces of the corpus.
fn writer_traces() -> Vec<Trace> {
    corpus::load("Coordination")
        .into_iter()
        .filter(|trace| {
            WRITERS
                .iter()
                .any(|configuration| trace.configuration == *configuration)
        })
        .collect()
}

#[test]
fn release_at_the_end_of_a_trace_clears_ownership() {
    for implementation in [Implementation::File, Implementation::Shared] {
        let (mut trace, step) = first("Release", implementation);
        trace.steps.truncate(step + 1);
        trace.states.truncate(step + 2);
        if let Err(error) = judge(&trace, implementation) {
            panic!("{} ({implementation:?}): {error}", trace.file);
        }
    }
}

#[test]
fn the_contract_follows_every_writer_trace() {
    let traces = writer_traces();
    assert!(!traces.is_empty(), "the corpus holds no writer trace");
    let mut failures = Vec::new();
    let mut replayed = 0;
    let mut shared = 0;
    for trace in &traces {
        let implementations = implementations(trace);
        if trace.configuration == "Coordination-WritersShared" {
            assert!(
                implementations.contains(&Implementation::Shared),
                "{} is not a behaviour of the shared implementation",
                trace.file
            );
        }
        for implementation in implementations {
            replayed += 1;
            if implementation == Implementation::Shared {
                shared += 1;
            }
            if let Err(error) = judge(trace, implementation) {
                failures.push(format!("{} ({implementation:?}): {error}", trace.file));
            }
        }
    }
    assert!(
        shared > 0 && replayed > shared,
        "each implementation replays a writer trace"
    );
    assert!(
        failures.is_empty(),
        "{} of {replayed} replays:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A writer trace that `implementation` follows to its end, and in it the first step that is
/// `name`.
fn first(name: &str, implementation: Implementation) -> (Trace, usize) {
    writer_traces()
        .into_iter()
        .filter(|trace| trace.departs.is_none() && implementations(trace).contains(&implementation))
        .find_map(|trace| {
            let k = trace.steps.iter().position(|step| step.name == name)?;
            Some((trace, k))
        })
        .unwrap_or_else(|| panic!("no writer trace takes {name}"))
}

/// A writer trace that `implementation` follows to its end, and in it the first step `k` that is
/// `name`, taken by a process whose next step is a call of its own, `k + 1`.
fn before_a_call(name: &str, implementation: Implementation) -> (Trace, usize) {
    writer_traces()
        .into_iter()
        .filter(|trace| trace.departs.is_none() && implementations(trace).contains(&implementation))
        .find_map(|trace| {
            let k = (0..trace.steps.len() - 1).find(|&k| {
                let (step, next) = (&trace.steps[k], &trace.steps[k + 1]);
                step.name == name
                    && next.args.first() == step.args.first()
                    && matches!(next.name.as_str(), "BeginOpen" | "BeginClose" | "Release")
            })?;
            Some((trace, k))
        })
        .unwrap_or_else(|| panic!("no writer trace takes {name} before a call"))
}

/// A writer trace of the shared implementation's behaviour with a crash, and the step that is the
/// crash.
fn crashing() -> (Trace, usize) {
    writer_traces()
        .into_iter()
        .filter(|trace| trace.configuration == "Coordination-WritersShared")
        .find_map(|trace| {
            let k = trace.steps.iter().position(|step| step.name == "Crash")?;
            Some((trace, k))
        })
        .expect("a writer trace crashes")
}

#[test]
fn a_count_that_is_not_the_model_s_leaves_the_trace_there() {
    for implementation in [Implementation::File, Implementation::Shared] {
        let (mut trace, k) = first("EndOpen", implementation);
        let count = trace.states[k + 1]["count"].as_u64().expect("a count");
        trace.states[k + 1]["count"] = (count + 1).into();
        let error = judge(&trace, implementation).expect_err("the code's count is odd");
        assert!(
            error.contains(&format!("action {}", k + 1))
                && error.contains("the count is odd where the model's is even"),
            "{implementation:?}: {error}"
        );
    }
}

#[test]
fn an_acquisition_that_is_not_told_where_the_model_s_is_leaves_the_trace_there() {
    for implementation in [Implementation::File, Implementation::Shared] {
        let (mut trace, k) = before_a_call("Record", implementation);
        let p = trace.steps[k].args[0].as_u64().expect("a process") as usize;
        assert_eq!(trace.states[k + 1]["pc"][p - 1], "held");
        trace.states[k + 1]["pc"][p - 1] = "told".into();
        let error = judge(&trace, implementation).expect_err("the code's holder is not told");
        assert!(
            error.contains(&format!("action {}", k + 1))
                && error.contains("returned Acquired { told: false } where the model's is told"),
            "{implementation:?}: {error}"
        );
    }
}

#[test]
fn a_writer_trace_marked_to_be_left_that_the_code_follows_is_refused() {
    let (mut trace, k) = first("EndOpen", Implementation::File);
    trace.departs = Some(trace.steps[k].id.clone());
    let error = judge(&trace, Implementation::File).expect_err("the code follows it to its end");
    assert!(error.contains("followed it to its end"), "{error}");
}

#[test]
fn each_implementation_replays_only_its_own_behaviours() {
    let (mut trace, k) = crashing();
    trace.states[k]["fence"] = false.into();
    trace.states[k + 1]["fence"] = true.into();
    assert_eq!(
        implementations(&trace),
        [Implementation::Shared],
        "the file-based implementation cannot set a fence in a crash"
    );
    trace.states[k + 1]["fence"] = false.into();
    assert_eq!(
        implementations(&trace),
        [Implementation::File],
        "a crash leaves the shared implementation fenced"
    );
    trace.states[k]["fence"] = true.into();
    trace.states[k + 1]["fence"] = true.into();
    assert_eq!(
        implementations(&trace),
        [Implementation::File, Implementation::Shared]
    );
    let last = trace.steps.len() - 1;
    trace.steps[last].name = "CloseFails".to_owned();
    assert_eq!(
        implementations(&trace),
        [Implementation::File],
        "the shared implementation's publications cannot fail"
    );
}

#[test]
fn calls_are_told_apart_by_the_step_they_begin() {
    let file = projection::root().join(".coord");
    let shared = projection::root().join(".coord-shared");
    let cases = [
        (
            Implementation::File,
            FaultOp::Lock,
            file.join("gate.lock"),
            Class::Gate,
        ),
        (
            Implementation::File,
            FaultOp::TryLock,
            file.join("exclusion.lock"),
            Class::Exclusion,
        ),
        (
            Implementation::File,
            FaultOp::LockExclusive,
            file.join("tiers.lock"),
            Class::Tiers,
        ),
        (
            Implementation::File,
            FaultOp::Write,
            file.join("fence"),
            Class::Fence,
        ),
        (
            Implementation::File,
            FaultOp::RemoveFile,
            file.join("fence"),
            Class::ClearFence,
        ),
        (
            Implementation::File,
            FaultOp::RemoveFile,
            file.join("owner"),
            Class::ClearRecord,
        ),
        (
            Implementation::File,
            FaultOp::Write,
            file.join("generation.staging.3"),
            Class::Record,
        ),
        (
            Implementation::File,
            FaultOp::Write,
            file.join("count.staging.1"),
            Class::Count,
        ),
        (
            Implementation::File,
            FaultOp::Read,
            file.join("fence"),
            Class::Other,
        ),
        (
            Implementation::File,
            FaultOp::Write,
            file.join("owner"),
            Class::Other,
        ),
        (
            Implementation::File,
            FaultOp::Lock,
            file.join("reader-tier.lock"),
            Class::Other,
        ),
        (
            Implementation::File,
            FaultOp::Lock,
            shared.join("gate.lock"),
            Class::Other,
        ),
        (
            Implementation::File,
            FaultOp::Write,
            projection::root().join("fence"),
            Class::Other,
        ),
        (
            Implementation::Shared,
            FaultOp::Lock,
            shared.join("gate.lock"),
            Class::Gate,
        ),
        (
            Implementation::Shared,
            FaultOp::Lock,
            shared.join("reader-tier.lock"),
            Class::ReaderTier,
        ),
        (
            Implementation::Shared,
            FaultOp::TryLock,
            shared.join("writer-tier.lock"),
            Class::WriterTier,
        ),
        (
            Implementation::Shared,
            FaultOp::Lock,
            shared.join("exclusion.lock"),
            Class::Exclusion,
        ),
        (
            Implementation::Shared,
            FaultOp::Lock,
            shared.join("tiers.lock"),
            Class::Other,
        ),
        (
            Implementation::Shared,
            FaultOp::Write,
            shared.join("fence"),
            Class::Other,
        ),
        (
            Implementation::Shared,
            FaultOp::Lock,
            file.join("exclusion.lock"),
            Class::Other,
        ),
    ];
    for (implementation, op, path, class) in cases {
        assert_eq!(
            process::classify(implementation, op, &path),
            class,
            "{implementation:?} {op:?} {}",
            path.display()
        );
    }
}

#[test]
fn each_announced_step_begins_the_model_step_it_is_named_for() {
    let cases = [
        (SharedStep::SetFence, Class::Fence),
        (SharedStep::Unresolved, Class::Fence),
        (SharedStep::Record, Class::Record),
        (SharedStep::BeginRaise, Class::Other),
        (SharedStep::PublishRaise, Class::Count),
        (SharedStep::BeginClose, Class::Other),
        (SharedStep::PublishClose, Class::Count),
        (SharedStep::PublishRecovered, Class::Count),
        (SharedStep::ClearFence, Class::ClearFence),
        (SharedStep::ClearRecord, Class::ClearRecord),
    ];
    for (step, class) in cases {
        assert_eq!(process::announced(step), class, "{step:?}");
    }
}

#[test]
fn each_implementation_s_state_is_read_as_the_model_s() {
    for implementation in [Implementation::File, Implementation::Shared] {
        let mem = MemFs::new();
        mem.create_dir_all(projection::root()).expect("the root");
        mem.fsync(projection::root()).expect("the root is durable");
        let observer = Implementation::File
            .participant(Arc::new(mem.another_process()), None)
            .expect("the observer");
        let participant = implementation
            .participant(Arc::new(mem.another_process()), None)
            .expect("a participant");
        let read = || projection::observe(&mem, implementation, observer.as_ref());
        let mut hold = participant.acquire(Waiting::Wait).expect("exclusion");
        let found = read();
        assert!(
            !found.trusted && found.owner,
            "{implementation:?}: {found:?}"
        );
        hold.complete_recovery().expect("recovery");
        let recovered = read();
        assert_eq!(
            (
                recovered.trusted,
                recovered.fence,
                recovered.owner,
                recovered.count_odd
            ),
            (true, false, true, false),
            "{implementation:?}"
        );
        hold.begin_modification(Purpose::Ordinary)
            .expect("an operation opens");
        assert!(read().count_odd, "{implementation:?}");
        hold.end_modification(ModificationEnd::Settled)
            .expect("the operation closes");
        assert!(!read().count_odd, "{implementation:?}");
        hold.release().expect("a clean release");
        let released = read();
        assert!(!released.owner, "{implementation:?}");
        let hold = participant.acquire(Waiting::Wait).expect("exclusion again");
        let again = read();
        assert!(again.owner, "{implementation:?}");
        assert_ne!(again.generation, released.generation, "{implementation:?}");
        hold.release().expect("a clean release");
    }
}

#[test]
fn state_that_cannot_be_vouched_for_is_compared_only_as_that() {
    let untrusted = Projection {
        trusted: false,
        fence: false,
        owner: false,
        count_odd: false,
        generation: 1,
    };
    let anything = Projection {
        fence: true,
        owner: true,
        count_odd: true,
        generation: 9,
        ..untrusted
    };
    assert_eq!(
        projection::differ(&untrusted, &untrusted, &untrusted, &anything),
        None
    );
    let trusted = Projection {
        trusted: true,
        ..untrusted
    };
    let difference =
        projection::differ(&trusted, &trusted, &trusted, &untrusted).expect("trust differs");
    assert!(
        difference.contains("untrusted where the model's is trusted"),
        "{difference}"
    );
}

#[test]
fn trusted_state_is_compared_word_by_word_and_the_generation_by_its_advance() {
    let trusted = Projection {
        trusted: true,
        fence: false,
        owner: true,
        count_odd: false,
        generation: 1,
    };
    let cases = [
        (
            Projection {
                fence: true,
                ..trusted
            },
            "the fence is set where the model's is clear",
        ),
        (
            Projection {
                owner: false,
                ..trusted
            },
            "the ownership record is absent where the model's is present",
        ),
        (
            Projection {
                count_odd: true,
                ..trusted
            },
            "the count is odd where the model's is even",
        ),
    ];
    for (code, expected) in cases {
        let difference =
            projection::differ(&trusted, &trusted, &trusted, &code).expect("a difference");
        assert!(difference.contains(expected), "{difference}");
    }
    let advanced = Projection {
        generation: 2,
        ..trusted
    };
    let code_before = Projection {
        generation: 10,
        ..trusted
    };
    let code_advanced = Projection {
        generation: 15,
        ..trusted
    };
    assert_eq!(
        projection::differ(&trusted, &advanced, &code_before, &code_advanced),
        None
    );
    let difference = projection::differ(&trusted, &advanced, &code_before, &code_before)
        .expect("the generation stayed");
    assert!(
        difference.contains("the generation stayed where the model's advanced"),
        "{difference}"
    );
    let difference = projection::differ(&trusted, &trusted, &code_before, &code_advanced)
        .expect("the generation advanced");
    assert!(
        difference.contains("the generation advanced where the model's stayed"),
        "{difference}"
    );
}
