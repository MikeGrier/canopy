// Copyright (c) Michael Grier.

//! The public write verbs on `Store`: read-modify-write without lost updates, compare-and-set,
//! all-or-nothing transactions, planned commits, fencing after an ambiguous failure, and the
//! commit feed.

use super::*;

use std::sync::Mutex;

use crate::{CommitObserver, StoreView};
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};

fn number(current: Option<&DocumentBody>) -> u64 {
    current
        .map(|b| std::str::from_utf8(b.as_bytes()).unwrap().parse().unwrap())
        .unwrap_or(0)
}

fn bump(current: Option<&DocumentBody>) -> DocumentBody {
    body(format!("{}", number(current) + 1).as_bytes())
}

/// A store on `vfs` at `root`, drawing from a counting source.
fn sourced_store_on(vfs: Arc<dyn Vfs>, root: &str) -> Store {
    let root = PathBuf::from(root);
    vfs.create_dir_all(&root).unwrap();
    let options = StoreOptions::with_nonce_source(counting_nonce_source());
    Store::init(vfs, root, test_config(), options).unwrap()
}

// -- Read-modify-write --------------------------------------------------------------------

#[test]
fn transform_sees_none_and_creates_when_absent() {
    let (_vfs, store) = sourced_store();
    let (node, d) = (name("record"), doc("record.json"));
    let (out, commit) = store
        .transform(node.clone(), &d, |current| {
            if current.is_none() {
                body(b"created")
            } else {
                body(b"existed")
            }
        })
        .unwrap();
    assert!(commit.is_committed());
    assert_eq!(out, body(b"created"));
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"created")));
}

#[test]
fn transform_reads_and_updates_the_current_body() {
    let (_vfs, store) = sourced_store();
    let (node, d) = (name("record"), doc("record.json"));
    store
        .update_document(Swap::new(node.clone(), d.clone(), body(b"v1")))
        .unwrap();
    let (out, _) = store
        .transform(node.clone(), &d, |current| {
            let mut bytes = current.map(|b| b.as_bytes().to_vec()).unwrap_or_default();
            bytes.extend_from_slice(b"+v2");
            body(&bytes)
        })
        .unwrap();
    assert_eq!(out, body(b"v1+v2"));
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v1+v2")));
}

#[test]
fn an_unconditional_update_overwrites_the_previous_body() {
    let (_vfs, store) = sourced_store();
    let (node, d) = (name("record"), doc("record.json"));
    for value in [b"v1", b"v2"] {
        store
            .update_document(Swap::new(node.clone(), d.clone(), body(value)))
            .unwrap();
    }
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));
}

#[test]
fn a_write_leaves_no_staging_orphan() {
    let (vfs, store) = sourced_store();
    store
        .update_document(Swap::new(name("record"), doc("record.json"), body(b"x")))
        .unwrap();
    store
        .write_document_replicated(name("record"), &doc("record.json"), &body(b"y"))
        .unwrap();
    assert_eq!(
        node_entries(&vfs, &store, 0),
        vec![OsString::from("RECORD.JSON")]
    );
}

#[test]
fn transforms_from_many_threads_have_no_lost_update() {
    let (_vfs, store) = sourced_store();
    let (node, d) = (name("counter"), doc("count"));
    const WRITERS: u64 = 20;
    thread::scope(|scope| {
        for _ in 0..WRITERS {
            let (store, node, d) = (store.clone(), node.clone(), d.clone());
            scope.spawn(move || {
                let (_, commit) = store.transform(node, &d, bump).unwrap();
                assert!(commit.is_committed());
            });
        }
    });
    assert_eq!(
        number(store.read_body(&node, &d).unwrap().as_ref()),
        WRITERS
    );
}

#[test]
fn a_counter_transform_is_just_a_document_write() {
    let (_vfs, store) = sourced_store();
    let (node, d) = (name("alloc"), doc("next-id"));
    assert_eq!(
        store.transform(node.clone(), &d, bump).unwrap().0,
        body(b"1")
    );
    assert_eq!(
        store.transform(node.clone(), &d, bump).unwrap().0,
        body(b"2")
    );
    // The counter is an ordinary replicated document, verifiable like any other.
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"2")));
}

#[test]
fn many_documents_are_independent() {
    let (_vfs, store) = sourced_store();
    let d = doc("record.json");
    store
        .write_document_replicated(name("alpha"), &d, &body(b"a"))
        .unwrap();
    store
        .write_document_replicated(name("beta"), &d, &body(b"b"))
        .unwrap();
    assert_eq!(
        store.read_body(name("alpha"), &d).unwrap(),
        Some(body(b"a"))
    );
    assert_eq!(store.read_body(name("beta"), &d).unwrap(), Some(body(b"b")));
}

// -- Compare-and-set ----------------------------------------------------------------------

#[test]
fn compare_and_set_commits_when_the_digest_matches() {
    let (_vfs, store) = sourced_store();
    let (node, d) = (name("record"), doc("record.json"));
    store
        .update_document(Swap::new(node.clone(), d.clone(), body(b"v1")))
        .unwrap();
    let commit = store
        .update_document(Swap::conditional(
            node.clone(),
            d.clone(),
            body(b"v2"),
            body(b"v1").digest(),
        ))
        .unwrap();
    assert_eq!(commit.outcome(), &TransactionOutcome::Committed);
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));
}

#[test]
fn compare_and_set_on_a_mismatch_reports_the_current_digest_and_writes_nothing() {
    let (_vfs, store) = sourced_store();
    let (node, d) = (name("record"), doc("record.json"));
    store
        .update_document(Swap::new(node.clone(), d.clone(), body(b"v1")))
        .unwrap();
    let commit = store
        .update_document(Swap::conditional(
            node.clone(),
            d.clone(),
            body(b"v2"),
            body(b"other").digest(),
        ))
        .unwrap();
    match commit.outcome() {
        TransactionOutcome::Rejected { current, .. } => {
            assert_eq!(*current, Some(body(b"v1").digest()))
        }
        other => panic!("expected rejection, got {other:?}"),
    }
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v1")));
}

#[test]
fn compare_and_set_against_an_absent_document_is_rejected() {
    let (_vfs, store) = sourced_store();
    let (node, d) = (name("record"), doc("record.json"));
    let commit = store
        .update_document(Swap::conditional(
            node.clone(),
            d.clone(),
            body(b"v1"),
            body(b"anything").digest(),
        ))
        .unwrap();
    match commit.outcome() {
        TransactionOutcome::Rejected { current, .. } => assert_eq!(*current, None),
        other => panic!("expected rejection, got {other:?}"),
    }
    assert_eq!(store.read_body(&node, &d).unwrap(), None);
}

// -- Transactions -------------------------------------------------------------------------

#[test]
fn a_transaction_commits_every_target_atomically() {
    let (_vfs, store) = sourced_store();
    // Two documents at the same node (D62): the shape a ticket commit lowers to.
    let swaps = SwapSet::new([
        Swap::new(name("alpha"), doc("record.json"), body(b"a-record")),
        Swap::new(name("alpha"), doc("engine"), body(b"a-engine")),
    ])
    .unwrap();
    assert!(store.apply_transaction(&swaps).unwrap().is_committed());
    assert_eq!(
        store.read_body(name("alpha"), &doc("record.json")).unwrap(),
        Some(body(b"a-record"))
    );
    assert_eq!(
        store.read_body(name("alpha"), &doc("engine")).unwrap(),
        Some(body(b"a-engine"))
    );
}

#[test]
fn a_transaction_rejects_the_whole_set_on_a_compare_and_set_miss() {
    let (_vfs, store) = sourced_store();
    let node = name("alpha");
    store
        .update_document(Swap::new(node.clone(), doc("record.json"), body(b"v1")))
        .unwrap();
    let swaps = SwapSet::new([
        Swap::conditional(
            node.clone(),
            doc("record.json"),
            body(b"v2"),
            body(b"other").digest(),
        ),
        Swap::new(node.clone(), doc("engine"), body(b"e1")),
    ])
    .unwrap();
    assert!(store.apply_transaction(&swaps).unwrap().is_rejected());
    assert_eq!(
        store.read_body(&node, &doc("record.json")).unwrap(),
        Some(body(b"v1"))
    );
    assert_eq!(store.read_body(&node, &doc("engine")).unwrap(), None);
}

#[test]
fn a_transaction_failing_after_its_commit_point_keeps_the_journal_until_reopen() {
    for fail_at_end in [false, true] {
        let gate = FaultGate::closed();
        let (operation, path, when) = if fail_at_end {
            // The `end` append: `begin` and `prepare` are the first two.
            (
                FaultOp::Append,
                PathMatcher::has_component(".commit-log"),
                When::OnCall(3),
            )
        } else {
            // The namespace only: the blob ingest's staging renames precede the commit point.
            (FaultOp::Rename, namespace_tree_staging(), When::Once)
        };
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
            MemFs::new(),
            FaultPolicy::new().inject_gated(operation, path, Fault::io_error(), when, gate.clone()),
        ));
        let store = sourced_store_on(Arc::clone(&vfs), "failed-commit");
        let root = store.root().to_path_buf();
        let swaps = SwapSet::new([
            Swap::new(name("alpha"), doc("pin"), body(b"retained")),
            Swap::new(name("alpha"), doc("intent"), body(b"roll-forward")),
        ])
        .unwrap();
        gate.open();
        assert!(store.apply_transaction(&swaps).is_err());
        assert!(matches!(
            store.quiesce(),
            Err(StoreError::RecoveryRequired { .. })
        ));
        assert!(!store.commit_log_is_empty().unwrap());
        assert!(matches!(
            store.write_document_replicated(name("alpha"), &doc("later"), &body(b"x")),
            Err(StoreError::RecoveryRequired { .. })
        ));
        drop(store);
        gate.close();

        let recovered = Store::open(
            vfs,
            root,
            StoreOptions::with_nonce_source(counting_nonce_source()),
        )
        .unwrap();
        for (document, expected) in [
            ("pin", b"retained".as_slice()),
            ("intent", b"roll-forward".as_slice()),
        ] {
            assert_eq!(
                recovered.read_body(name("alpha"), &doc(document)).unwrap(),
                Some(body(expected))
            );
        }
        assert!(
            recovered
                .read_body(name("alpha"), &doc("later"))
                .unwrap()
                .is_none()
        );
        recovered.quiesce().unwrap();
        assert!(recovered.commit_log_is_empty().unwrap());
    }
}

#[test]
fn no_clone_can_erase_or_write_past_another_clones_unfinished_transaction() {
    // The journal is shared by every handle to the store, so a failure after `prepare` must
    // fence all of them: a sibling resetting the log would throw away the only record
    // recovery has of the half-applied transaction.
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
    let failing = sourced_store_on(Arc::clone(&vfs), "sibling-handles");
    let root = failing.root().to_path_buf();
    let sibling = failing.clone();

    gate.open();
    let swaps = SwapSet::single(Swap::new(name("alpha"), doc("pin"), body(b"committed")));
    assert!(failing.apply_transaction(&swaps).is_err());
    gate.close();
    assert!(sibling.recovery_required());

    let mut ran = false;
    assert!(matches!(
        sibling.snapshot(|_| ran = true),
        Err(StoreError::RecoveryRequired { .. })
    ));
    assert!(
        !ran,
        "a snapshot never reads a store another handle left half-applied"
    );
    assert!(matches!(
        sibling.quiesce(),
        Err(StoreError::RecoveryRequired { .. })
    ));
    assert!(matches!(
        sibling.write_document_replicated(name("alpha"), &doc("later"), &body(b"x")),
        Err(StoreError::RecoveryRequired { .. })
    ));
    assert!(!sibling.commit_log_is_empty().unwrap());
    drop(sibling);
    drop(failing);

    let recovered = Store::open(vfs, root, StoreOptions::without_nonce_source()).unwrap();
    assert!(!recovered.recovery_required());
    assert_eq!(
        recovered.read_body(name("alpha"), &doc("pin")).unwrap(),
        Some(body(b"committed"))
    );
}

#[test]
fn a_transaction_failing_before_its_commit_point_is_rolled_back_and_can_be_retried() {
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
    let store = sourced_store_on(Arc::clone(&vfs), "retry");
    let swaps = SwapSet::new([
        Swap::new(name("alpha"), doc("pin"), body(b"retained")),
        Swap::new(name("alpha"), doc("intent"), body(b"roll-forward")),
    ])
    .unwrap();
    gate.open();
    assert!(store.apply_transaction(&swaps).is_err());
    assert!(!store.recovery_required());
    assert!(store.commit_log_is_empty().unwrap());
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
        committed_blobs(&vfs, &store.root().join(crate::store::BLOB_TREE_DIR)),
        0,
        "the aborted transaction's blob was left behind"
    );
    gate.close();
    assert!(store.apply_transaction(&swaps).unwrap().is_committed());
    for (document, expected) in [
        ("pin", b"retained".as_slice()),
        ("intent", b"roll-forward".as_slice()),
    ] {
        assert_eq!(
            store.read_body(name("alpha"), &doc(document)).unwrap(),
            Some(body(expected))
        );
    }
}

#[test]
fn snapshots_never_observe_a_partial_transaction() {
    let (_vfs, store) = sourced_store();
    let barrier = std::sync::Barrier::new(2);
    thread::scope(|threads| {
        let writer = store.clone();
        let barrier = &barrier;
        threads.spawn(move || {
            barrier.wait();
            for ordinal in 0..100 {
                let value = body(ordinal.to_string().as_bytes());
                writer
                    .apply_transaction(
                        &SwapSet::new([
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
            match store.snapshot(|view| {
                let first = view.body(name("snapshot"), &doc("first")).unwrap();
                thread::yield_now();
                let second = view.body(name("snapshot"), &doc("second")).unwrap();
                (first, second)
            }) {
                Ok((first, second)) => assert_eq!(first, second),
                // A writer that never pauses may outlast every attempt; that is reported,
                // never answered with a torn pair.
                Err(StoreError::SnapshotContended { .. }) => {}
                Err(other) => panic!("{other}"),
            }
        }
    });
}

#[test]
fn the_new_errors_say_what_happened() {
    assert_eq!(
        StoreError::NoNonceSource.to_string(),
        "the store was opened without a nonce source, so it cannot mint one"
    );
    assert_eq!(
        StoreError::SnapshotContended { attempts: 3 }.to_string(),
        "a snapshot was disturbed by a concurrent modification on all 3 attempts"
    );
    assert_eq!(
        StoreError::NonceCollision { nonce: nonce("n1") }.to_string(),
        "nonce N1 is already claimed"
    );
}

// -- The commit feed ----------------------------------------------------------------------

/// Records every committed write the store reports.
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

fn observed_store() -> (Store, Arc<Spy>) {
    let (_vfs, store) = sourced_store();
    let spy = Arc::new(Spy::default());
    store.observe(spy.clone());
    (store, spy)
}

#[test]
fn a_transform_reports_the_body_it_committed() {
    let (store, spy) = observed_store();
    store
        .transform(name("record"), &doc("record.json"), |_| body(b"hello"))
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
    let (store, spy) = observed_store();
    let (node, d) = (name("record"), doc("record.json"));
    store
        .update_document(Swap::new(node.clone(), d.clone(), body(b"v1")))
        .unwrap();
    let digest = body(b"v1").digest();
    store
        .update_document(Swap::conditional(
            node.clone(),
            d.clone(),
            body(b"v2"),
            digest,
        ))
        .unwrap();
    // The same expected digest is now stale, so this one writes nothing.
    let commit = store
        .update_document(Swap::conditional(
            node.clone(),
            d.clone(),
            body(b"v3"),
            digest,
        ))
        .unwrap();
    assert!(commit.is_rejected());
    let bodies: Vec<Vec<u8>> = spy.seen().into_iter().map(|(_, _, body)| body).collect();
    assert_eq!(
        bodies,
        vec![b"v1".to_vec(), b"v2".to_vec()],
        "a rejected CAS wrote nothing, so reporting it would invent a change"
    );
}

#[test]
fn a_committed_transaction_reports_every_swap_it_wrote() {
    let (store, spy) = observed_store();
    let swaps = SwapSet::new([
        Swap::new(name("A"), doc("one.json"), body(b"1")),
        Swap::new(name("B"), doc("two.json"), body(b"2")),
    ])
    .unwrap();
    assert!(store.apply_transaction(&swaps).unwrap().is_committed());
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
        ]
    );
}

#[test]
fn a_rejected_transaction_reports_nothing_at_all() {
    let (store, spy) = observed_store();
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
    assert!(store.apply_transaction(&swaps).unwrap().is_rejected());
    assert!(
        spy.seen().is_empty(),
        "reporting the swap that would have committed would tell an observer about a \
         state the store never held"
    );
}

#[test]
fn an_observer_installed_late_hears_only_what_follows() {
    let (_vfs, store) = sourced_store();
    store
        .write_document_replicated(name("record"), &doc("record.json"), &body(b"before"))
        .unwrap();
    let spy = Arc::new(Spy::default());
    store.observe(spy.clone());
    store
        .write_document_replicated(name("record"), &doc("record.json"), &body(b"after"))
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
fn the_observer_runs_on_the_writing_thread() {
    // Not incidental: the reason the trait forbids blocking is that it runs inside the write.
    #[derive(Default)]
    struct ThreadSpy(Mutex<Option<String>>);
    impl CommitObserver for ThreadSpy {
        fn committed(&self, _: &NodePath, _: &DocumentName, _: &DocumentBody) {
            *self.0.lock().expect("thread spy mutex") =
                thread::current().name().map(str::to_string);
        }
    }

    let (_vfs, store) = sourced_store();
    let spy = Arc::new(ThreadSpy::default());
    store.observe(spy.clone());
    let writer = store.clone();
    thread::Builder::new()
        .name("the-writer".to_string())
        .spawn(move || {
            writer
                .write_document_replicated(name("record"), &doc("record.json"), &body(b"x"))
                .unwrap();
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(
        spy.0.lock().expect("thread spy mutex").as_deref(),
        Some("the-writer")
    );
}

// -- Plan and apply -----------------------------------------------------------------------

/// Reads a one-byte counter document and plans the swap that increments it.
fn increment_plan(view: &StoreView<'_>) -> (u8, Option<SwapSet>) {
    let current = view
        .body(name("counter"), &doc("n"))
        .unwrap()
        .map(|b| b.as_bytes()[0])
        .unwrap_or(0);
    let swap = Swap::new(name("counter"), doc("n"), body(&[current + 1]));
    (current, Some(SwapSet::single(swap)))
}

#[test]
fn plan_and_apply_commits_what_the_plan_returns_and_hands_back_its_value() {
    let (_vfs, store) = sourced_store();
    let (value, commit) = store.plan_and_apply(increment_plan).unwrap();
    assert_eq!(value, 0);
    assert!(commit.unwrap().is_committed());
    assert_eq!(
        store.read_body(name("counter"), &doc("n")).unwrap(),
        Some(body(&[1]))
    );
}

#[test]
fn plan_and_apply_with_no_swaps_writes_nothing_and_reports_no_commit() {
    let (_vfs, store) = sourced_store();
    let (value, commit) = store.plan_and_apply(|_| ("nothing", None)).unwrap();
    assert_eq!(value, "nothing");
    assert!(commit.is_none());
    assert_eq!(store.read_body(name("counter"), &doc("n")).unwrap(), None);
}

#[test]
fn plan_and_apply_sees_the_state_the_previous_write_left() {
    let (_vfs, store) = sourced_store();
    for expected in 0..10u8 {
        let (value, commit) = store.plan_and_apply(increment_plan).unwrap();
        assert_eq!(value, expected);
        assert!(commit.unwrap().is_committed());
    }
}

#[test]
fn concurrent_plan_and_apply_calls_never_lose_a_round() {
    // Every call plans from the state the previous call left, so no call is rejected and none
    // overwrites another.
    let (_vfs, store) = sourced_store();
    let (threads, each) = (6u8, 20u8);
    thread::scope(|scope| {
        for _ in 0..threads {
            let store = store.clone();
            scope.spawn(move || {
                for _ in 0..each {
                    let (_, commit) = store.plan_and_apply(increment_plan).unwrap();
                    assert!(commit.unwrap().is_committed());
                }
            });
        }
    });
    assert_eq!(
        store.read_body(name("counter"), &doc("n")).unwrap(),
        Some(body(&[threads * each]))
    );
}

#[test]
fn a_plan_guarded_on_a_stale_digest_is_rejected_whole() {
    let (_vfs, store) = sourced_store();
    store
        .update_document(Swap::new(name("counter"), doc("n"), body(&[1])))
        .unwrap();
    let (_, commit) = store
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
    assert!(commit.unwrap().is_rejected());
    assert_eq!(
        store.read_body(name("counter"), &doc("other")).unwrap(),
        None,
        "no partial apply"
    );
}

#[test]
fn plan_and_apply_reports_what_it_committed_to_the_observer() {
    let (store, spy) = observed_store();
    store.plan_and_apply(increment_plan).unwrap();
    assert_eq!(
        spy.seen(),
        vec![("COUNTER".to_string(), "N".to_string(), vec![1])]
    );
}
