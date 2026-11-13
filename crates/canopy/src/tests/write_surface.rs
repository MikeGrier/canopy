// Copyright (c) Michael Grier.

//! The write surface on `Store`: verbs that draw their nonces from the source the store was
//! opened with, commit through the log, report to the commit feed, and lend callbacks a view
//! that only reads. Nonces come from the seeded generator in `nonce_source`, one stream per
//! actor, so every run draws the same nonces; faults are injected in a fixed order.

use std::sync::Mutex;
use std::sync::OnceLock;

use super::nonce_source::{TEST_SEED, scripted_source, seeded_nonces, seeded_source};
use super::*;
use crate::store::{BLOB_TREE_DIR, QUARANTINE_DIR, STAGING_DIR};
use crate::{Commit, CommitObserver, SNAPSHOT_ATTEMPTS, StoreOptions};
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};

/// A store on `vfs` at a fresh root, drawing actor 0's stream under [`TEST_SEED`].
fn seeded_store_on(vfs: Arc<dyn Vfs>, root: &str) -> Store {
    let root = PathBuf::from(root);
    vfs.create_dir_all(&root).unwrap();
    let options = StoreOptions::with_nonce_source(seeded_source(TEST_SEED, 0));
    Store::init(vfs, root, test_config(), options).unwrap()
}

fn seeded_store() -> (Arc<dyn Vfs>, Store) {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let store = seeded_store_on(Arc::clone(&vfs), "shire");
    (vfs, store)
}

/// A store whose source yields exactly `nonces`.
fn scripted_store(nonces: Vec<Nonce>) -> (Arc<dyn Vfs>, Store) {
    let (vfs, root) = fresh_root();
    let options = StoreOptions::with_nonce_source(scripted_source(nonces));
    let store = Store::init(Arc::clone(&vfs), root, test_config(), options).unwrap();
    (vfs, store)
}

/// Drops `store` and opens its root again with `options`.
fn reopen_with(vfs: &Arc<dyn Vfs>, store: Store, options: StoreOptions) -> Store {
    let root = store.root().to_path_buf();
    drop(store);
    Store::open(Arc::clone(vfs), root, options).unwrap()
}

fn text(store: &Store, node: &str, document: &str) -> Option<String> {
    store
        .read_body(name(node), &doc(document))
        .unwrap()
        .map(|body| String::from_utf8(body.as_bytes().to_vec()).unwrap())
}

fn increment(current: Option<&DocumentBody>) -> DocumentBody {
    let value: u64 = current
        .map(|body| {
            std::str::from_utf8(body.as_bytes())
                .unwrap()
                .parse()
                .unwrap()
        })
        .unwrap_or(0);
    body((value + 1).to_string().as_bytes())
}

fn claim_path(store: &Store, nonce: &Nonce) -> PathBuf {
    store
        .root()
        .join(crate::store::CLAIMS_DIR)
        .join(nonce.path_component().as_ref())
}

fn blob_staging_path(store: &Store, index: usize, nonce: &Nonce) -> PathBuf {
    store
        .root()
        .join(BLOB_TREE_DIR)
        .join(format!("p{index}"))
        .join(STAGING_DIR)
        .join(format!("blob.tmp.{}", nonce.as_str()))
}

/// Records every report the commit feed makes, in order.
#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl CommitObserver for Recorder {
    fn committed(&self, node: &NodePath, document: &DocumentName, body: &DocumentBody) {
        let text = String::from_utf8_lossy(body.as_bytes()).into_owned();
        self.0
            .lock()
            .unwrap()
            .push(format!("write {node}/{} = {text}", document.as_str()));
    }

    fn removed(&self, node: &NodePath, document: &DocumentName) {
        self.0
            .lock()
            .unwrap()
            .push(format!("remove {node}/{}", document.as_str()));
    }
}

impl Recorder {
    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

// -- The nonce source ----------------------------------------------------------

#[test]
fn a_store_opened_without_a_source_refuses_every_minting_verb() {
    let (_vfs, store) = fresh_store();
    let swap = || Swap::new(name("alpha"), doc("one"), body(b"x"));
    assert!(matches!(
        store.update_document(swap()),
        Err(StoreError::NoNonceSource)
    ));
    assert!(matches!(
        store.apply_transaction(&SwapSet::single(swap())),
        Err(StoreError::NoNonceSource)
    ));
    assert!(matches!(
        store.write_document_replicated(name("alpha"), &doc("one"), &body(b"x")),
        Err(StoreError::NoNonceSource)
    ));
    assert!(matches!(
        store.transform(name("alpha"), &doc("one"), increment),
        Err(StoreError::NoNonceSource)
    ));
    assert_eq!(text(&store, "alpha", "one"), None, "nothing was written");
    assert!(store.snapshot(|_| ()).is_ok(), "reading needs no nonce");
}

#[test]
fn each_operation_draws_one_nonce_and_reports_it() {
    let (_vfs, store) = seeded_store();
    let expected = seeded_nonces(TEST_SEED, 0, 3);

    let first = store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"1")))
        .unwrap();
    let second = store
        .write_document_replicated(name("alpha"), &doc("two"), &body(b"2"))
        .unwrap();
    let (_, third) = store
        .transform(name("alpha"), &doc("one"), increment)
        .unwrap();

    assert_eq!(first.nonce(), &expected[0]);
    assert_eq!(second, expected[1]);
    assert_eq!(third.nonce(), &expected[2]);
}

#[test]
fn a_rejected_write_still_reports_the_nonce_it_drew() {
    let (_vfs, store) = seeded_store();
    let expected = seeded_nonces(TEST_SEED, 0, 1);
    let commit = store
        .update_document(Swap::conditional_absent(
            name("alpha"),
            doc("one"),
            body(b"x"),
        ))
        .unwrap();
    assert!(commit.is_committed());
    let rejected = store
        .update_document(Swap::conditional_absent(
            name("alpha"),
            doc("one"),
            body(b"y"),
        ))
        .unwrap();
    assert!(rejected.is_rejected());
    assert_eq!(commit.nonce(), &expected[0]);
    assert_ne!(rejected.nonce(), commit.nonce());
}

#[test]
fn a_reopened_store_draws_from_the_source_it_was_reopened_with() {
    let (vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"1")))
        .unwrap();
    let store = reopen_with(
        &vfs,
        store,
        StoreOptions::with_nonce_source(scripted_source(vec![nonce("after")])),
    );
    let commit = store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"2")))
        .unwrap();
    assert_eq!(commit.nonce(), &nonce("after"));
}

// -- Compare-and-set and read-modify-write ------------------------------------

#[test]
fn a_compare_and_set_against_a_stale_digest_is_rejected_and_writes_nothing() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"current")))
        .unwrap();
    let commit = store
        .update_document(Swap::conditional(
            name("alpha"),
            doc("one"),
            body(b"lost"),
            body(b"stale").digest(),
        ))
        .unwrap();
    match commit.outcome() {
        TransactionOutcome::Rejected { current, .. } => {
            assert_eq!(*current, Some(body(b"current").digest()));
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("current"));
}

#[test]
fn a_compare_and_set_against_the_current_digest_commits() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"current")))
        .unwrap();
    let commit = store
        .update_document(Swap::conditional(
            name("alpha"),
            doc("one"),
            body(b"next"),
            body(b"current").digest(),
        ))
        .unwrap();
    assert!(commit.is_committed());
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("next"));
}

#[test]
fn a_transform_sees_absence_and_then_each_value_it_wrote() {
    let (_vfs, store) = seeded_store();
    for expected in 1..=20u64 {
        let (written, commit) = store
            .transform(name("alpha"), &doc("count"), increment)
            .unwrap();
        assert!(commit.is_committed());
        assert_eq!(written, body(expected.to_string().as_bytes()));
    }
    assert_eq!(text(&store, "alpha", "count").as_deref(), Some("20"));
}

#[test]
fn transforms_racing_through_clones_lose_no_update() {
    let (_vfs, store) = seeded_store();
    let threads = 4;
    let each = 25;
    let nonces: Vec<Nonce> = thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let store = store.clone();
                scope.spawn(move || {
                    (0..each)
                        .map(|_| {
                            let (_, commit) = store
                                .transform(name("alpha"), &doc("count"), increment)
                                .unwrap();
                            assert!(commit.is_committed(), "exclusion is held read to commit");
                            commit.nonce().clone()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    });
    assert_eq!(
        text(&store, "alpha", "count"),
        Some((threads * each).to_string())
    );
    let mut distinct = nonces.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), nonces.len(), "clones share one source");
}

#[test]
fn a_transform_whose_closure_writes_through_a_captured_clone_is_rejected() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"before")))
        .unwrap();
    let captured = store.clone();
    let (_, commit) = store
        .transform(name("alpha"), &doc("one"), |_| {
            captured
                .update_document(Swap::new(name("alpha"), doc("one"), body(b"sneaked")))
                .unwrap();
            body(b"transformed")
        })
        .unwrap();
    assert!(
        commit.is_rejected(),
        "the commit is conditional on what was read, so the sneaked write is not lost"
    );
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("sneaked"));
}

// -- Plan and apply ------------------------------------------------------------

#[test]
fn a_plan_commits_against_the_state_it_read() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"1")))
        .unwrap();
    let (seen, commit) = store
        .plan_and_apply(|view| {
            let current = view.body(name("alpha"), &doc("one")).unwrap().unwrap();
            let swaps = SwapSet::new([
                Swap::conditional(name("alpha"), doc("one"), body(b"2"), current.digest()),
                Swap::new(name("beta"), doc("copy"), current.clone()),
            ])
            .unwrap();
            (current, Some(swaps))
        })
        .unwrap();
    assert_eq!(seen, body(b"1"));
    assert!(commit.unwrap().is_committed());
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("2"));
    assert_eq!(text(&store, "beta", "copy").as_deref(), Some("1"));
}

#[test]
fn a_plan_with_nothing_to_write_draws_no_nonce() {
    let (_vfs, store) = scripted_store(vec![nonce("only")]);
    let (listed, commit) = store
        .plan_and_apply(|view| (view.list_documents().unwrap().len(), None))
        .unwrap();
    assert_eq!(listed, 0);
    assert!(commit.is_none());
    let commit = store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"x")))
        .unwrap();
    assert_eq!(
        commit.nonce(),
        &nonce("only"),
        "the plan left the script untouched"
    );
}

#[test]
fn a_plan_whose_closure_writes_through_a_captured_clone_is_rejected() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"before")))
        .unwrap();
    let captured = store.clone();
    let (_, commit) = store
        .plan_and_apply(|view| {
            let current = view.body(name("alpha"), &doc("one")).unwrap().unwrap();
            captured
                .write_document_replicated(name("alpha"), &doc("one"), &body(b"sneaked"))
                .unwrap();
            let swaps = SwapSet::single(Swap::conditional(
                name("alpha"),
                doc("one"),
                body(b"planned"),
                current.digest(),
            ));
            ((), Some(swaps))
        })
        .unwrap();
    assert!(commit.unwrap().is_rejected());
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("sneaked"));
}

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

/// A store whose namespace pointer moves fail once while `gate` is open: a failure after the
/// commit point, which recovery must finish.
fn store_failing_after_prepare(root: &str) -> (Arc<dyn Vfs>, Store, FaultGate) {
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
    let store = seeded_store_on(Arc::clone(&vfs), root);
    (vfs, store, gate)
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

// -- Fencing -------------------------------------------------------------------

#[test]
fn an_ambiguous_failure_fences_the_store_and_keeps_its_claim_until_reopened() {
    let (vfs, store, gate) = store_failing_after_prepare("fenced");
    let expected = seeded_nonces(TEST_SEED, 0, 2);
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

    assert!(store.recovery_required());
    assert!(vfs.exists(&claim_path(&store, &expected[1])).unwrap());
    assert!(matches!(
        store.update_document(Swap::new(name("alpha"), doc("one"), body(b"later"))),
        Err(StoreError::RecoveryRequired { .. })
    ));
    assert!(matches!(
        store.quiesce(),
        Err(StoreError::RecoveryRequired { .. })
    ));

    // Recovery reuses the transaction's staging under the same nonce: that is ownership
    // through its `begin` record, not a collision.
    let claim = claim_path(&store, &expected[1]);
    let store = reopen_with(
        &vfs,
        store,
        StoreOptions::with_nonce_source(seeded_source(TEST_SEED, 1)),
    );
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("after"));
    assert!(!vfs.exists(&claim).unwrap(), "open releases the dead claim");
    assert!(
        store
            .update_document(Swap::new(name("alpha"), doc("one"), body(b"later")))
            .unwrap()
            .is_committed()
    );
}

// -- The commit log is reset only when nothing is unresolved -------------------

#[test]
fn the_outermost_write_leaves_the_commit_log_empty() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"1")))
        .unwrap();
    assert!(store.commit_log_is_empty().unwrap());
    store
        .transform(name("alpha"), &doc("one"), increment)
        .unwrap();
    assert!(store.commit_log_is_empty().unwrap());
    store
        .plan_and_apply(|_| {
            (
                (),
                Some(SwapSet::single(Swap::new(
                    name("beta"),
                    doc("two"),
                    body(b"2"),
                ))),
            )
        })
        .unwrap();
    assert!(store.commit_log_is_empty().unwrap());
}

#[test]
fn a_write_nested_inside_another_leaves_the_reset_to_the_outermost() {
    let (_vfs, store) = seeded_store();
    let inner = store.clone();
    let (records_inside, _) = store
        .plan_and_apply(|_| {
            inner
                .update_document(Swap::new(name("alpha"), doc("one"), body(b"1")))
                .unwrap();
            (inner.commit_log().read().unwrap().len(), None)
        })
        .unwrap();
    assert_eq!(
        records_inside, 3,
        "the nested write's records survive until the outer ends"
    );
    assert!(store.commit_log_is_empty().unwrap());
}

#[test]
fn a_reset_is_refused_while_the_log_holds_a_prepared_transaction_without_its_end() {
    let (_vfs, store) = seeded_store();
    let pending = nonce("pending");
    let log = store.commit_log();
    log.append(&begin(&pending)).unwrap();
    log.append(&LogRecord::Prepare {
        txn: pending.clone(),
        prepared: prepared(&sample_swaps()),
    })
    .unwrap();

    assert!(matches!(
        store.quiesce(),
        Err(StoreError::RecoveryRequired { .. })
    ));
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"1")))
        .unwrap();
    let records = log.read().unwrap();
    assert!(
        records.iter().any(|record| record.txn() == &pending),
        "the automatic reset kept the unresolved transaction"
    );
}

#[test]
fn a_reset_of_a_log_holding_only_finished_work_succeeds() {
    let (_vfs, store) = seeded_store();
    let done = nonce("done");
    let log = store.commit_log();
    log.append(&begin(&done)).unwrap();
    log.append(&LogRecord::Prepare {
        txn: done.clone(),
        prepared: prepared(&sample_swaps()),
    })
    .unwrap();
    log.append(&LogRecord::End { txn: done }).unwrap();
    store.quiesce().unwrap();
    assert!(store.commit_log_is_empty().unwrap());
}

// -- The commit feed -----------------------------------------------------------

#[test]
fn the_feed_reports_logged_commits_and_unlogged_writes_in_order() {
    let (_vfs, store) = seeded_store();
    let recorder = Arc::new(Recorder::default());
    store.observe(recorder.clone());

    store
        .apply_transaction(
            &SwapSet::new([
                Swap::new(name("alpha"), doc("one"), body(b"1")),
                Swap::new(name("beta"), doc("two"), body(b"2")),
            ])
            .unwrap(),
        )
        .unwrap();
    store
        .write_document_replicated(name("alpha"), &doc("one"), &body(b"3"))
        .unwrap();
    store
        .transform(name("alpha"), &doc("one"), increment)
        .unwrap();
    store
        .update_document(Swap::remove(name("beta"), doc("two")))
        .unwrap();

    assert_eq!(
        recorder.events(),
        [
            "write ALPHA/ONE = 1",
            "write BETA/TWO = 2",
            "write ALPHA/ONE = 3",
            "write ALPHA/ONE = 4",
            "remove BETA/TWO",
        ]
    );
}

#[test]
fn the_feed_reports_writes_made_through_any_clone_and_the_old_verbs() {
    let (_vfs, store) = seeded_store();
    let recorder = Arc::new(Recorder::default());
    store.observe(recorder.clone());
    let clone = store.clone();
    clone
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"1")))
        .unwrap();
    store
        .apply_transaction_as(
            &SwapSet::single(Swap::new(name("alpha"), doc("one"), body(b"2"))),
            &nonce("explicit"),
        )
        .unwrap();
    assert_eq!(
        recorder.events(),
        ["write ALPHA/ONE = 1", "write ALPHA/ONE = 2"]
    );
}

#[test]
fn the_feed_reports_nothing_for_a_rejection_or_a_failure() {
    let (_vfs, store, gate) = store_failing_after_prepare("feed-failure");
    let recorder = Arc::new(Recorder::default());
    store.observe(recorder.clone());
    store
        .update_document(Swap::conditional(
            name("alpha"),
            doc("one"),
            body(b"x"),
            body(b"absent").digest(),
        ))
        .unwrap();
    gate.open();
    assert!(
        store
            .update_document(Swap::new(name("alpha"), doc("one"), body(b"y")))
            .is_err()
    );
    gate.close();
    assert!(recorder.events().is_empty());
}

#[test]
fn the_feed_does_not_report_repairs() {
    let (vfs, store) = seeded_store();
    store
        .write_document_replicated(name("alpha"), &doc("one"), &body(b"content"))
        .unwrap();
    let reference = store
        .read_pointer(&name("alpha"), &doc("one"))
        .unwrap()
        .pointer()
        .target();
    let replica = store
        .root()
        .join(BLOB_TREE_DIR)
        .join("p0")
        .join(reference.relative_path());
    vfs.write(&replica, b"rotted").unwrap();
    store.note_damaged_blob(reference);

    let recorder = Arc::new(Recorder::default());
    store.observe(recorder.clone());
    let (report, nonces) = store.repair_reported_damage().unwrap();

    assert_eq!(report.repairs().len(), 1, "{report:?}");
    assert_eq!(nonces.len(), 1, "the repair drew one nonce");
    assert!(recorder.events().is_empty());
    assert_eq!(vfs.read(&replica).unwrap(), b"content");
}

// -- Nonce collisions ----------------------------------------------------------

#[test]
fn a_nonce_another_operation_holds_is_refused() {
    let taken = nonce("taken");
    let (_vfs, store) = scripted_store(vec![taken.clone(), nonce("free")]);
    let held = store.claim_nonce(taken.clone()).unwrap();
    assert!(matches!(
        store.update_document(Swap::new(name("alpha"), doc("one"), body(b"x"))),
        Err(StoreError::NonceCollision { nonce }) if nonce == taken
    ));
    assert_eq!(text(&store, "alpha", "one"), None);
    drop(held);
    let commit = store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"x")))
        .unwrap();
    assert_eq!(commit.nonce(), &nonce("free"));
}

#[test]
fn a_claim_left_by_a_dead_operation_refuses_its_nonce_until_reopen() {
    let orphaned = nonce("orphaned");
    let (vfs, store) = scripted_store(vec![orphaned.clone()]);
    vfs.create_dir_all(&claim_path(&store, &orphaned)).unwrap();
    assert!(matches!(
        store.write_document_replicated(name("alpha"), &doc("one"), &body(b"x")),
        Err(StoreError::NonceCollision { .. })
    ));
    let store = reopen_with(
        &vfs,
        store,
        StoreOptions::with_nonce_source(scripted_source(vec![orphaned.clone()])),
    );
    assert_eq!(
        store
            .write_document_replicated(name("alpha"), &doc("one"), &body(b"x"))
            .unwrap(),
        orphaned
    );
}

#[test]
fn an_orphaned_blob_staging_temp_refuses_its_nonce_and_survives() {
    let orphaned = nonce("blob-orphan");
    let (vfs, store) = scripted_store(vec![orphaned.clone(), orphaned.clone()]);
    for index in 0..store.blob_replica_count() {
        let temp = blob_staging_path(&store, index, &orphaned);
        vfs.create_dir_all(temp.parent().unwrap()).unwrap();
        vfs.write(&temp, b"left by an earlier write").unwrap();
    }
    assert!(matches!(
        store.update_document(Swap::new(name("alpha"), doc("one"), body(b"x"))),
        Err(StoreError::NonceCollision { .. })
    ));
    assert!(matches!(
        store.write_document_replicated(name("alpha"), &doc("one"), &body(b"x")),
        Err(StoreError::NonceCollision { .. })
    ));
    for index in 0..store.blob_replica_count() {
        assert_eq!(
            vfs.read(&blob_staging_path(&store, index, &orphaned))
                .unwrap(),
            b"left by an earlier write"
        );
    }
    assert!(
        !vfs.exists(&claim_path(&store, &orphaned)).unwrap(),
        "the refused claim was released"
    );
    assert_eq!(text(&store, "alpha", "one"), None);
}

#[test]
fn orphaned_namespace_staging_refuses_its_nonce_and_survives() {
    let orphaned = nonce("pointer-orphan");
    let (vfs, store) = scripted_store(vec![orphaned.clone()]);
    let temp = namespace_root(&store, 0)
        .join(STAGING_DIR)
        .join(orphaned.path_component().as_ref())
        .join(format!("one.tmp.{}", orphaned.as_str()));
    vfs.create_dir_all(temp.parent().unwrap()).unwrap();
    vfs.write(&temp, b"left by an earlier transaction").unwrap();
    assert!(matches!(
        store.update_document(Swap::new(name("alpha"), doc("one"), body(b"x"))),
        Err(StoreError::NonceCollision { .. })
    ));
    assert_eq!(vfs.read(&temp).unwrap(), b"left by an earlier transaction");
    assert!(store.commit_log_is_empty().unwrap(), "no begin was written");
}

#[test]
fn a_transaction_id_the_log_already_holds_is_refused_without_disturbing_it() {
    let recorded = nonce("recorded");
    let (_vfs, store) = scripted_store(vec![recorded.clone()]);
    store.commit_log().append(&begin(&recorded)).unwrap();
    assert!(matches!(
        store.update_document(Swap::new(name("alpha"), doc("one"), body(b"x"))),
        Err(StoreError::NonceCollision { nonce }) if nonce == recorded
    ));
    let records = store.commit_log().read().unwrap();
    assert_eq!(
        records,
        vec![begin(&recorded)],
        "the earlier transaction's record is untouched"
    );
    assert!(
        !store.recovery_required(),
        "a refused begin is not an ambiguous failure"
    );
}

#[test]
fn the_explicit_nonce_verbs_also_refuse_a_transaction_id_the_log_holds() {
    let (_vfs, store) = fresh_store();
    store.commit_log().append(&begin(&nonce("t1"))).unwrap();
    assert!(matches!(
        store.apply_transaction_as(
            &SwapSet::single(Swap::new(name("alpha"), doc("one"), body(b"x"))),
            &nonce("t1"),
        ),
        Err(StoreError::NonceCollision { .. })
    ));
}

#[test]
fn a_quarantine_name_already_taken_refuses_the_repair_and_keeps_both_copies() {
    let (vfs, store) = scripted_store(vec![nonce("w1"), nonce("r1")]);
    store
        .write_document_replicated(name("alpha"), &doc("one"), &body(b"content"))
        .unwrap();
    let reference = store
        .read_pointer(&name("alpha"), &doc("one"))
        .unwrap()
        .pointer()
        .target();
    let partition = store.root().join(BLOB_TREE_DIR).join("p0");
    let replica = partition.join(reference.relative_path());
    vfs.write(&replica, b"rotted").unwrap();
    let file = replica.file_name().unwrap().to_string_lossy().into_owned();
    let evidence = partition.join(QUARANTINE_DIR).join(format!("{file}.R1"));
    vfs.create_dir_all(evidence.parent().unwrap()).unwrap();
    vfs.write(&evidence, b"earlier evidence").unwrap();
    store.note_damaged_blob(reference);

    assert!(matches!(
        store.repair_reported_damage(),
        Err(StoreError::NonceCollision { .. })
    ));
    assert_eq!(vfs.read(&evidence).unwrap(), b"earlier evidence");
    assert_eq!(
        vfs.read(&replica).unwrap(),
        b"rotted",
        "the corrupt copy was not lost"
    );
    assert_eq!(
        store.repair_backlog().0,
        1,
        "the suspect waits for the next drain"
    );
}

#[test]
fn a_successful_operation_releases_its_claim() {
    let (vfs, store) = seeded_store();
    let commit = store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"x")))
        .unwrap();
    assert!(!vfs.exists(&claim_path(&store, commit.nonce())).unwrap());
    let written = store
        .write_document_replicated(name("alpha"), &doc("two"), &body(b"y"))
        .unwrap();
    assert!(!vfs.exists(&claim_path(&store, &written)).unwrap());
}

/// `Commit` is what callers correlate on, so it must say what it carried.
#[test]
fn a_commit_exposes_its_outcome_and_nonce() {
    let (_vfs, store) = seeded_store();
    let commit: Commit = store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"x")))
        .unwrap();
    assert_eq!(commit.outcome(), &TransactionOutcome::Committed);
    assert!(commit.is_committed() && !commit.is_rejected());
}
