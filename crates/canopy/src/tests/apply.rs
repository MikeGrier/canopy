// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! TXN-2: the two-phase `apply_transaction` -- staging, the commit point, and
//! precondition rejection.

use super::*;
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, VfsErrorKind, When};

// -- Two-phase apply (TXN-2) ---------------------------------------------------

#[test]
fn single_document_transaction_commits() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    let swaps = SwapSet::single(Swap::new(node.clone(), d.clone(), body(b"payload")));
    let outcome = store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    assert_eq!(outcome, TransactionOutcome::Committed);
    assert!(outcome.is_committed());
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"payload")));
}

use crate::store::STAGING_DIR;

#[test]
fn a_non_not_found_staged_rename_error_is_propagated() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Rename,
        PathMatcher::has_component(STAGING_DIR),
        Fault::io_error(),
        When::Always,
        gate.clone(),
    );
    let fault: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let root = PathBuf::from("shire");
    fault.create_dir_all(&root).unwrap();
    let store = Store::init(fault, root, test_config()).unwrap();
    gate.open();

    let swaps = SwapSet::single(Swap::new(name("record"), doc("record.json"), body(b"x")));
    match store.apply_transaction(&swaps, &nonce("apply1")) {
        Err(StoreError::Vfs(error)) => assert_eq!(error.kind(), VfsErrorKind::Other),
        other => panic!("expected Vfs(Other), got {other:?}"),
    }
}

#[test]
fn transaction_propagates_staging_tree_cleanup_errors() {
    let gate = FaultGate::closed();
    let root = PathBuf::from("shire");
    let staging = root
        .join(crate::store::NAMESPACE_TREE_DIR)
        .join("p0")
        .join(STAGING_DIR)
        .join("APPLY1");
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::RemoveDirAll,
        PathMatcher::exact(staging),
        Fault::io_error(),
        When::Always,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(vfs, root, test_config()).unwrap();
    gate.open();

    let swaps = SwapSet::single(Swap::new(name("record"), doc("record.json"), body(b"x")));
    match store.apply_transaction(&swaps, &nonce("apply1")) {
        Err(StoreError::Vfs(error)) => assert_eq!(error.kind(), VfsErrorKind::Other),
        other => panic!("expected Vfs(Other), got {other:?}"),
    }
}

#[test]
fn recovery_does_not_tolerate_non_not_found_rename_errors() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Rename,
        PathMatcher::has_component(STAGING_DIR),
        Fault::io_error(),
        When::Always,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).unwrap();
    let txn = nonce("recover-rename");
    let swaps = SwapSet::single(Swap::new(
        name("record"),
        doc("record.json"),
        body(b"prepared"),
    ));
    store.commit_log().append(&begin(&txn)).unwrap();
    store.stage_transaction_for_test(&txn, &swaps).unwrap();
    store
        .commit_log()
        .append(&LogRecord::Prepare {
            txn,
            prepared: prepared(&swaps),
        })
        .unwrap();
    drop(store);
    gate.open();

    match Store::open(vfs, root) {
        Err(StoreError::Vfs(error)) => assert_eq!(error.kind(), VfsErrorKind::Other),
        other => panic!("expected rename Vfs(Other), got {other:?}"),
    }
}

#[test]
fn a_committed_transaction_logs_begin_prepare_end() {
    let (_vfs, store) = fresh_store();
    let swaps = SwapSet::single(Swap::new(name("record"), doc("record.json"), body(b"x")));
    store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    let records = store.commit_log().read().unwrap();
    assert!(
        matches!(records.as_slice(),
            [
                LogRecord::Begin { txn: b, .. },
                LogRecord::Prepare { txn: p, .. },
                LogRecord::End { txn: e },
            ] if *b == nonce("apply1") && *p == nonce("apply1") && *e == nonce("apply1")
        ),
        "log is begin/prepare/end for the transaction, got {records:?}"
    );
}

#[test]
fn a_multi_document_transaction_commits_every_target() {
    let (_vfs, store) = fresh_store();
    // Two nodes, and two documents at the same node (D62): four distinct targets.
    let swaps = SwapSet::new([
        Swap::new(name("alpha"), doc("record.json"), body(b"a-record")),
        Swap::new(name("alpha"), doc("engine"), body(b"a-engine")),
        Swap::new(name("bravo"), doc("record.json"), body(b"b-record")),
    ])
    .expect("distinct targets");
    let outcome = store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    assert_eq!(outcome, TransactionOutcome::Committed);
    assert_eq!(
        store.read_body(name("alpha"), &doc("record.json")).unwrap(),
        Some(body(b"a-record"))
    );
    assert_eq!(
        store.read_body(name("alpha"), &doc("engine")).unwrap(),
        Some(body(b"a-engine"))
    );
    assert_eq!(
        store.read_body(name("bravo"), &doc("record.json")).unwrap(),
        Some(body(b"b-record"))
    );
}

#[test]
fn an_empty_transaction_is_a_vacuous_commit() {
    let (_vfs, store) = fresh_store();
    let outcome = store
        .apply_transaction(&SwapSet::new([]).unwrap(), &nonce("apply1"))
        .unwrap();
    assert_eq!(outcome, TransactionOutcome::Committed);
    // The log still brackets the (empty) transaction.
    let records = store.commit_log().read().unwrap();
    assert_eq!(records.len(), 3, "begin/prepare/end even for an empty set");
}

#[test]
fn a_conditional_swap_commits_when_its_precondition_holds() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    store
        .write_document_replicated(&node, &d, &body(b"old"), &nonce("seed"))
        .unwrap();
    let swaps = SwapSet::single(Swap::conditional(
        node.clone(),
        d.clone(),
        body(b"new"),
        body(b"old").digest(),
    ));
    let outcome = store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    assert_eq!(outcome, TransactionOutcome::Committed);
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"new")));
}

#[test]
fn a_conditional_swap_rejects_the_whole_transaction_when_its_precondition_fails() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    store
        .write_document_replicated(&node, &d, &body(b"actual"), &nonce("seed"))
        .unwrap();
    // A second, unconditional swap that must NOT land because the transaction is
    // rejected atomically.
    let swaps = SwapSet::new([
        Swap::conditional(
            node.clone(),
            d.clone(),
            body(b"new"),
            body(b"stale").digest(),
        ),
        Swap::new(name("other"), doc("record.json"), body(b"other")),
    ])
    .expect("distinct targets");
    let outcome = store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    assert_eq!(
        outcome,
        TransactionOutcome::Rejected {
            node: "RECORD".to_owned(),
            document: Some("RECORD.JSON".to_owned()),
            expected: RejectedPrecondition::ExpectedDigest(body(b"stale").digest()),
            current: Some(body(b"actual").digest()),
        }
    );
    assert!(outcome.is_rejected());
    // Nothing changed and nothing was logged.
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"actual")));
    assert_eq!(
        store.read_body(name("other"), &doc("record.json")).unwrap(),
        None
    );
    assert_eq!(store.commit_log().read().unwrap(), Vec::new());
}

#[test]
fn a_conditional_swap_on_an_absent_document_rejects_with_no_current() {
    let (_vfs, store) = fresh_store();
    let swaps = SwapSet::single(Swap::conditional(
        name("record"),
        doc("record.json"),
        body(b"new"),
        body(b"expected").digest(),
    ));
    let outcome = store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    assert_eq!(
        outcome,
        TransactionOutcome::Rejected {
            node: "RECORD".to_owned(),
            document: Some("RECORD.JSON".to_owned()),
            expected: RejectedPrecondition::ExpectedDigest(body(b"expected").digest()),
            current: None,
        }
    );
}

#[test]
fn a_conditional_absent_swap_creates_a_document_that_does_not_yet_exist() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("engine");
    let swaps = SwapSet::single(Swap::conditional_absent(
        node.clone(),
        d.clone(),
        body(b"created"),
    ));
    let outcome = store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    assert_eq!(outcome, TransactionOutcome::Committed);
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"created")));
}

#[test]
fn a_conditional_absent_swap_rejects_when_the_document_already_exists() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("engine");
    store
        .write_document_replicated(&node, &d, &body(b"present"), &nonce("seed"))
        .unwrap();
    // A second, unconditional swap that must NOT land because the transaction is
    // rejected atomically on the absent-precondition miss.
    let swaps = SwapSet::new([
        Swap::conditional_absent(node.clone(), d.clone(), body(b"created")),
        Swap::new(name("other"), doc("record.json"), body(b"other")),
    ])
    .expect("distinct targets");
    let outcome = store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    assert_eq!(
        outcome,
        TransactionOutcome::Rejected {
            node: "RECORD".to_owned(),
            document: Some("ENGINE".to_owned()),
            expected: RejectedPrecondition::ExpectedAbsent,
            current: Some(body(b"present").digest()),
        }
    );
    // Nothing changed and nothing was logged.
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"present")));
    assert_eq!(
        store.read_body(name("other"), &doc("record.json")).unwrap(),
        None
    );
    assert_eq!(store.commit_log().read().unwrap(), Vec::new());
}

#[test]
fn a_prepare_record_names_every_target_by_digest_and_carries_no_body() {
    // Renamed from `a_prepare_record_carries_the_full_swap_set`, which `CVS-14` made
    // false: the record still describes every target, but names each body by digest
    // rather than carrying it.
    let (vfs, store) = fresh_store();
    let swaps = SwapSet::new([
        Swap::new(name("alpha"), doc("record.json"), body(b"a")),
        Swap::new(name("bravo"), doc("record.json"), body(b"b")),
    ])
    .expect("distinct targets");
    store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    let records = store.commit_log().read().unwrap();
    let prepare = records
        .iter()
        .find_map(|r| match r {
            LogRecord::Prepare { prepared, .. } => Some(prepared),
            _ => None,
        })
        .expect("a prepare record");

    // Every target survives, in order, each naming the digest of the body written there.
    assert_eq!(prepare, &prepared(&swaps));
    assert_eq!(prepare.swaps()[0].digest(), Some(body(b"a").digest()));
    assert_eq!(prepare.swaps()[1].digest(), Some(body(b"b").digest()));

    // And the bodies themselves are not in the log. Asserted over the raw bytes rather
    // than the parsed record, because the parsed type has no body field to inspect --
    // only the file can say whether one was written.
    let raw = String::from_utf8(vfs.read(store.commit_log().path()).unwrap()).unwrap();
    assert!(
        !raw.contains("\"body\""),
        "the prepare record must carry no body field: {raw}"
    );
}

#[test]
fn a_committed_transaction_sweeps_its_staging() {
    let (vfs, store) = fresh_store();
    let swaps = SwapSet::single(Swap::new(name("record"), doc("record.json"), body(b"x")));
    store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    for index in 0..store.pointer_replica_count() {
        assert!(
            !staging_exists(&vfs, &store, index, &nonce("apply1")),
            "replica {index} staging swept"
        );
        assert_eq!(
            node_entries(&vfs, &store, index),
            vec![OsString::from("RECORD.JSON")],
            "replica {index} holds only the committed document"
        );
    }
}

#[test]
fn re_applying_a_committed_transaction_is_a_safe_no_op() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    let swaps = SwapSet::single(Swap::new(node.clone(), d.clone(), body(b"payload")));
    store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    // Applying the same swap set again (same body) is a repeatable no-op change.
    let outcome = store.apply_transaction(&swaps, &nonce("apply2")).unwrap();
    assert_eq!(outcome, TransactionOutcome::Committed);
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"payload")));
}

#[test]
fn a_transaction_overwrites_an_existing_document() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    store
        .write_document_replicated(&node, &d, &body(b"first"), &nonce("seed"))
        .unwrap();
    let swaps = SwapSet::single(Swap::new(node.clone(), d.clone(), body(b"second")));
    store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    let read = store.read_body(&node, &d).unwrap();
    assert_eq!(read, Some(body(b"second")));
}

fn node(raw: &str) -> NodePath {
    NodePath::parse(raw).expect("valid test node path")
}

/// A transaction that can never apply is refused before `prepare`, so recovery never
/// inherits a commitment it cannot complete.
#[test]
fn a_document_over_an_existing_child_node_is_refused_before_prepare() {
    let (vfs, store) = fresh_store();
    store
        .update_document(
            Swap::new(node("parent/child"), doc("leaf"), body(b"x")),
            &nonce("t1"),
        )
        .unwrap();
    store.quiesce_commit_log().unwrap();

    let refused = store
        .update_document(
            Swap::new(node("parent"), doc("child"), body(b"y")),
            &nonce("t2"),
        )
        .expect_err("a document cannot share a child node's name");
    assert!(
        matches!(refused, StoreError::NameIsChildNode { .. }),
        "got {refused:?}"
    );
    assert!(store.commit_log().read().unwrap().is_empty());
    reopen(&vfs, store).expect("the store reopens after a refused transaction");
}

#[test]
fn a_node_under_an_existing_document_is_refused_before_prepare() {
    let (vfs, store) = fresh_store();
    store
        .update_document(
            Swap::new(node("parent"), doc("child"), body(b"x")),
            &nonce("t1"),
        )
        .unwrap();
    store.quiesce_commit_log().unwrap();

    let refused = store
        .update_document(
            Swap::new(node("parent/child"), doc("leaf"), body(b"y")),
            &nonce("t2"),
        )
        .expect_err("a node cannot nest under a document");
    assert!(
        matches!(refused, StoreError::PathIsDocument { .. }),
        "got {refused:?}"
    );
    assert!(store.commit_log().read().unwrap().is_empty());
    reopen(&vfs, store).expect("the store reopens after a refused transaction");
}

#[test]
fn colliding_writes_within_one_set_are_refused_before_prepare() {
    let (vfs, store) = fresh_store();
    let swaps = SwapSet::new([
        Swap::new(node("parent"), doc("child"), body(b"document")),
        Swap::new(node("parent/child/deeper"), doc("leaf"), body(b"nested")),
    ])
    .unwrap();

    let refused = store
        .apply_transaction(&swaps, &nonce("t1"))
        .expect_err("the set contradicts itself");
    assert!(
        matches!(refused, StoreError::NameIsChildNode { .. }),
        "got {refused:?}"
    );
    assert!(store.commit_log().read().unwrap().is_empty());
    let reopened = reopen(&vfs, store).expect("the store reopens after a refused transaction");
    assert_eq!(
        reopened
            .read_document(node("parent"), &doc("child"))
            .unwrap(),
        crate::Read::Absent
    );
}

/// A write and a node removal that name the same path can never both apply: the write makes
/// the path a file and the removal's `rmdir` then fails on every recovery.
#[test]
fn a_write_at_a_removed_node_path_is_refused_before_prepare() {
    let (vfs, store) = fresh_store();
    let swaps = SwapSet::with_node_removals(
        [Swap::new(node("a"), doc("b"), body(b"document"))],
        [node("a/b")],
    )
    .expect("construction cannot see that the write's entry is the removed node");

    let refused = store
        .apply_transaction(&swaps, &nonce("t1"))
        .expect_err("the write and the removal contradict each other");
    assert!(
        matches!(refused, StoreError::NameIsChildNode { ref path, ref name }
            if path.to_string() == "A" && name == "B"),
        "got {refused:?}"
    );
    assert!(store.commit_log().read().unwrap().is_empty());
    store
        .update_document(
            Swap::new(node("a"), doc("other"), body(b"after")),
            &nonce("t2"),
        )
        .expect("the refusal did not fence the store");
    let reopened = reopen(&vfs, store).expect("the store reopens after a refused transaction");
    assert_eq!(
        reopened.read_document(node("a"), &doc("b")).unwrap(),
        crate::Read::Absent
    );
}

#[test]
fn a_write_above_a_removed_node_is_refused_before_prepare() {
    for removed in ["a/b/c", "a/b/c/d"] {
        let (vfs, store) = fresh_store();
        let swaps = SwapSet::with_node_removals(
            [Swap::new(node("a"), doc("b"), body(b"document"))],
            [node(removed)],
        )
        .unwrap();

        let refused = store
            .apply_transaction(&swaps, &nonce("t1"))
            .expect_err("the write puts a file above the removed node");
        assert!(
            matches!(refused, StoreError::NameIsChildNode { .. }),
            "{removed}: got {refused:?}"
        );
        assert!(store.commit_log().read().unwrap().is_empty(), "{removed}");
        reopen(&vfs, store).expect("the store reopens after a refused transaction");
    }
}

/// A node that is, or lies beneath, a document already on disk cannot be removed as a
/// directory; admitting it would fence the store and make every reopen retry it.
#[test]
fn removing_a_node_at_or_beneath_an_existing_document_is_refused_before_prepare() {
    for removed in ["a/b", "a/b/c", "a/b/c/d"] {
        let (vfs, store) = fresh_store();
        store
            .update_document(Swap::new(node("a"), doc("b"), body(b"doc")), &nonce("t1"))
            .unwrap();
        store.quiesce_commit_log().unwrap();
        let swaps = SwapSet::with_node_removals([], [node(removed)]).unwrap();

        let refused = store
            .apply_transaction(&swaps, &nonce("t2"))
            .expect_err("a document is not a node");
        assert!(
            matches!(refused, StoreError::PathIsDocument { ref path } if path.to_string() == "A/B"),
            "{removed}: got {refused:?}"
        );
        assert!(store.commit_log().read().unwrap().is_empty(), "{removed}");
        store
            .update_document(Swap::new(node("a"), doc("c"), body(b"after")), &nonce("t3"))
            .expect("the refusal did not fence the store");
        let reopened = reopen(&vfs, store).expect("the store reopens after a refused removal");
        assert_ne!(
            reopened.read_document(node("a"), &doc("b")).unwrap(),
            crate::Read::Absent
        );
    }
}

#[test]
fn removing_an_absent_node_beside_a_document_still_commits() {
    let (_vfs, store) = fresh_store();
    store
        .update_document(Swap::new(node("a"), doc("b"), body(b"doc")), &nonce("t1"))
        .unwrap();
    let swaps = SwapSet::with_node_removals([], [node("a/bx/c")]).unwrap();
    assert_eq!(
        store.apply_transaction(&swaps, &nonce("t2")).unwrap(),
        TransactionOutcome::Committed
    );
}

#[test]
fn a_write_beside_a_removed_node_still_commits() {
    // `A/BX` is not `A/B`, and a document in `A` is not above `A/B`: only an entry path that
    // is the removed node, or one of its ancestors, collides.
    let (vfs, store) = fresh_store();
    store
        .update_document(
            Swap::new(node("a/b"), doc("leaf"), body(b"x")),
            &nonce("seed"),
        )
        .unwrap();
    let swaps = SwapSet::with_node_removals(
        [
            Swap::remove(node("a/b"), doc("leaf")),
            Swap::new(node("a"), doc("bx"), body(b"beside")),
            Swap::new(node("a"), doc("other"), body(b"above-parent")),
        ],
        [node("a/b")],
    )
    .unwrap();

    assert_eq!(
        store.apply_transaction(&swaps, &nonce("t1")).unwrap(),
        TransactionOutcome::Committed
    );
    let reopened = reopen(&vfs, store).unwrap();
    assert_ne!(
        reopened.read_document(node("a"), &doc("bx")).unwrap(),
        crate::Read::Absent
    );
}

#[test]
fn sibling_writes_that_only_share_a_prefix_are_not_collisions() {
    let (_vfs, store) = fresh_store();
    let swaps = SwapSet::new([
        Swap::new(node("parent"), doc("child.json"), body(b"document")),
        Swap::new(node("parent/child"), doc("leaf"), body(b"nested")),
    ])
    .unwrap();
    assert!(
        store
            .apply_transaction(&swaps, &nonce("t1"))
            .unwrap()
            .is_committed()
    );
}
