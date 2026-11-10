// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! LAY-5: the single-writer monitor -- serialized transforms and conditional writes.

use super::*;

use std::error::Error as _;
use std::sync::Mutex;

use crate::monitor::{CommitObserver, MonitorError};

// -- Store: single-writer monitor (LAY-5) --

#[test]
fn transform_sees_none_and_creates_when_absent() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("record");
    let d = doc("record.json");
    let out = monitor
        .transform(node.clone(), d.clone(), |current| {
            if current.is_none() {
                body(b"created")
            } else {
                body(b"existed")
            }
        })
        .unwrap();
    assert_eq!(out, body(b"created"));
    let read = monitor.store().read_body(&node, &d).unwrap();
    assert_eq!(read, Some(body(b"created")));
}

#[test]
fn transform_reads_and_updates_the_current_body() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("record");
    let d = doc("record.json");
    monitor.write(node.clone(), d.clone(), body(b"v1")).unwrap();
    let out = monitor
        .transform(node.clone(), d.clone(), |current| {
            let mut bytes = current.map(|b| b.as_bytes().to_vec()).unwrap_or_default();
            bytes.extend_from_slice(b"+v2");
            body(&bytes)
        })
        .unwrap();
    assert_eq!(out, body(b"v1+v2"));
    let read = monitor.store().read_body(&node, &d).unwrap();
    assert_eq!(read, Some(body(b"v1+v2")));
}

#[test]
fn write_overwrites_the_previous_body() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("record");
    let d = doc("record.json");
    monitor.write(node.clone(), d.clone(), body(b"v1")).unwrap();
    monitor.write(node.clone(), d.clone(), body(b"v2")).unwrap();
    let read = monitor.store().read_body(&node, &d).unwrap();
    assert_eq!(read, Some(body(b"v2")));
}

#[test]
fn write_leaves_no_staging_orphan() {
    let (vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("record");
    let d = doc("record.json");
    monitor.write(node.clone(), d.clone(), body(b"x")).unwrap();
    assert_eq!(
        node_entries(&vfs, monitor.store(), 0),
        vec![OsString::from("RECORD.JSON")]
    );
}

#[test]
fn serialized_transforms_have_no_lost_update() {
    let (_vfs, store) = fresh_store();
    let monitor = Arc::new(WriteMonitor::spawn(store, counting_nonce_source()).unwrap());
    let node = name("counter");
    let d = doc("count");
    monitor.write(node.clone(), d.clone(), body(b"0")).unwrap();

    const WRITERS: u64 = 20;
    let handles: Vec<_> = (0..WRITERS)
        .map(|_| {
            let monitor = Arc::clone(&monitor);
            let node = node.clone();
            let d = d.clone();
            thread::spawn(move || {
                monitor
                    .transform(node, d, |current| {
                        let n: u64 = current
                            .map(|b| std::str::from_utf8(b.as_bytes()).unwrap().parse().unwrap())
                            .unwrap_or(0);
                        body(format!("{}", n + 1).as_bytes())
                    })
                    .unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let read = monitor.store().read_body(&node, &d).unwrap();
    let final_body = read.unwrap();
    assert_eq!(
        std::str::from_utf8(final_body.as_bytes()).unwrap(),
        WRITERS.to_string()
    );
}

#[test]
fn a_counter_transform_is_just_a_document_write() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("alloc");
    let d = doc("next-id");
    let bump = |current: Option<&DocumentBody>| {
        let n: u64 = current
            .map(|b| std::str::from_utf8(b.as_bytes()).unwrap().parse().unwrap())
            .unwrap_or(0);
        body(format!("{}", n + 1).as_bytes())
    };
    assert_eq!(
        monitor.transform(node.clone(), d.clone(), bump).unwrap(),
        body(b"1")
    );
    assert_eq!(
        monitor.transform(node.clone(), d.clone(), bump).unwrap(),
        body(b"2")
    );
    // The counter is an ordinary replicated document, verifiable like any other.
    let read = monitor.store().read_body(&node, &d).unwrap();
    assert_eq!(read, Some(body(b"2")));
}

#[test]
fn conditional_write_commits_when_digest_matches() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("record");
    let d = doc("record.json");
    monitor.write(node.clone(), d.clone(), body(b"v1")).unwrap();
    let expected = body(b"v1").digest();
    let outcome = monitor
        .conditional_write(node.clone(), d.clone(), body(b"v2"), expected)
        .unwrap();
    assert_eq!(outcome, WriteOutcome::Committed);
    assert!(outcome.is_committed());
    assert!(!outcome.is_rejected());
    let read = monitor.store().read_body(&node, &d).unwrap();
    assert_eq!(read, Some(body(b"v2")));
}

#[test]
fn conditional_write_rejected_on_digest_mismatch_leaves_body_unchanged() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("record");
    let d = doc("record.json");
    monitor.write(node.clone(), d.clone(), body(b"v1")).unwrap();
    let stale = body(b"other").digest();
    let outcome = monitor
        .conditional_write(node.clone(), d.clone(), body(b"v2"), stale)
        .unwrap();
    assert!(outcome.is_rejected());
    assert!(!outcome.is_committed());
    match outcome {
        WriteOutcome::Rejected { current } => assert_eq!(current, Some(body(b"v1").digest())),
        other => panic!("expected rejection, got {other:?}"),
    }
    let read = monitor.store().read_body(&node, &d).unwrap();
    assert_eq!(read, Some(body(b"v1")));
}

#[test]
fn conditional_write_rejected_when_absent() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("record");
    let d = doc("record.json");
    let expected = body(b"anything").digest();
    let outcome = monitor
        .conditional_write(node.clone(), d.clone(), body(b"v1"), expected)
        .unwrap();
    match outcome {
        WriteOutcome::Rejected { current } => assert_eq!(current, None),
        other => panic!("expected rejection, got {other:?}"),
    }
    let read = monitor.store().read_body(&node, &d).unwrap();
    assert_eq!(read, None);
}

#[test]
fn monitor_apply_transaction_commits_every_target_atomically() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    // Two documents at the same node (D62): the shape a ticket commit lowers to.
    let swaps = SwapSet::new([
        Swap::new(name("alpha"), doc("record.json"), body(b"a-record")),
        Swap::new(name("alpha"), doc("engine"), body(b"a-engine")),
    ])
    .expect("distinct targets");
    let outcome = monitor.apply_transaction(swaps).unwrap();
    assert_eq!(outcome, TransactionOutcome::Committed);
    assert!(outcome.is_committed());
    assert!(!outcome.is_rejected());
    let record = monitor
        .store()
        .read_body(name("alpha"), &doc("record.json"))
        .unwrap();
    assert_eq!(record, Some(body(b"a-record")));
    let engine = monitor
        .store()
        .read_body(name("alpha"), &doc("engine"))
        .unwrap();
    assert_eq!(engine, Some(body(b"a-engine")));
}

#[test]
fn monitor_apply_transaction_rejects_the_whole_set_on_a_cas_miss() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("alpha");
    // Seed the record so a stale CAS can miss; the engine doc stays absent.
    monitor
        .write(node.clone(), doc("record.json"), body(b"v1"))
        .unwrap();

    // A batch whose record swap is guarded on a stale digest must reject as a whole:
    // the unconditional engine swap alongside it must not land (no partial apply).
    let stale = body(b"other").digest();
    let swaps = SwapSet::new([
        Swap::conditional(node.clone(), doc("record.json"), body(b"v2"), stale),
        Swap::new(node.clone(), doc("engine"), body(b"e1")),
    ])
    .expect("distinct targets");
    let outcome = monitor.apply_transaction(swaps).unwrap();
    assert!(outcome.is_rejected());
    assert!(!outcome.is_committed());

    // Neither document changed: the record is still v1, the engine doc is still absent.
    let record = monitor
        .store()
        .read_body(&node, &doc("record.json"))
        .unwrap();
    assert_eq!(record, Some(body(b"v1")));
    let engine = monitor.store().read_body(&node, &doc("engine")).unwrap();
    assert_eq!(engine, None);
}

#[test]
fn failed_prepared_transaction_preserves_journal_until_reopen() {
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};

    for fail_at_end in [false, true] {
        let gate = FaultGate::closed();
        let (operation, path, when) = if fail_at_end {
            (
                FaultOp::Append,
                PathMatcher::has_component(".commit-log"),
                When::OnCall(3),
            )
        } else {
            // The namespace only: the first `.staging` rename of a transaction belongs to
            // the blob ingest in phase 2, which is before the commit point this test needs
            // to fail after. See `namespace_tree_staging`.
            (FaultOp::Rename, namespace_tree_staging(), When::Once)
        };
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
            MemFs::new(),
            FaultPolicy::new().inject_gated(operation, path, Fault::io_error(), when, gate.clone()),
        ));
        let root = PathBuf::from("failed-monitor");
        vfs.create_dir_all(&root).unwrap();
        let store = Store::init(vfs.clone(), root.clone(), test_config()).unwrap();
        let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
        let swaps = SwapSet::new([
            Swap::new(name("alpha"), doc("pin"), body(b"retained")),
            Swap::new(name("alpha"), doc("intent"), body(b"roll-forward")),
        ])
        .unwrap();
        gate.open();
        assert!(matches!(
            monitor.apply_transaction(swaps),
            Err(MonitorError::Store(_))
        ));
        // Not `Stopped`: that would claim the log was truncated, and it holds the prepare.
        assert!(matches!(
            monitor.quiesce(),
            Err(MonitorError::Store(StoreError::RecoveryRequired { .. }))
        ));
        assert!(!monitor.store().commit_log_is_empty().unwrap());
        assert!(matches!(
            monitor.write(name("alpha"), doc("later"), body(b"must-not-land")),
            Err(MonitorError::Store(StoreError::RecoveryRequired { .. }))
        ));
        drop(monitor);
        gate.close();
        let recovered = Store::open(vfs, root).unwrap();
        for (document, expected) in [
            ("pin", b"retained".as_slice()),
            ("intent", b"roll-forward".as_slice()),
        ] {
            let read = recovered.read_body(name("alpha"), &doc(document)).unwrap();
            assert_eq!(read, Some(body(expected)));
        }
        assert!(
            recovered
                .read_body(name("alpha"), &doc("later"))
                .unwrap()
                .is_none()
        );
        let recovered_monitor = WriteMonitor::spawn(recovered, counting_nonce_source()).unwrap();
        recovered_monitor.quiesce().unwrap();
        assert!(recovered_monitor.store().commit_log_is_empty().unwrap());
    }
}

#[test]
fn a_sibling_monitor_cannot_erase_another_monitors_unfinished_transaction() {
    // The journal is shared by every handle to the store, so a failure after `prepare` must
    // fence all of them: a sibling's barrier or shutdown resetting the log would throw away
    // the only record recovery has of the half-applied transaction.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, When};
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            namespace_tree_staging(),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("sibling-monitors");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).unwrap();
    let direct = store.clone();
    let failing = WriteMonitor::spawn(store.clone(), counting_nonce_source()).unwrap();
    let sibling = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();

    gate.open();
    let swaps = SwapSet::new([Swap::new(name("alpha"), doc("pin"), body(b"committed"))]).unwrap();
    assert!(matches!(
        failing.apply_transaction(swaps),
        Err(MonitorError::Store(_))
    ));
    gate.close();
    assert!(direct.recovery_required());

    // Neither a sibling's barrier nor its shutdown may reset the journal, and no handle may
    // write past the unfinished transaction.
    // A sibling must not run a snapshot over a store another handle left half-applied.
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ran_in_snapshot = Arc::clone(&ran);
    assert!(matches!(
        sibling.snapshot(move |_| ran_in_snapshot.store(true, std::sync::atomic::Ordering::SeqCst)),
        Err(MonitorError::Store(StoreError::RecoveryRequired { .. }))
    ));
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    assert!(matches!(
        sibling.quiesce(),
        Err(MonitorError::Store(StoreError::RecoveryRequired { .. }))
    ));
    assert!(matches!(
        failing.quiesce(),
        Err(MonitorError::Store(StoreError::RecoveryRequired { .. }))
    ));
    assert!(!direct.commit_log_is_empty().unwrap());
    assert!(matches!(
        direct.write_document_replicated(name("alpha"), &doc("later"), &body(b"x"), &nonce("d1")),
        Err(StoreError::RecoveryRequired { .. })
    ));
    drop(sibling);
    drop(failing);
    assert!(!direct.commit_log_is_empty().unwrap());
    drop(direct);

    let recovered = Store::open(vfs, root).unwrap();
    assert!(!recovered.recovery_required());
    assert_eq!(
        recovered.read_body(name("alpha"), &doc("pin")).unwrap(),
        Some(body(b"committed"))
    );
}

#[test]
fn unprepared_transaction_is_rolled_back_before_monitor_retry() {
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Write,
            PathMatcher::has_component(crate::store::STAGING_DIR),
            Fault::io_error(),
            // Each ingest truncates one staging temp per blob replica, so this lets the first
            // swap's blob land on every replica and fails the second swap's ingest.
            When::OnCall(crate::DEFAULT_BLOB_REPLICA_COUNT as u32 + 1),
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("retry-monitor");
    vfs.create_dir_all(&root).unwrap();
    let fs = Arc::clone(&vfs);
    let store = Store::init(vfs, root.clone(), test_config()).unwrap();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let swaps = SwapSet::new([
        Swap::new(name("alpha"), doc("pin"), body(b"retained")),
        Swap::new(name("alpha"), doc("intent"), body(b"roll-forward")),
    ])
    .unwrap();
    gate.open();
    assert!(matches!(
        monitor.apply_transaction(swaps.clone()),
        Err(MonitorError::Store(_))
    ));
    monitor.quiesce().unwrap();
    assert!(monitor.store().commit_log_is_empty().unwrap());
    // The first swap's blob was ingested before the second failed; the rollback must not
    // leave it behind once the log that named it is gone.
    fn committed_blobs(fs: &Arc<dyn Vfs>, dir: &std::path::Path) -> usize {
        fs.read_dir(dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| entry.file_name != crate::store::STAGING_DIR)
            .map(|entry| match entry.file_type {
                acadia::FileType::Dir => committed_blobs(fs, &dir.join(&entry.file_name)),
                _ => 1,
            })
            .sum()
    }
    assert_eq!(
        committed_blobs(&fs, &root.join(crate::store::BLOB_TREE_DIR)),
        0,
        "the aborted transaction's blob was left behind"
    );
    for document in ["pin", "intent"] {
        assert!(
            monitor
                .store()
                .read_body(name("alpha"), &doc(document))
                .unwrap()
                .is_none()
        );
    }
    gate.close();
    assert!(monitor.apply_transaction(swaps).unwrap().is_committed());
    for (document, expected) in [
        ("pin", b"retained".as_slice()),
        ("intent", b"roll-forward".as_slice()),
    ] {
        assert_eq!(
            monitor
                .store()
                .read_body(name("alpha"), &doc(document))
                .unwrap(),
            Some(body(expected))
        );
    }
}

#[test]
fn reads_bypass_the_monitor_and_see_the_latest_write() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let node = name("record");
    let d = doc("record.json");
    monitor.write(node.clone(), d.clone(), body(b"v1")).unwrap();
    assert_eq!(
        monitor.store().read_body(&node, &d).unwrap(),
        Some(body(b"v1"))
    );
}

#[test]
fn monitor_snapshots_never_observe_a_partial_transaction() {
    let (_vfs, store) = fresh_store();
    let monitor = Arc::new(WriteMonitor::spawn(store, counting_nonce_source()).unwrap());
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|threads| {
        threads.spawn(|| {
            barrier.wait();
            for ordinal in 0..100 {
                let value = body(ordinal.to_string().as_bytes());
                monitor
                    .apply_transaction(
                        SwapSet::new([
                            Swap::new(name("snapshot"), doc("first"), value.clone()),
                            Swap::new(name("snapshot"), doc("second"), value),
                        ])
                        .unwrap(),
                    )
                    .unwrap();
            }
        });
        barrier.wait();
        for _ in 0..100 {
            let (first, second) = monitor
                .snapshot(|store| {
                    let first = store.read_body(name("snapshot"), &doc("first")).unwrap();
                    std::thread::yield_now();
                    let second = store.read_body(name("snapshot"), &doc("second")).unwrap();
                    (first, second)
                })
                .unwrap();
            assert_eq!(first, second);
        }
    });
}

#[test]
fn many_documents_are_independent() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let alpha = name("alpha");
    let beta = name("beta");
    let d = doc("record.json");
    monitor.write(alpha.clone(), d.clone(), body(b"a")).unwrap();
    monitor.write(beta.clone(), d.clone(), body(b"b")).unwrap();
    assert_eq!(
        monitor.store().read_body(&alpha, &d).unwrap(),
        Some(body(b"a"))
    );
    assert_eq!(
        monitor.store().read_body(&beta, &d).unwrap(),
        Some(body(b"b"))
    );
}

#[test]
fn dropping_the_monitor_shuts_the_actor_down_without_hanging() {
    let (vfs, store) = fresh_store();
    let root = store.root().to_path_buf();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let swaps = SwapSet::single(Swap::new(name("record"), doc("record.json"), body(b"x")));
    monitor
        .store()
        .apply_transaction(&swaps, &nonce("direct"))
        .unwrap();
    assert!(!monitor.store().commit_log_is_empty().unwrap());

    drop(monitor);

    let reopened = Store::open(vfs, root).expect("monitor drop synchronously releases the store");
    assert!(reopened.commit_log_is_empty().unwrap());
}

#[test]
fn monitor_debug_and_errors_report_context_and_sources() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let debug = format!("{monitor:?}");
    assert!(debug.contains("WriteMonitor"));
    assert!(debug.contains("store"));

    let stopped = MonitorError::Stopped;
    assert_eq!(stopped.to_string(), "the write monitor actor has stopped");
    assert!(stopped.source().is_none());

    let store_error = MonitorError::Store(StoreError::EmptyShireId);
    assert_eq!(
        store_error.to_string(),
        "write monitor store error: a Shire identity must not be empty"
    );
    assert!(store_error.source().is_some());
}

// -- The commit feed over the funnel (LQ-105) --

/// Records every committed write the monitor reports.
#[derive(Default)]
struct Spy {
    seen: Mutex<Vec<(String, String, Vec<u8>)>>,
}

impl Spy {
    fn seen(&self) -> Vec<(String, String, Vec<u8>)> {
        self.seen.lock().expect("spy mutex").clone()
    }
}

impl CommitObserver for Spy {
    fn committed(&self, node: &NodePath, document: &DocumentName, body: &DocumentBody) {
        self.seen.lock().expect("spy mutex").push((
            node.to_string(),
            document.as_str().to_string(),
            body.as_bytes().to_vec(),
        ));
    }
}

fn observed_monitor(store: Store) -> (WriteMonitor, Arc<Spy>) {
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let spy = Arc::new(Spy::default());
    monitor.observe(spy.clone());
    (monitor, spy)
}

#[test]
fn a_transform_reports_the_body_it_committed() {
    let (_vfs, store) = fresh_store();
    let (monitor, spy) = observed_monitor(store);
    monitor
        .write(name("record"), doc("record.json"), body(b"hello"))
        .unwrap();
    assert_eq!(
        spy.seen(),
        vec![(
            "RECORD".to_string(),
            "RECORD.JSON".to_string(),
            b"hello".to_vec()
        )]
    );
}

#[test]
fn a_committed_compare_and_set_reports_and_a_rejected_one_does_not() {
    let (_vfs, store) = fresh_store();
    let (monitor, spy) = observed_monitor(store);
    let node = name("record");
    let d = doc("record.json");
    monitor.write(node.clone(), d.clone(), body(b"v1")).unwrap();
    let digest = body(b"v1").digest();
    monitor
        .conditional_write(node.clone(), d.clone(), body(b"v2"), digest)
        .unwrap();

    // The same expected digest is now stale, so this one writes nothing.
    let outcome = monitor
        .conditional_write(node.clone(), d.clone(), body(b"v3"), digest)
        .unwrap();
    assert!(outcome.is_rejected());

    let bodies: Vec<Vec<u8>> = spy.seen().into_iter().map(|(_, _, body)| body).collect();
    assert_eq!(
        bodies,
        vec![b"v1".to_vec(), b"v2".to_vec()],
        "a rejected CAS wrote nothing, so reporting it would invent a change"
    );
}

#[test]
fn a_committed_transaction_reports_every_swap_it_wrote() {
    let (_vfs, store) = fresh_store();
    let (monitor, spy) = observed_monitor(store);
    let swaps = SwapSet::new([
        Swap::new(name("A"), doc("one.json"), body(b"1")),
        Swap::new(name("B"), doc("two.json"), body(b"2")),
    ])
    .unwrap();
    assert!(monitor.apply_transaction(swaps).unwrap().is_committed());

    let targets: Vec<(String, String)> = spy
        .seen()
        .into_iter()
        .map(|(node, document, _)| (node, document))
        .collect();
    assert_eq!(
        targets,
        vec![
            ("A".to_string(), "ONE.JSON".to_string()),
            ("B".to_string(), "TWO.JSON".to_string())
        ],
        "the transactional path is a write shape too; a feed that missed it would miss \
         every D102 handle commit"
    );
}

#[test]
fn a_rejected_transaction_reports_nothing_at_all() {
    let (_vfs, store) = fresh_store();
    let (monitor, spy) = observed_monitor(store);
    // The first swap is fine; the second's CAS cannot hold against an absent document, and
    // a transaction is all-or-nothing (D53), so neither is written.
    let swaps = SwapSet::new([
        Swap::new(name("A"), doc("one.json"), body(b"1")),
        Swap::conditional(
            name("B"),
            doc("two.json"),
            body(b"2"),
            body(b"stale").digest(),
        ),
    ])
    .unwrap();
    assert!(monitor.apply_transaction(swaps).unwrap().is_rejected());
    assert!(
        spy.seen().is_empty(),
        "reporting the swap that would have committed would tell an observer about a \
         state the store never held"
    );
}

#[test]
fn an_observer_installed_late_hears_only_what_follows() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    monitor
        .write(name("record"), doc("record.json"), body(b"before"))
        .unwrap();
    let spy = Arc::new(Spy::default());
    monitor.observe(spy.clone());
    monitor
        .write(name("record"), doc("record.json"), body(b"after"))
        .unwrap();
    let bodies: Vec<Vec<u8>> = spy.seen().into_iter().map(|(_, _, body)| body).collect();
    assert_eq!(
        bodies,
        vec![b"after".to_vec()],
        "the feed is not a log to replay; an observer that needs a starting point reads \
         the store for one"
    );
}

#[test]
fn the_observer_runs_on_the_single_writer_thread() {
    // Not incidental: the whole reason the trait forbids blocking is that this is where it
    // runs. If the call ever moved off the actor, the contract would be quietly overstated
    // and a sink could start waiting on a reader without anyone noticing.
    #[derive(Default)]
    struct ThreadSpy(Mutex<Option<String>>);
    impl CommitObserver for ThreadSpy {
        fn committed(&self, _: &NodePath, _: &DocumentName, _: &DocumentBody) {
            *self.0.lock().expect("thread spy mutex") =
                std::thread::current().name().map(str::to_string);
        }
    }

    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let spy = Arc::new(ThreadSpy::default());
    monitor.observe(spy.clone());
    monitor
        .write(name("record"), doc("record.json"), body(b"x"))
        .unwrap();
    assert_eq!(
        spy.0.lock().expect("thread spy mutex").as_deref(),
        Some("canopy-write-monitor")
    );
}

// -- Plan and commit on the actor ---------------------------------------------------------

/// Reads a one-byte counter document and plans the swap that increments it.
fn increment_plan(store: &Store) -> (u8, Option<SwapSet>) {
    let current = store
        .read_body(name("counter"), &doc("n"))
        .unwrap()
        .map(|b| b.as_bytes()[0])
        .unwrap_or(0);
    let swap = Swap::new(name("counter"), doc("n"), body(&[current + 1]));
    (current, Some(SwapSet::new([swap]).unwrap()))
}

#[test]
fn plan_and_apply_commits_what_the_plan_returns_and_hands_back_its_value() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let (value, outcome) = monitor.plan_and_apply(increment_plan).unwrap();
    assert_eq!(value, 0);
    assert_eq!(outcome, Some(TransactionOutcome::Committed));
    let read = monitor
        .store()
        .read_body(name("counter"), &doc("n"))
        .unwrap();
    assert_eq!(read, Some(body(&[1])));
}

#[test]
fn plan_and_apply_with_no_swaps_writes_nothing_and_reports_no_outcome() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    let (value, outcome) = monitor.plan_and_apply(|_| ("nothing", None)).unwrap();
    assert_eq!(value, "nothing");
    assert_eq!(outcome, None);
    let read = monitor
        .store()
        .read_body(name("counter"), &doc("n"))
        .unwrap();
    assert_eq!(read, None);
}

#[test]
fn plan_and_apply_sees_the_state_the_previous_write_left() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    for expected in 0..10u8 {
        let (value, outcome) = monitor.plan_and_apply(increment_plan).unwrap();
        assert_eq!(value, expected);
        assert!(outcome.unwrap().is_committed());
    }
}

#[test]
fn concurrent_plan_and_apply_calls_never_lose_a_round() {
    // The property a plan made outside the actor lacks: every call plans from the state the
    // previous call left, so no call is rejected and none overwrites another.
    let (_vfs, store) = fresh_store();
    let monitor = Arc::new(WriteMonitor::spawn(store, counting_nonce_source()).unwrap());
    let (threads, each) = (6u8, 20u8);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let monitor = Arc::clone(&monitor);
            scope.spawn(move || {
                for _ in 0..each {
                    let (_, outcome) = monitor.plan_and_apply(increment_plan).unwrap();
                    assert_eq!(outcome, Some(TransactionOutcome::Committed));
                }
            });
        }
    });
    let read = monitor
        .store()
        .read_body(name("counter"), &doc("n"))
        .unwrap();
    assert_eq!(read, Some(body(&[threads * each])));
}

#[test]
fn a_plan_guarded_on_a_digest_a_bypassing_write_changed_is_rejected_whole() {
    let (_vfs, store) = fresh_store();
    let monitor = WriteMonitor::spawn(store.clone(), counting_nonce_source()).unwrap();
    monitor
        .write(name("counter"), doc("n"), body(&[1]))
        .unwrap();
    let (_, outcome) = monitor
        .plan_and_apply(|_| {
            let stale = body(b"not what is stored").digest();
            let swaps = SwapSet::new([
                Swap::conditional(name("counter"), doc("n"), body(&[9]), stale),
                Swap::new(name("counter"), doc("other"), body(b"x")),
            ])
            .unwrap();
            ((), Some(swaps))
        })
        .unwrap();
    assert!(outcome.unwrap().is_rejected());
    let other = monitor
        .store()
        .read_body(name("counter"), &doc("other"))
        .unwrap();
    assert_eq!(other, None, "no partial apply");
}

#[test]
fn plan_and_apply_reports_what_it_committed_to_the_observer() {
    let (_vfs, store) = fresh_store();
    let (monitor, spy) = observed_monitor(store);
    monitor.plan_and_apply(increment_plan).unwrap();
    assert_eq!(
        spy.seen(),
        vec![("COUNTER".to_string(), "N".to_string(), vec![1])]
    );
}
