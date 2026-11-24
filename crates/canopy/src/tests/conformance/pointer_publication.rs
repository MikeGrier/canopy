// Copyright (c) Michael Grier.

//! `PointerPublication`'s bridge: one process and one document (DESIGN-NOTES "Replaying a trace
//! against the code").
//!
//! An operation a trace starts runs whole against the store, and the calls it makes are grouped
//! into runs of one kind of call; each run that changes the projection must make the trace's next change. A
//! `Fail` fails the first call of the step the model was about to take, once the code has made
//! the changes the model made before it; a `Crash` kills the process at the first call after
//! them, then loses what was not durable. Failing or killing at that point, rather than at a
//! counted call, keeps the calls the model leaves out -- staging, directory creation -- from
//! deciding where a fault lands.

pub(super) mod class;
mod projection;
mod replay;

use super::corpus::{self, Trace};
use replay::{replay, replay_without_collection_sync};

/// Whether the store did with `trace` what it must: follow it to the end, or leave it at the step
/// its `departs` names.
fn judge(trace: &Trace) -> Result<(), String> {
    match (replay(trace), &trace.departs) {
        (Ok(()), None) => Ok(()),
        (Ok(()), Some(at)) => Err(format!(
            "the store followed it to its end; it must leave it at step {at}"
        )),
        (Err(departure), Some(at))
            if trace.steps[departure.from..=departure.to]
                .iter()
                .any(|step| step.id == *at) =>
        {
            Ok(())
        }
        (Err(departure), _) if departure.from == departure.to => Err(format!(
            "the store left it at {}: {}",
            trace.at(departure.to),
            departure.detail
        )),
        (Err(departure), _) => Err(format!(
            "the store left it between {} and {}: {}",
            trace.at(departure.from),
            trace.at(departure.to),
            departure.detail
        )),
    }
}

#[test]
fn the_store_follows_every_pointer_publication_trace() {
    let traces = corpus::load("PointerPublication");
    assert!(
        !traces.is_empty(),
        "the corpus holds no PointerPublication trace"
    );
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
    corpus::load("PointerPublication")
        .into_iter()
        .find(|trace| trace.file == format!("PointerPublication/{file}"))
        .unwrap_or_else(|| panic!("no trace {file}"))
}

#[test]
fn a_trace_whose_step_the_store_does_not_take_is_left_at_that_step() {
    let mut trace = trace("T26.json");
    let sync = trace
        .steps
        .iter()
        .position(|step| step.name == "IngestSync")
        .expect("T26 syncs an ingested blob");
    let unsynced = trace.states[sync]["blobDur"].clone();
    trace.states[sync + 1].insert("blobDur".to_owned(), unsynced);
    let error = judge(&trace).expect_err("the store syncs the blob the altered trace does not");
    assert!(error.contains("step ING"), "{error}");
}

#[test]
fn collection_sync_removal_departs_at_the_sync_that_changes_the_durable_namespace() {
    let trace = trace("T32.json");
    let sync = trace
        .steps
        .iter()
        .enumerate()
        .position(|(index, step)| {
            step.name == "CollSync"
                && trace.states[index]["ptrDur"] != trace.states[index + 1]["ptrDur"]
        })
        .expect("T32 syncs a repaired live entry before collecting");
    let departure = replay_without_collection_sync(&trace)
        .expect_err("the replay must see the absent collection sync");
    assert_eq!(
        (departure.from, departure.to),
        (sync, sync),
        "{}",
        departure.detail
    );
}

#[test]
fn a_trace_marked_to_be_left_that_the_store_follows_is_refused() {
    let mut trace = trace("T26.json");
    trace.departs = Some("FAI".to_owned());
    let error = judge(&trace).expect_err("the store follows T26 to its end");
    assert!(error.contains("followed it to its end"), "{error}");
}

#[test]
fn a_control_trace_left_at_another_step_than_marked_is_refused() {
    let mut trace = trace("T24.json");
    trace.departs = Some("ING".to_owned());
    let error = judge(&trace).expect_err("the store leaves T24 where its settle syncs");
    assert!(error.contains("step OPN"), "{error}");
}

#[test]
fn a_control_trace_without_its_mark_is_refused() {
    let mut trace = trace("T24.json");
    trace.departs = None;
    assert!(
        judge(&trace).is_err(),
        "the store does not settle as T24's control does"
    );
}

#[test]
fn a_control_trace_is_accepted_where_its_rule_acts() {
    for file in [
        "T24.json",
        "T31.json",
        "cex-DANGLING-DURABLE.json",
        "cex-STRANDED-VOTE.json",
    ] {
        let trace = trace(file);
        assert!(trace.departs.is_some(), "{file} is marked");
        judge(&trace).unwrap_or_else(|error| panic!("{file}: {error}"));
    }
}
