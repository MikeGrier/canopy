// Copyright (c) Michael Grier.
// Split from write_surface.rs at 3b07087.

use super::*;

// -- Snapshots -----------------------------------------------------------------

#[test]
fn an_undisturbed_snapshot_runs_its_read_once() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"1")))
        .unwrap();
    let mut runs = 0;
    let value = store
        .snapshot(|view| {
            runs += 1;
            view.body(name("alpha"), &doc("one")).unwrap()
        })
        .unwrap();
    assert_eq!(value, Some(body(b"1")));
    assert_eq!(runs, 1);
}

#[test]
fn a_snapshot_straddling_a_transaction_is_retried_and_sees_all_of_it() {
    let (_vfs, store) = seeded_store();
    store
        .apply_transaction(
            &SwapSet::new([
                Swap::new(name("alpha"), doc("one"), body(b"old")),
                Swap::new(name("beta"), doc("two"), body(b"old")),
            ])
            .unwrap(),
        )
        .unwrap();
    let writer = store.clone();
    let mut runs = 0;
    let (first, second) = store
        .snapshot(|view| {
            runs += 1;
            let first = view.body(name("alpha"), &doc("one")).unwrap();
            if runs == 1 {
                writer
                    .apply_transaction(
                        &SwapSet::new([
                            Swap::new(name("alpha"), doc("one"), body(b"new")),
                            Swap::new(name("beta"), doc("two"), body(b"new")),
                        ])
                        .unwrap(),
                    )
                    .unwrap();
            }
            let second = view.body(name("beta"), &doc("two")).unwrap();
            (first, second)
        })
        .unwrap();
    assert_eq!(runs, 2, "the torn first run was discarded");
    assert_eq!(first, Some(body(b"new")));
    assert_eq!(second, Some(body(b"new")));
}

#[test]
fn a_snapshot_taken_between_two_swaps_of_one_transaction_never_returns() {
    let probe: Arc<OnceLock<Store>> = Arc::default();
    let outcomes: Arc<Mutex<Vec<Result<(), String>>>> = Arc::default();
    let gate = FaultGate::closed();
    let (probe_store, recorded) = (Arc::clone(&probe), Arc::clone(&outcomes));
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(NAMESPACE_TREE_DIR),
            Fault::observe_with(move |_, _| {
                if let Some(store) = probe_store.get() {
                    let outcome = store
                        .snapshot(|view| view.body(name("alpha"), &doc("one")).unwrap())
                        .map(|_| ())
                        .map_err(|error| error.to_string());
                    recorded.lock().unwrap().push(outcome);
                }
            }),
            When::Always,
            gate.clone(),
        ),
    ));
    let store = seeded_store_on(vfs, "between");
    let _ = probe.set(store.clone());

    gate.open();
    let commit = store
        .apply_transaction(
            &SwapSet::new([
                Swap::new(name("alpha"), doc("one"), body(b"1")),
                Swap::new(name("beta"), doc("two"), body(b"2")),
            ])
            .unwrap(),
        )
        .unwrap();
    gate.close();

    assert!(commit.is_committed());
    let outcomes = outcomes.lock().unwrap().clone();
    assert!(
        outcomes.len() >= 2 * store.pointer_replica_count(),
        "every pointer rename of both swaps was probed: {outcomes:?}"
    );
    let contended = StoreError::SnapshotContended {
        attempts: SNAPSHOT_ATTEMPTS,
    }
    .to_string();
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome.as_ref() == Err(&contended)),
        "{outcomes:?}"
    );
    assert!(
        store.snapshot(|_| ()).is_ok(),
        "settled once the transaction ends"
    );
}

#[test]
fn a_snapshot_disturbed_on_every_attempt_reports_contention() {
    let (_vfs, store) = seeded_store();
    let writer = store.clone();
    let mut runs = 0;
    let result = store.snapshot(|_| {
        runs += 1;
        writer
            .write_document_replicated(
                name("alpha"),
                &doc("one"),
                &body(runs.to_string().as_bytes()),
            )
            .unwrap();
    });
    assert!(matches!(
        result,
        Err(StoreError::SnapshotContended { attempts }) if attempts == SNAPSHOT_ATTEMPTS
    ));
    assert_eq!(runs, SNAPSHOT_ATTEMPTS);
}

#[test]
fn a_snapshot_after_a_failed_publication_is_refused_until_recovery() {
    let (_vfs, store, gate) = store_failing_after_prepare("failed-publication");
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"before")))
        .unwrap();
    gate.open();
    assert!(
        store
            .update_document(Swap::new(name("alpha"), doc("one"), body(b"after")))
            .is_err()
    );
    gate.close();
    let mut runs = 0;
    let result = store.snapshot(|_| runs += 1);
    assert!(matches!(result, Err(StoreError::RecoveryRequired { .. })));
    assert_eq!(
        runs, 0,
        "a state recovery has not finished is never read as settled"
    );
}

#[test]
fn a_snapshot_after_a_failure_rolled_back_reads_the_prior_state() {
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(BLOB_TREE_DIR),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let store = seeded_store_on(vfs, "rolled-back");
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"before")))
        .unwrap();
    gate.open();
    assert!(
        store
            .update_document(Swap::new(name("alpha"), doc("one"), body(b"after")))
            .is_err()
    );
    gate.close();
    assert!(
        !store.recovery_required(),
        "it failed before its commit point"
    );
    let value = store
        .snapshot(|view| view.body(name("alpha"), &doc("one")).unwrap())
        .unwrap();
    assert_eq!(value, Some(body(b"before")));
}
