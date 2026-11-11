// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! TXN-4: recovery replay on `open` (`D46`, `D53`, `D54`).

use super::*;

// -- Recovery replay on open (TXN-4, D46/D53/D54) ------------------------------

#[test]
fn recovery_on_an_absent_log_is_the_fast_path() {
    let (vfs, store) = fresh_store();
    // A committed document, then the log emptied at quiescence -- the clean state.
    let swaps = SwapSet::single(Swap::new(name("record"), doc("record.json"), body(b"kept")));
    store.apply_transaction_as(&swaps, &nonce("t1")).unwrap();
    store.quiesce_commit_log().unwrap();
    assert!(store.commit_log().read().unwrap().is_empty());
    // Reopen: an empty log skips replay and disturbs nothing.
    let recovered = reopen(&vfs, store).unwrap();
    assert_eq!(
        recovered
            .read_body(name("record"), &doc("record.json"))
            .unwrap(),
        Some(body(b"kept"))
    );
    assert!(recovered.commit_log().read().unwrap().is_empty());
}

#[test]
fn recovery_sweeps_an_applied_transaction_and_keeps_its_data() {
    let (vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    let swaps = SwapSet::single(Swap::new(node.clone(), d.clone(), body(b"v1")));
    store.apply_transaction_as(&swaps, &nonce("t1")).unwrap();
    // The log still holds begin/prepare/end (not quiesced): reopening treats the applied
    // transaction as a sweep no-op and keeps its data.
    let recovered = reopen(&vfs, store).unwrap();
    assert_eq!(recovered.read_body(&node, &d).unwrap(), Some(body(b"v1")));
    for index in 0..recovered.pointer_replica_count() {
        assert!(
            !staging_exists(&vfs, &recovered, index, &nonce("t1")),
            "replica {index} staging swept on recovery"
        );
    }
}

#[test]
fn recovery_rolls_forward_a_prepared_transaction() {
    let (vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    let swaps = SwapSet::single(Swap::new(node.clone(), d.clone(), body(b"v1")));
    let txn = nonce("t1");
    store.commit_log().append(&begin(&txn)).unwrap();
    store.stage_transaction_for_test(&txn, &swaps).unwrap();
    store
        .commit_log()
        .append(&LogRecord::Prepare {
            txn: txn.clone(),
            prepared: prepared(&swaps),
        })
        .unwrap();
    // Crash point: prepared (past the commit point) but nothing renamed yet.
    assert_eq!(store.read_body(&node, &d).unwrap(), None);
    // Reopen: recovery rolls the prepared transaction forward into the namespace.
    let recovered = reopen(&vfs, store).unwrap();
    assert_eq!(recovered.read_body(&node, &d).unwrap(), Some(body(b"v1")));
    // The transaction is now closed with an end record and its staging is swept.
    assert!(
        matches!(
            recovered.commit_log().read().unwrap().last(),
            Some(LogRecord::End { txn }) if *txn == nonce("t1")
        ),
        "roll-forward appends the transaction's end record"
    );
    for index in 0..recovered.pointer_replica_count() {
        assert!(!staging_exists(&vfs, &recovered, index, &txn));
    }
}

#[test]
fn recovery_rolls_back_a_begun_but_unprepared_transaction() {
    let (vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    let swaps = SwapSet::single(Swap::new(node.clone(), d.clone(), body(b"v1")));
    let txn = nonce("t1");
    store.commit_log().append(&begin(&txn)).unwrap();
    store.stage_transaction_for_test(&txn, &swaps).unwrap();
    // Crash point: begun and staged, but no prepare -- the commit point was never
    // reached, so no committed copy was ever touched.
    let recovered = reopen(&vfs, store).unwrap();
    // Rolled back: nothing committed and the staging temps are discarded.
    assert_eq!(recovered.read_body(&node, &d).unwrap(), None);
    for index in 0..recovered.pointer_replica_count() {
        assert!(!staging_exists(&vfs, &recovered, index, &txn));
    }
}

#[test]
fn recovery_rolls_forward_a_multi_document_transaction() {
    let (vfs, store) = fresh_store();
    let swaps = SwapSet::new([
        Swap::new(name("alpha"), doc("record.json"), body(b"a")),
        Swap::new(name("bravo"), doc("record.json"), body(b"b")),
        Swap::new(name("alpha"), doc("other.json"), body(b"a2")),
    ])
    .expect("distinct targets");
    let txn = nonce("t1");
    store.commit_log().append(&begin(&txn)).unwrap();
    store.stage_transaction_for_test(&txn, &swaps).unwrap();
    store
        .commit_log()
        .append(&LogRecord::Prepare {
            txn: txn.clone(),
            prepared: prepared(&swaps),
        })
        .unwrap();
    let recovered = reopen(&vfs, store).unwrap();
    assert_eq!(
        recovered
            .read_body(name("alpha"), &doc("record.json"))
            .unwrap(),
        Some(body(b"a"))
    );
    assert_eq!(
        recovered
            .read_body(name("bravo"), &doc("record.json"))
            .unwrap(),
        Some(body(b"b"))
    );
    assert_eq!(
        recovered
            .read_body(name("alpha"), &doc("other.json"))
            .unwrap(),
        Some(body(b"a2"))
    );
    for index in 0..recovered.pointer_replica_count() {
        assert!(!staging_exists(&vfs, &recovered, index, &txn));
    }
}

#[test]
fn reopening_after_roll_forward_is_idempotent() {
    let (vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    let swaps = SwapSet::single(Swap::new(node.clone(), d.clone(), body(b"v1")));
    let txn = nonce("t1");
    store.commit_log().append(&begin(&txn)).unwrap();
    store.stage_transaction_for_test(&txn, &swaps).unwrap();
    store
        .commit_log()
        .append(&LogRecord::Prepare {
            txn: txn.clone(),
            prepared: prepared(&swaps),
        })
        .unwrap();
    // First open rolls the transaction forward (and appends its end record). Its handle
    // is dropped at the end of the statement, which releases the root for the second.
    let first = reopen(&vfs, store).unwrap();
    // A second open over the now-ended log is a sweep no-op that keeps the data.
    let reopened = reopen(&vfs, first).unwrap();
    assert_eq!(reopened.read_body(&node, &d).unwrap(), Some(body(b"v1")));
}

#[test]
fn recovery_rolls_forward_a_conditional_swap_without_a_fence() {
    let (vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    // A conditional swap whose precondition would FAIL a live CAS: the document is
    // absent, so no digest matches `expected`. Recovery compares no versions (D54), so
    // the prepared swap is re-applied regardless.
    let swaps = SwapSet::single(Swap::conditional(
        node.clone(),
        d.clone(),
        body(b"new"),
        body(b"a-precondition-that-does-not-hold").digest(),
    ));
    let txn = nonce("t1");
    store.commit_log().append(&begin(&txn)).unwrap();
    store.stage_transaction_for_test(&txn, &swaps).unwrap();
    store
        .commit_log()
        .append(&LogRecord::Prepare {
            txn: txn.clone(),
            prepared: prepared(&swaps),
        })
        .unwrap();
    let recovered = reopen(&vfs, store).unwrap();
    assert_eq!(recovered.read_body(&node, &d).unwrap(), Some(body(b"new")));
}

#[test]
fn recovery_rolls_forward_multiple_transactions_in_log_order() {
    let (vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    let first = SwapSet::single(Swap::new(node.clone(), d.clone(), body(b"first")));
    let second = SwapSet::single(Swap::new(node.clone(), d.clone(), body(b"second")));
    let t1 = nonce("t1");
    let t2 = nonce("t2");
    // Two prepared-but-unended transactions on the same target, t1 before t2.
    store.commit_log().append(&begin(&t1)).unwrap();
    store.stage_transaction_for_test(&t1, &first).unwrap();
    store
        .commit_log()
        .append(&LogRecord::Prepare {
            txn: t1.clone(),
            prepared: prepared(&first),
        })
        .unwrap();
    store.commit_log().append(&begin(&t2)).unwrap();
    store.stage_transaction_for_test(&t2, &second).unwrap();
    store
        .commit_log()
        .append(&LogRecord::Prepare {
            txn: t2.clone(),
            prepared: prepared(&second),
        })
        .unwrap();
    // Recovery replays in log order, so the later transaction wins the shared target.
    let recovered = reopen(&vfs, store).unwrap();
    assert_eq!(
        recovered.read_body(&node, &d).unwrap(),
        Some(body(b"second"))
    );
    for index in 0..recovered.pointer_replica_count() {
        assert!(!staging_exists(&vfs, &recovered, index, &t1));
        assert!(!staging_exists(&vfs, &recovered, index, &t2));
    }
}

#[test]
fn recovery_surfaces_a_corrupt_log_as_a_typed_error() {
    let (vfs, store) = fresh_store();
    // A complete but non-JSON log line is corruption, not a torn tail.
    vfs.write(store.commit_log().path(), b"{not valid json}\n")
        .unwrap();
    let err = reopen(&vfs, store).unwrap_err();
    assert!(
        matches!(err, StoreError::CommitLog(_)),
        "a corrupt log surfaces as a typed CommitLog error, got {err:?}"
    );
}

/// A committed write survives a power loss, not merely a restart.
///
/// The write creates fresh shard and node directories several levels deep; unless each new
/// directory's name is synced into its parent, the crash drops the chain and the document
/// with it, even though the files inside were fsynced.
#[test]
fn a_committed_write_into_new_directories_survives_a_crash() {
    let mem = Arc::new(MemFs::new());
    let vfs: Arc<dyn Vfs> = mem.clone();
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();

    let node = NodePath::parse("deep/nested/node").unwrap();
    let outcome = store
        .update_document_as(
            Swap::new(&node, doc("record.json"), body(b"durable")),
            &nonce("t1"),
        )
        .unwrap();
    assert!(outcome.is_committed());
    drop(store);

    mem.simulate_crash();
    let reopened = Store::open_unsourced(Arc::clone(&vfs), root).unwrap();
    assert_eq!(
        reopened.read_document(&node, &doc("record.json")).unwrap(),
        crate::Read::Found(body(b"durable"))
    );
}
