// Copyright (c) Michael Grier.

//! How calls are grouped into the steps a model takes.

use super::{Recorder, Runs, Verdict, Watcher};
use acadia::{FaultOp, Vfs, VfsErrorKind};
use std::path::Path;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Rename,
    Sync,
    Other,
}

#[test]
fn recorder_preserves_process_hooks() {
    let recorder = Recorder::new();
    drop(recorder.mem().simulate_restart());
    assert_eq!(
        Vfs::check_process(&recorder, "read", Path::new("/file"))
            .unwrap_err()
            .kind(),
        VfsErrorKind::ProcessEnded
    );
    let committed = AtomicBool::new(false);
    assert_eq!(
        Vfs::commit_process(&recorder, "read", Path::new("/file"), &mut || {
            committed.store(true, Ordering::Release);
        })
        .unwrap_err()
        .kind(),
        VfsErrorKind::ProcessEnded
    );
    assert!(!committed.load(Ordering::Acquire));
}

struct FailOnePath;

impl Watcher for FailOnePath {
    fn before(&self, _op: FaultOp, path: &Path, _to: Option<&Path>) -> Verdict {
        if path == Path::new("/fail") {
            Verdict::Fail
        } else {
            Verdict::Proceed
        }
    }

    fn after(&self, _op: FaultOp, _path: &Path, _to: Option<&Path>) {}
}

#[test]
fn concurrent_calls_keep_their_own_fault_verdict() {
    let recorder = Arc::new(Recorder::new());
    recorder
        .mem()
        .write(Path::new("/ok"), b"ok")
        .expect("write");
    recorder.watch(Arc::new(FailOnePath));
    let start = Arc::new(Barrier::new(2));
    let failed = {
        let recorder = Arc::clone(&recorder);
        let start = Arc::clone(&start);
        thread::spawn(move || {
            for _ in 0..128 {
                start.wait();
                assert_eq!(
                    recorder.read(Path::new("/fail")).unwrap_err().kind(),
                    VfsErrorKind::Other
                );
            }
        })
    };
    let proceeded = {
        let recorder = Arc::clone(&recorder);
        thread::spawn(move || {
            for _ in 0..128 {
                start.wait();
                assert_eq!(recorder.read(Path::new("/ok")).expect("read"), b"ok");
            }
        })
    };
    failed.join().expect("failed calls");
    proceeded.join().expect("proceeding calls");
}

#[test]
fn a_run_that_changes_nothing_records_no_change() {
    let mut runs = Runs::new(0);
    assert_eq!(runs.begin(Kind::Other), None);
    runs.observe(0);
    assert_eq!(runs.end(), None);
    assert!(runs.changes().is_empty());
}

#[test]
fn a_change_is_recorded_when_a_call_of_another_kind_begins() {
    let mut runs = Runs::new(0);
    runs.begin(Kind::Rename);
    runs.observe(1);
    assert_eq!(runs.begin(Kind::Sync), Some(0));
    assert_eq!(runs.changes(), &[(Kind::Rename, 1)]);
}

#[test]
fn calls_of_one_kind_in_a_row_are_one_step() {
    let mut runs = Runs::new(0);
    runs.begin(Kind::Rename);
    runs.observe(1);
    assert_eq!(runs.begin(Kind::Rename), None);
    runs.observe(2);
    assert_eq!(runs.end(), Some(0));
    assert_eq!(runs.changes(), &[(Kind::Rename, 2)]);
}

#[test]
fn ending_records_the_run_in_progress() {
    let mut runs = Runs::new(0);
    runs.begin(Kind::Sync);
    runs.observe(5);
    assert_eq!(runs.end(), Some(0));
    assert_eq!(runs.end(), None);
    assert_eq!(runs.changes(), &[(Kind::Sync, 5)]);
}

#[test]
fn a_run_whose_calls_undo_each_other_records_nothing() {
    let mut runs = Runs::new(3);
    runs.begin(Kind::Rename);
    runs.observe(4);
    runs.begin(Kind::Rename);
    runs.observe(3);
    assert_eq!(runs.end(), None);
    assert!(runs.changes().is_empty());
}

#[test]
fn runs_of_alternating_kinds_each_record_their_change() {
    let mut runs = Runs::new(0);
    for (index, kind) in [Kind::Rename, Kind::Sync, Kind::Rename, Kind::Sync]
        .into_iter()
        .enumerate()
    {
        runs.begin(kind);
        runs.observe(index + 1);
    }
    runs.end();
    assert_eq!(
        runs.changes(),
        &[
            (Kind::Rename, 1),
            (Kind::Sync, 2),
            (Kind::Rename, 3),
            (Kind::Sync, 4)
        ]
    );
}

#[test]
fn a_run_between_changing_runs_that_changes_nothing_is_skipped() {
    let mut runs = Runs::new(0);
    runs.begin(Kind::Rename);
    runs.observe(1);
    assert_eq!(runs.begin(Kind::Other), Some(0));
    runs.observe(1);
    assert_eq!(runs.begin(Kind::Sync), None);
    runs.observe(2);
    assert_eq!(runs.end(), Some(1));
    assert_eq!(runs.changes(), &[(Kind::Rename, 1), (Kind::Sync, 2)]);
}

#[test]
fn ending_with_no_run_records_nothing() {
    let mut runs: Runs<Kind, u32> = Runs::new(7);
    assert_eq!(runs.end(), None);
    assert!(runs.changes().is_empty());
    assert_eq!(*runs.current(), 7);
}

#[test]
fn the_current_state_is_the_last_one_observed() {
    let mut runs = Runs::new(0);
    runs.begin(Kind::Rename);
    runs.observe(9);
    assert_eq!(*runs.current(), 9);
    runs.begin(Kind::Sync);
    assert_eq!(*runs.current(), 9);
}

#[test]
fn a_run_begins_from_what_the_last_run_left() {
    let mut runs = Runs::new(0);
    runs.begin(Kind::Rename);
    runs.observe(1);
    runs.begin(Kind::Sync);
    runs.observe(1);
    assert_eq!(
        runs.end(),
        None,
        "the sync changed nothing the rename had not"
    );
    assert_eq!(runs.changes(), &[(Kind::Rename, 1)]);
}
