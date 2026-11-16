// Copyright (c) Michael Grier.

//! `Creation`'s bridge: acadia's creation contract, through both of its implementations, driven
//! along the model's behaviours (DESIGN-NOTES "The Creation replay").
//!
//! Each of the model's processes is a thread with a participant of its own -- the file-based
//! implementation for the model's implementation 1, the shared one for 2 -- over its own simulated
//! process of one filesystem. A process is held before the first call of each step until the
//! replay lets it take that step, and then runs to the first call of its next, so the processes
//! interleave as the trace does. After every step the root is read as the model's root variables
//! and compared, and a process the model ends must have returned what the model's ended with.

mod process;
mod projection;
mod replay;

use std::sync::Arc;

use acadia::coord::file::FileFactory;
use acadia::coord::shared::SharedFactory;
use acadia::coord::{CoordinationFactory, CreationNonce};
use acadia::{FaultOp, MemFs, Vfs};

use super::corpus::{self, Trace};
use process::Class;
use replay::replay;

/// Whether the code did with `trace` what it must: follow it to the end, or leave it at the step
/// its `departs` names.
fn judge(trace: &Trace) -> Result<(), String> {
    match (replay(trace), &trace.departs) {
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

#[test]
fn the_coordination_follows_every_creation_trace() {
    let traces = corpus::load("Creation");
    assert!(!traces.is_empty(), "the corpus holds no Creation trace");
    let failures: Vec<String> = traces
        .iter()
        .filter_map(|trace| {
            judge(trace)
                .err()
                .map(|error| format!("{}: {error}", trace.file))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} traces:\n{}",
        failures.len(),
        traces.len(),
        failures.join("\n")
    );
}

/// The corpus trace in `file`.
fn trace(file: &str) -> Trace {
    corpus::load("Creation")
        .into_iter()
        .find(|trace| trace.file == format!("Creation/{file}"))
        .unwrap_or_else(|| panic!("no trace {file}"))
}

/// A trace the code follows to its end in which process `p` ends with `outcome` at step `k`.
fn ending(outcome: &str) -> (Trace, usize, usize) {
    corpus::load("Creation")
        .into_iter()
        .filter(|trace| trace.departs.is_none())
        .find_map(|trace| {
            let found = (0..trace.steps.len()).find_map(|k| {
                let out = trace.states[k + 1]["out"].as_array()?;
                let before = trace.states[k]["out"].as_array()?;
                (0..out.len())
                    .find(|&i| out[i] == outcome && before[i] != outcome)
                    .map(|i| (k, i + 1))
            });
            found.map(|(k, p)| (trace, k, p))
        })
        .unwrap_or_else(|| panic!("no trace in which a process ends {outcome}"))
}

#[test]
fn a_process_that_ends_otherwise_than_the_model_says_leaves_the_trace_there() {
    let (mut trace, k, p) = ending("created");
    trace.states[k + 1]["out"][p - 1] = "resumed".into();
    let error = judge(&trace).expect_err("the code creates where the altered trace resumes");
    assert!(
        error.contains(&format!("action {}", k + 1))
            && error.contains("ended created where the model's ended resumed"),
        "{error}"
    );
}

#[test]
fn a_root_that_is_not_the_model_s_leaves_the_trace_there() {
    let (mut trace, k, _) = ending("created");
    let artifacts = trace.states[k + 1]["adur"]
        .as_array()
        .expect("adur is a sequence")
        .len();
    trace.states[k + 1]["adur"][artifacts - 1] = false.into();
    let error = judge(&trace).expect_err("the code makes the marker durable");
    assert!(
        error.contains(&format!("action {}", k + 1)) && error.contains("the root holds"),
        "{error}"
    );
}

#[test]
fn a_process_the_model_ends_that_goes_on_leaves_the_trace_there() {
    let (mut trace, k, p) = ending("created");
    trace.states[k + 1]["pc"][p - 1] = "syncArt".into();
    trace.states[k + 1]["out"][p - 1] = "none".into();
    let error = judge(&trace).expect_err("the code ends where the altered trace goes on");
    assert!(
        error.contains("ended created where the model's goes on"),
        "{error}"
    );
}

#[test]
fn a_trace_marked_to_be_left_that_the_code_follows_is_refused() {
    let (mut trace, k, _) = ending("created");
    trace.departs = Some(trace.steps[k].id.clone());
    let error = judge(&trace).expect_err("the code follows the trace to its end");
    assert!(error.contains("followed it to its end"), "{error}");
}

#[test]
fn a_control_trace_left_at_another_step_than_marked_is_refused() {
    let mut trace = trace("T12.json");
    trace.departs = Some("LOOK2".to_owned());
    let error = judge(&trace).expect_err("the code leaves T12 where the resumer reads again");
    assert!(error.contains("step CNFM"), "{error}");
}

#[test]
fn a_control_trace_without_its_mark_is_refused() {
    for file in ["T10.json", "T11.json", "T12.json", "T13.json"] {
        let mut trace = trace(file);
        trace.departs = None;
        assert!(judge(&trace).is_err(), "{file}: the code breaks no rule");
    }
}

#[test]
fn calls_are_told_apart_by_the_step_they_begin() {
    let root = projection::root();
    let reservation = root.join(projection::RESERVATION);
    let cases = [
        (FaultOp::ReadDir, root.to_path_buf(), Class::LookRoot),
        (FaultOp::Fsync, root.to_path_buf(), Class::SyncRoot),
        (FaultOp::Metadata, root.to_path_buf(), Class::Other),
        (
            FaultOp::Lock,
            root.join(".coord-creation.lock"),
            Class::CreationLock,
        ),
        (
            FaultOp::Lock,
            root.join(".coord-shared-creation.lock"),
            Class::CreationLock,
        ),
        (
            FaultOp::TryLock,
            root.join(".coord-creation.lock"),
            Class::CreationLock,
        ),
        (FaultOp::Lock, root.join(".coord-other.lock"), Class::Other),
        (FaultOp::Fsync, reservation.clone(), Class::SyncReservation),
        (FaultOp::Read, reservation.clone(), Class::Other),
        (FaultOp::PublishNew, reservation, Class::Other),
        (FaultOp::Metadata, root.join("a1"), Class::Artifact(1)),
        (FaultOp::Write, root.join("a2"), Class::Artifact(2)),
        (FaultOp::Fsync, root.join("a2"), Class::Artifact(2)),
        (FaultOp::Write, root.join("a3"), Class::Other),
        (FaultOp::Write, root.join("sub").join("a1"), Class::Other),
        (FaultOp::ReadDir, root.join("sub"), Class::Other),
    ];
    for (op, path, class) in cases {
        assert_eq!(
            process::classify(op, &path, 2),
            class,
            "{op:?} {}",
            path.display()
        );
    }
}

/// A root with `NArt` 2, as each implementation leaves it after reserving with inputs `2` and
/// nonce 3, and the model's reading of it.
fn reserved(implementation: u64) -> projection::Projection {
    let mem = MemFs::new();
    mem.create_dir_all(projection::root()).expect("the root");
    mem.fsync(projection::root()).expect("the root is durable");
    let factory: Box<dyn CoordinationFactory> = match implementation {
        1 => Box::new(FileFactory::new()),
        _ => Box::new(SharedFactory::new()),
    };
    let participant = factory
        .open(Arc::new(mem.another_process()), projection::root())
        .expect("a participant");
    participant
        .reserve(b"2", CreationNonce::from_bytes([3; 16]))
        .expect("the reservation");
    projection::observe(&mem, 2)
}

#[test]
fn a_reservation_is_read_as_the_model_s() {
    for implementation in [1, 2] {
        let root = reserved(implementation);
        assert_eq!(
            root.res,
            projection::Reservation {
                st: "whole".to_owned(),
                implementation,
                inputs: 2,
                nonce: 3,
            }
        );
        assert!(root.rdur, "a reservation is durable once reserved");
        assert_eq!(root.aliv, [false, false]);
    }
}

#[test]
fn a_reservation_cut_short_is_partial_and_an_unsynced_artifact_is_not_durable() {
    let mem = MemFs::new();
    mem.create_dir_all(projection::root()).expect("the root");
    mem.fsync(projection::root()).expect("the root is durable");
    let reservation = projection::root().join(projection::RESERVATION);
    mem.write(&reservation, b"ACRV\x01").expect("a reservation");
    mem.write(&projection::artifact_path(1), b"1")
        .expect("an artifact");
    mem.fsync(&projection::artifact_path(1))
        .expect("its bytes are durable");
    mem.write(&projection::artifact_path(2), b"x")
        .expect("an artifact");
    let root = projection::observe(&mem, 2);
    assert_eq!(root.res.st, "partial");
    assert!(!root.rdur);
    assert_eq!(root.aliv, [true, true]);
    assert_eq!(root.ainp, [1, 99]);
    assert_eq!(root.adur, [false, false]);
    mem.fsync(projection::root())
        .expect("the names are durable");
    let root = projection::observe(&mem, 2);
    assert_eq!(
        root.adur,
        [true, false],
        "the marker's bytes are not durable"
    );
}

#[test]
fn every_refusal_for_a_reservation_is_the_code_s_one() {
    for model in ["otherImpl", "differentInputs", "lostRace", "changed"] {
        assert_eq!(process::expected(model), "reserved", "{model}");
    }
    for model in [
        "created",
        "resumed",
        "exists",
        "occupied",
        "partial",
        "opened",
        "notAStore",
        "mismatch",
    ] {
        assert_eq!(process::expected(model), model);
    }
}

#[test]
fn a_control_trace_is_accepted_where_its_rule_acts() {
    for file in ["T10.json", "T11.json", "T12.json", "T13.json"] {
        let trace = trace(file);
        assert!(trace.departs.is_some(), "{file} is marked");
        judge(&trace).unwrap_or_else(|error| panic!("{file}: {error}"));
    }
}
