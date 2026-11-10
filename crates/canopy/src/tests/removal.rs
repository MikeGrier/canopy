// Copyright (c) Michael Grier.

//! Transactional document removal (`BG-5`, `D222`): the effect vocabulary, the
//! all-or-nothing apply, idempotence, preconditions, and the `D53` crash matrix.

use super::*;

/// Seeds a committed document through the transactional path.
fn seed_committed(store: &Store, node: &NodePath, document: &DocumentName, bytes: &[u8]) {
    store
        .apply_transaction(
            &SwapSet::single(Swap::new(node, document.clone(), body(bytes))),
            &nonce("seed"),
        )
        .expect("seed");
}

// -- The effect vocabulary -----------------------------------------------------

#[test]
fn a_removal_carries_no_body_and_says_so() {
    let swap = Swap::remove(name("record"), doc("record.json"));
    assert!(swap.is_removal());
    assert_eq!(swap.body(), None);
    assert_eq!(swap.effect(), &Effect::Remove);
    assert_eq!(swap.expected(), None);
    assert_eq!(swap.precondition(), Precondition::Unconditional);
}

#[test]
fn a_write_is_not_a_removal_and_carries_its_body() {
    let swap = Swap::new(name("record"), doc("record.json"), body(b"v1"));
    assert!(!swap.is_removal());
    assert_eq!(swap.effect(), &Effect::Write(body(b"v1")));
    assert_eq!(swap.body(), Some(&body(b"v1")));
}

#[test]
fn a_conditional_removal_carries_its_expected_digest() {
    let digest = body(b"v1").digest();
    let swap = Swap::remove_conditional(name("record"), doc("record.json"), digest);
    assert!(swap.is_removal());
    assert_eq!(swap.body(), None);
    assert_eq!(swap.expected(), Some(digest));
}

#[test]
fn a_removal_and_a_write_at_one_target_are_still_a_duplicate() {
    // The dedup key is the target, not the effect: "write it and also delete it" has no
    // meaning as one atomic set, and letting it through would make the outcome depend on
    // swap order.
    let result = SwapSet::new([
        Swap::new(name("record"), doc("record.json"), body(b"v1")),
        Swap::remove(name("record"), doc("record.json")),
    ]);
    assert!(matches!(result, Err(SwapSetError::DuplicateTarget { .. })));
}

// -- Applying a removal --------------------------------------------------------

#[test]
fn a_committed_removal_deletes_the_document() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    seed_committed(&store, &node, &d, b"v1");

    let outcome = store
        .apply_transaction(
            &SwapSet::single(Swap::remove(&node, d.clone())),
            &nonce("t1"),
        )
        .expect("remove");
    assert_eq!(outcome, TransactionOutcome::Committed);

    assert_eq!(store.read_body(&node, &d).unwrap(), None);
}

#[test]
fn removing_an_absent_document_succeeds() {
    // Specified, not incidental: `D53` recovery replays a prepared transaction, so a
    // removal that failed on the second run would make a crash between `prepare` and
    // `end` unrecoverable.
    let (_vfs, store) = fresh_store();
    let outcome = store
        .apply_transaction(
            &SwapSet::single(Swap::remove(name("record"), doc("record.json"))),
            &nonce("t1"),
        )
        .expect("removing nothing is not an error");
    assert_eq!(outcome, TransactionOutcome::Committed);
}

#[test]
fn a_removal_is_idempotent_when_applied_twice() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    seed_committed(&store, &node, &d, b"v1");

    for txn in ["t1", "t2"] {
        assert_eq!(
            store
                .apply_transaction(
                    &SwapSet::single(Swap::remove(&node, d.clone())),
                    &nonce(txn)
                )
                .expect("remove"),
            TransactionOutcome::Committed
        );
    }
    assert_eq!(store.read_body(&node, &d).unwrap(), None);
}

#[test]
fn a_removal_leaves_its_siblings_at_the_same_node() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    seed_committed(&store, &node, &doc("record.json"), b"kept");
    seed_committed(&store, &node, &doc("engine"), b"gone");

    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(&node, doc("engine"))),
            &nonce("t1"),
        )
        .expect("remove");

    assert_eq!(
        store.read_body(&node, &doc("record.json")).unwrap(),
        Some(body(b"kept"))
    );
    assert_eq!(store.read_body(&node, &doc("engine")).unwrap(), None);
}

#[test]
fn a_removal_leaves_the_node_behind() {
    // Removing the last document does not remove the node; that is `BG-6`'s explicit
    // node removal. The listing is how a caller sees the difference.
    let (_vfs, store) = fresh_store();
    let node = name("tickets/ab");
    seed_committed(&store, &node, &doc("record"), b"v1");

    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(&node, doc("record"))),
            &nonce("t1"),
        )
        .expect("remove");

    assert!(store.list_children(&node).unwrap().is_empty());
    // The node is still a child of its parent, now holding nothing.
    let parent = store.list_children(name("tickets")).unwrap();
    assert_eq!(
        parent
            .nodes()
            .iter()
            .map(NodePath::to_string)
            .collect::<Vec<_>>(),
        ["TICKETS/AB"]
    );
}

#[test]
fn a_removed_document_disappears_from_the_traversal() {
    let (_vfs, store) = fresh_store();
    seed_committed(&store, &name("a"), &doc("keep"), b"v1");
    seed_committed(&store, &name("a"), &doc("drop"), b"v1");

    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(name("a"), doc("drop"))),
            &nonce("t1"),
        )
        .expect("remove");

    let listed: Vec<String> = store
        .list_documents()
        .unwrap()
        .into_iter()
        .map(|(path, document)| format!("{path}/{document}"))
        .collect();
    assert_eq!(listed, ["A/KEEP"]);
}

// -- Mixed transactions --------------------------------------------------------

#[test]
fn a_write_and_a_removal_land_together_in_one_transaction() {
    // The reason writes and removals share one type: `D216` needs a queue entry's
    // removal to commit with whatever else the action changed.
    let (_vfs, store) = fresh_store();
    seed_committed(&store, &name("entry"), &doc("body"), b"pending");
    seed_committed(&store, &name("ticket"), &doc("record"), b"before");

    let swaps = SwapSet::new([
        Swap::remove(name("entry"), doc("body")),
        Swap::new(name("ticket"), doc("record"), body(b"after")),
    ])
    .expect("distinct targets");
    assert_eq!(
        store.apply_transaction(&swaps, &nonce("t1")).unwrap(),
        TransactionOutcome::Committed
    );

    assert_eq!(store.read_body(name("entry"), &doc("body")).unwrap(), None);
    assert_eq!(
        store.read_body(name("ticket"), &doc("record")).unwrap(),
        Some(body(b"after"))
    );
}

#[test]
fn a_rejected_precondition_anywhere_leaves_the_removal_undone() {
    // All-or-nothing across effects: the write's stale CAS rejects the set, so the
    // removal beside it must not have happened either.
    let (_vfs, store) = fresh_store();
    seed_committed(&store, &name("entry"), &doc("body"), b"pending");
    seed_committed(&store, &name("ticket"), &doc("record"), b"before");

    let swaps = SwapSet::new([
        Swap::remove(name("entry"), doc("body")),
        Swap::conditional(
            name("ticket"),
            doc("record"),
            body(b"after"),
            body(b"stale").digest(),
        ),
    ])
    .expect("distinct targets");

    assert!(
        store
            .apply_transaction(&swaps, &nonce("t1"))
            .unwrap()
            .is_rejected()
    );
    assert_eq!(
        store.read_body(name("entry"), &doc("body")).unwrap(),
        Some(body(b"pending")),
        "the removal must not have landed"
    );
}

// -- Preconditions on a removal ------------------------------------------------

#[test]
fn a_conditional_removal_applies_when_the_digest_matches() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    seed_committed(&store, &node, &d, b"v1");

    let swaps = SwapSet::single(Swap::remove_conditional(
        &node,
        d.clone(),
        body(b"v1").digest(),
    ));
    assert_eq!(
        store.apply_transaction(&swaps, &nonce("t1")).unwrap(),
        TransactionOutcome::Committed
    );
    assert_eq!(store.read_body(&node, &d).unwrap(), None);
}

#[test]
fn a_conditional_removal_is_rejected_on_a_stale_digest() {
    // "Delete what I read, not whatever is there now."
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    seed_committed(&store, &node, &d, b"v2");

    let swaps = SwapSet::single(Swap::remove_conditional(
        &node,
        d.clone(),
        body(b"v1").digest(),
    ));
    match store.apply_transaction(&swaps, &nonce("t1")).unwrap() {
        TransactionOutcome::Rejected {
            expected, current, ..
        } => {
            assert_eq!(
                expected,
                RejectedPrecondition::ExpectedDigest(body(b"v1").digest())
            );
            assert_eq!(current, Some(body(b"v2").digest()));
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert_eq!(
        store.read_body(&node, &d).unwrap(),
        Some(body(b"v2")),
        "a rejected removal changes nothing"
    );
}

#[test]
fn a_conditional_removal_of_an_absent_document_is_rejected() {
    // Absent has no digest, so a digest precondition cannot hold -- which is how a
    // caller expresses "fail if it is not there", since the unconditional removal
    // deliberately succeeds on an absent document.
    let (_vfs, store) = fresh_store();
    let swaps = SwapSet::single(Swap::remove_conditional(
        name("record"),
        doc("record.json"),
        body(b"v1").digest(),
    ));
    match store.apply_transaction(&swaps, &nonce("t1")).unwrap() {
        TransactionOutcome::Rejected { current, .. } => assert_eq!(current, None),
        other => panic!("expected a rejection, got {other:?}"),
    }
}

// -- The `D53` crash matrix ----------------------------------------------------

#[test]
fn recovery_rolls_forward_a_prepared_removal() {
    let (vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    seed_committed(&store, &node, &d, b"v1");

    let txn = nonce("t1");
    let swaps = SwapSet::single(Swap::remove(&node, d.clone()));
    store.commit_log().append(&begin(&txn)).unwrap();
    // A removal stages nothing, which is exactly why the committed copy is still here.
    store.stage_transaction_for_test(&txn, &swaps).unwrap();
    store
        .commit_log()
        .append(&LogRecord::Prepare {
            txn: txn.clone(),
            prepared: prepared(&swaps),
        })
        .unwrap();

    // Crash point: prepared (past the commit point) but nothing deleted yet.
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v1")));

    let recovered = reopen(&vfs, store).unwrap();
    assert_eq!(
        recovered.read_body(&node, &d).unwrap(),
        None,
        "the prepared removal is rolled forward"
    );
    assert!(
        matches!(
            recovered.commit_log().read().unwrap().last(),
            Some(LogRecord::End { txn }) if *txn == nonce("t1")
        ),
        "roll-forward closes the transaction"
    );
}

#[test]
fn recovery_rolls_back_a_begun_but_unprepared_removal() {
    let (vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    seed_committed(&store, &node, &d, b"v1");

    let txn = nonce("t1");
    let swaps = SwapSet::single(Swap::remove(&node, d.clone()));
    store.commit_log().append(&begin(&txn)).unwrap();
    store.stage_transaction_for_test(&txn, &swaps).unwrap();

    // Crash point: begun but never prepared. A removal touches no committed copy before
    // the commit point, so rollback has nothing to undo -- the document must survive.
    let recovered = reopen(&vfs, store).unwrap();
    assert_eq!(
        recovered.read_body(&node, &d).unwrap(),
        Some(body(b"v1")),
        "an unprepared removal is rolled back"
    );
}

#[test]
fn recovery_rolls_forward_a_mixed_prepared_transaction() {
    let (vfs, store) = fresh_store();
    seed_committed(&store, &name("entry"), &doc("body"), b"pending");
    seed_committed(&store, &name("ticket"), &doc("record"), b"before");

    let txn = nonce("t1");
    let swaps = SwapSet::new([
        Swap::remove(name("entry"), doc("body")),
        Swap::new(name("ticket"), doc("record"), body(b"after")),
    ])
    .expect("distinct targets");
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
        recovered.read_body(name("entry"), &doc("body")).unwrap(),
        None
    );
    assert_eq!(
        recovered.read_body(name("ticket"), &doc("record")).unwrap(),
        Some(body(b"after"))
    );
}

// -- The commit log's durable shape --------------------------------------------

#[test]
fn a_removal_round_trips_through_the_commit_log() {
    let (_vfs, store) = fresh_store();
    let log = store.commit_log();
    let txn = nonce("t1");
    let swaps = SwapSet::new([
        Swap::remove(name("a"), doc("plain")),
        Swap::remove_conditional(name("b"), doc("cas"), body(b"v1").digest()),
        Swap::new(name("c"), doc("write"), body(b"v1")),
    ])
    .expect("distinct targets");
    log.append(&LogRecord::Prepare {
        txn,
        prepared: prepared(&swaps),
    })
    .unwrap();

    let records = log.read().expect("read back");
    let LogRecord::Prepare { prepared: read, .. } = &records[0] else {
        panic!("expected a prepare record");
    };
    assert_eq!(read, &prepared(&swaps));
    assert!(read.swaps()[0].is_removal());
    assert_eq!(
        read.swaps()[1].precondition(),
        crate::Precondition::ExpectedDigest(body(b"v1").digest())
    );
    assert!(!read.swaps()[2].is_removal());
    // The write names its body by digest; the two removals name nothing.
    assert_eq!(read.swaps()[0].digest(), None);
    assert_eq!(read.swaps()[1].digest(), None);
    assert_eq!(read.swaps()[2].digest(), Some(body(b"v1").digest()));
}
