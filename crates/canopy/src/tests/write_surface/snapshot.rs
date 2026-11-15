// Copyright (c) Michael Grier.
// Split from write_surface.rs at 3b07087.

use super::*;
use std::sync::Barrier;
use std::time::Duration;
use test_timers::mpsc::recv_timeout;

// -- Snapshots -----------------------------------------------------------------

#[test]
fn an_undisturbed_snapshot_runs_its_read_once() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"1")))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
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
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
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
                    .unwrap()
                    .agent_stamp
                    .expect("agent version stamp");
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
fn a_snapshot_waits_for_a_transaction_and_reads_its_final_state() {
    let (store, release, gate, writer) = store_with_held_transaction(Duration::from_secs(1));
    let reader = store.clone();
    let (started, waiting) = std::sync::mpsc::channel();
    let (completed, result) = std::sync::mpsc::channel();
    let snapshot = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = reader
            .snapshot(|view| {
                (
                    view.body(name("alpha"), &doc("one")).unwrap(),
                    view.body(name("beta"), &doc("two")).unwrap(),
                )
            })
            .map_err(|error| error.to_string());
        completed.send(result).unwrap();
    });
    waiting.recv().unwrap();
    assert!(matches!(
        recv_timeout(&result, Duration::from_millis(25)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));

    release.wait();
    assert!(writer.join().unwrap().unwrap().is_committed());
    gate.close();
    assert_eq!(
        recv_timeout(&result, Duration::from_secs(1))
            .unwrap()
            .unwrap(),
        (Some(body(b"new")), Some(body(b"new")))
    );
    snapshot.join().unwrap();
}

#[test]
fn a_snapshot_reports_contention_when_a_write_exceeds_its_deadline() {
    let (store, release, gate, writer) = store_with_held_transaction(Duration::from_millis(50));
    let result = store.snapshot(|_| ());
    assert!(matches!(result, Err(StoreError::SnapshotContended { .. })));

    release.wait();
    assert!(writer.join().unwrap().unwrap().is_committed());
    gate.close();
}

type HeldTransaction = (
    Store,
    Arc<Barrier>,
    FaultGate,
    std::thread::JoinHandle<Result<crate::WriteOutcome<Commit>, StoreError>>,
);

fn store_with_held_transaction(timeout: Duration) -> HeldTransaction {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let (observed, held) = (Arc::clone(&entered), Arc::clone(&release));
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(NAMESPACE_TREE_DIR),
            Fault::observe_with(move |_, _| {
                observed.wait();
                held.wait();
            }),
            When::Once,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("snapshot-wait");
    vfs.create_dir_all(&root).unwrap();
    let options = StoreOptions::with_nonce_source(test_agent(), seeded_source(TEST_SEED, 0))
        .with_snapshot_timeout(timeout);
    let store = Store::init(Arc::clone(&vfs), root, test_config(), options).unwrap();
    store
        .apply_transaction(
            &SwapSet::new([
                Swap::new(name("alpha"), doc("one"), body(b"old")),
                Swap::new(name("beta"), doc("two"), body(b"old")),
            ])
            .unwrap(),
        )
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    let writer_store = store.clone();
    let writer_gate = gate.clone();
    let writer = std::thread::spawn(move || {
        writer_gate.open();
        writer_store.apply_transaction(
            &SwapSet::new([
                Swap::new(name("alpha"), doc("one"), body(b"new")),
                Swap::new(name("beta"), doc("two"), body(b"new")),
            ])
            .unwrap(),
        )
    });
    entered.wait();
    (store, release, gate, writer)
}

#[test]
fn a_snapshot_disturbed_on_every_attempt_reports_contention() {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("disturbed-snapshot");
    vfs.create_dir_all(&root).unwrap();
    let options = StoreOptions::with_nonce_source(test_agent(), seeded_source(TEST_SEED, 0))
        .with_snapshot_timeout(Duration::from_millis(25));
    let store = Store::init(vfs, root, test_config(), options).unwrap();
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
            .unwrap()
            .agent_stamp
            .expect("agent version stamp");
    });
    assert!(matches!(
        result,
        Err(StoreError::SnapshotContended { attempts }) if attempts > 0
    ));
    assert_eq!(
        runs,
        match result {
            Err(StoreError::SnapshotContended { attempts }) => attempts,
            _ => unreachable!(),
        }
    );
}

#[test]
fn a_snapshot_after_a_failed_publication_is_refused_until_recovery() {
    let (_vfs, store, gate) = store_failing_after_prepare("failed-publication");
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"before")))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
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
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    gate.open();
    assert!(
        store
            .update_document(Swap::new(name("alpha"), doc("one"), body(b"after")))
            .is_err()
    );
    gate.close();
    assert!(
        !store.recovery_required().unwrap(),
        "it failed before its commit point"
    );
    let value = store
        .snapshot(|view| view.body(name("alpha"), &doc("one")).unwrap())
        .unwrap();
    assert_eq!(value, Some(body(b"before")));
}
