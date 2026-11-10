// Copyright (c) Michael Grier.

//! Transactional node removal (`BG-6`, `D222`): the construction rules that refuse a
//! contradictory set, the **post-transaction** emptiness precondition, the all-or-nothing
//! apply, idempotence, and the `D53` crash matrix.
//!
//! The precondition here is unlike every other one in `D53`: those ask about state
//! *before* the transaction, while emptiness is judged against the node as it will stand
//! *after* the transaction's own effects land. The tests that pin that distinction are
//! [`the_last_document_and_its_node_go_in_one_transaction`] and
//! [`a_document_this_transaction_writes_keeps_the_node_from_being_removed`].

use super::*;

/// Seeds a committed document through the transactional path.
fn seed(store: &Store, node: &NodePath, document: &DocumentName, bytes: &[u8]) {
    store
        .apply_transaction(
            &SwapSet::single(Swap::new(node, document.clone(), body(bytes))),
            &nonce("seed"),
        )
        .expect("seed");
}

/// Whether `path`'s directory is present, checked in every namespace replica.
///
/// Reads the directories rather than listing the parent, because a root-level node has
/// no parent path to list -- the store root is not itself a node. Asserting agreement
/// across replicas is deliberate: a node exists everywhere or nowhere, and a removal
/// that took only some copies is exactly the bug worth catching here.
fn node_exists(vfs: &Arc<dyn Vfs>, store: &Store, path: &NodePath) -> bool {
    let present: Vec<bool> = (0..store.pointer_replica_count())
        .map(|index| {
            vfs.read_dir(&namespace_root(store, index).join(path.relative_path()))
                .is_ok()
        })
        .collect();
    assert!(
        present.iter().all(|seen| *seen == present[0]),
        "replicas disagree about whether {path} exists: {present:?}"
    );
    present[0]
}

/// A set that removes `path` and nothing else.
fn remove_node(path: &NodePath) -> SwapSet {
    SwapSet::with_node_removals([], [path.clone()]).expect("a lone removal is always valid")
}

// -- Construction rules --------------------------------------------------------

#[test]
fn a_set_carries_its_node_removals() {
    let swaps = SwapSet::with_node_removals(
        [Swap::new(name("other"), doc("record"), body(b"v1"))],
        [name("record/aa"), name("record/ab")],
    )
    .expect("distinct targets");

    assert_eq!(swaps.swaps().len(), 1);
    assert_eq!(
        swaps
            .node_removals()
            .iter()
            .map(NodePath::to_string)
            .collect::<Vec<_>>(),
        ["RECORD/AA", "RECORD/AB"]
    );
    assert!(!swaps.is_empty());
}

#[test]
fn a_set_of_only_node_removals_is_not_empty() {
    // `is_empty` decides whether there is work to do, and a node removal is work even
    // though it moves no document.
    let swaps = SwapSet::with_node_removals([], [name("record")]).expect("valid");
    assert!(swaps.swaps().is_empty());
    assert!(!swaps.is_empty());
}

#[test]
fn an_entirely_empty_set_is_empty() {
    let swaps = SwapSet::with_node_removals([], []).expect("valid");
    assert!(swaps.is_empty());
}

#[test]
fn a_plain_swap_set_removes_no_nodes() {
    let swaps = SwapSet::single(Swap::new(name("record"), doc("record.json"), body(b"v1")));
    assert!(swaps.node_removals().is_empty());
}

#[test]
fn removing_one_node_twice_is_refused() {
    let result = SwapSet::with_node_removals([], [name("record"), name("record")]);
    assert!(matches!(
        result,
        Err(SwapSetError::DuplicateNodeRemoval { node }) if node == "RECORD"
    ));
}

#[test]
fn a_write_at_a_removed_node_is_refused() {
    // Contradictory rather than merely redundant: neither ordering gives the caller what
    // they asked for, so the set never reaches a store.
    let result = SwapSet::with_node_removals(
        [Swap::new(name("record"), doc("record.json"), body(b"v1"))],
        [name("record")],
    );
    assert!(matches!(
        result,
        Err(SwapSetError::WriteBeneathRemovedNode { node, write, document })
            if node == "RECORD" && write == "RECORD" && document == "RECORD.JSON"
    ));
}

#[test]
fn a_write_beneath_a_removed_node_is_refused() {
    // The check is by containment, not equality: writing into a descendant would
    // re-create the very subtree the removal is taking.
    let result = SwapSet::with_node_removals(
        [Swap::new(
            name("record/aa/deeper"),
            doc("record.json"),
            body(b"v1"),
        )],
        [name("record")],
    );
    assert!(matches!(
        result,
        Err(SwapSetError::WriteBeneathRemovedNode { node, write, .. })
            if node == "RECORD" && write == "RECORD/AA/DEEPER"
    ));
}

#[test]
fn a_write_beside_a_removed_node_is_allowed() {
    // Containment, not prefix-of-the-string: `RECORDS` is not beneath `RECORD` even
    // though the one name starts with the other.
    let swaps = SwapSet::with_node_removals(
        [Swap::new(name("records"), doc("record.json"), body(b"v1"))],
        [name("record")],
    )
    .expect("a sibling write is untouched by the removal");
    assert_eq!(swaps.swaps().len(), 1);
}

#[test]
fn a_document_removal_at_a_removed_node_is_allowed() {
    // The driving case of `BG-6`, refused by no construction rule: emptying the node is
    // exactly how a caller earns the right to remove it.
    let swaps = SwapSet::with_node_removals(
        [Swap::remove(name("record"), doc("record.json"))],
        [name("record")],
    )
    .expect("emptying a node is not a write beneath it");
    assert_eq!(swaps.swaps().len(), 1);
    assert_eq!(swaps.node_removals().len(), 1);
}

#[test]
fn the_contradiction_errors_read_as_sentences() {
    let duplicate = SwapSet::with_node_removals([], [name("record"), name("record")])
        .expect_err("duplicate node removal");
    assert!(
        duplicate.to_string().contains("RECORD"),
        "{duplicate} names the node"
    );

    let contradiction = SwapSet::with_node_removals(
        [Swap::new(name("record"), doc("record.json"), body(b"v1"))],
        [name("record")],
    )
    .expect_err("write beneath a removed node");
    let text = contradiction.to_string();
    assert!(text.contains("RECORD"), "{text} names the node");
    assert!(text.contains("RECORD.JSON"), "{text} names the document");
}

// -- Applying a node removal ---------------------------------------------------

#[test]
fn an_empty_node_is_removed_from_every_partition() {
    let (_vfs, store) = fresh_store();
    let node = name("record/aa");
    let d = doc("record.json");
    seed(&store, &node, &d, b"v1");
    // Empty it, so the node itself is all that remains.
    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(&node, d.clone())),
            &nonce("empty"),
        )
        .expect("empty the node");
    assert!(
        node_exists(&_vfs, &store, &node),
        "the node outlives its document"
    );

    let outcome = store
        .apply_transaction(&remove_node(&node), &nonce("t1"))
        .expect("apply");
    assert_eq!(outcome, TransactionOutcome::Committed);
    assert!(!node_exists(&_vfs, &store, &node));
    // Every partition, not just the one a reader happens to consult.
    for index in 0..3 {
        assert!(
            !node_entries(&_vfs, &store, index).contains(&OsString::from("AA")),
            "partition {index} still holds the node directory"
        );
    }
}

#[test]
fn the_last_document_and_its_node_go_in_one_transaction() {
    // The case that decided `BG-6`'s shape. A pre-transaction emptiness check would
    // refuse exactly this -- the node is *not* empty when the transaction starts -- which
    // is why emptiness is judged against the post-transaction state instead.
    let (_vfs, store) = fresh_store();
    let node = name("pending-work/0001");
    let d = doc("entry");
    seed(&store, &node, &d, b"v1");

    let swaps = SwapSet::with_node_removals([Swap::remove(&node, d.clone())], [node.clone()])
        .expect("valid");
    let outcome = store
        .apply_transaction(&swaps, &nonce("t1"))
        .expect("apply");

    assert_eq!(outcome, TransactionOutcome::Committed);
    assert!(!node_exists(&_vfs, &store, &node));
    assert_eq!(store.read_body(&node, &d).unwrap(), None);
}

#[test]
fn a_node_still_holding_a_document_is_refused() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    seed(&store, &node, &doc("record.json"), b"v1");

    let outcome = store
        .apply_transaction(&remove_node(&node), &nonce("t1"))
        .expect("apply");

    assert_eq!(
        outcome,
        TransactionOutcome::Rejected {
            node: "RECORD".to_owned(),
            // A node removal names no document, which is why the field is optional.
            document: None,
            expected: RejectedPrecondition::NodeNotEmpty {
                documents: vec!["RECORD.JSON".to_owned()],
                nodes: Vec::new(),
            },
            current: None,
        }
    );
    assert!(outcome.is_rejected());
    assert!(
        node_exists(&_vfs, &store, &node),
        "the refused node is untouched"
    );
}

#[test]
fn a_node_still_holding_a_child_node_is_refused() {
    let (_vfs, store) = fresh_store();
    seed(&store, &name("record/aa"), &doc("record.json"), b"v1");

    let outcome = store
        .apply_transaction(&remove_node(&name("record")), &nonce("t1"))
        .expect("apply");

    assert_eq!(
        outcome,
        TransactionOutcome::Rejected {
            node: "RECORD".to_owned(),
            document: None,
            expected: RejectedPrecondition::NodeNotEmpty {
                documents: Vec::new(),
                // Reported as a full path, matching `list_children`.
                nodes: vec!["RECORD/AA".to_owned()],
            },
            current: None,
        }
    );
}

#[test]
fn a_node_and_its_child_go_in_one_transaction_in_either_order() {
    // The child is gone by the time the parent's removal runs, so the parent ends up empty.
    for parent_first in [true, false] {
        let (vfs, store) = fresh_store();
        let parent = name("record");
        let child = name("record/aa");
        let d = doc("record.json");
        seed(&store, &child, &d, b"v1");

        let removals = if parent_first {
            vec![parent.clone(), child.clone()]
        } else {
            vec![child.clone(), parent.clone()]
        };
        let swaps =
            SwapSet::with_node_removals([Swap::remove(&child, d.clone())], removals).unwrap();
        let outcome = store
            .apply_transaction(&swaps, &nonce("t1"))
            .expect("apply");

        assert_eq!(outcome, TransactionOutcome::Committed);
        assert!(!node_exists(&vfs, &store, &child));
        assert!(!node_exists(&vfs, &store, &parent));
    }
}

#[test]
fn a_child_removed_alongside_its_parent_must_itself_be_empty() {
    let (vfs, store) = fresh_store();
    let parent = name("record");
    let child = name("record/aa");
    seed(&store, &child, &doc("record.json"), b"v1");

    let swaps = SwapSet::with_node_removals([], [parent.clone(), child.clone()]).unwrap();
    let outcome = store
        .apply_transaction(&swaps, &nonce("t1"))
        .expect("apply");

    assert!(outcome.is_rejected());
    assert!(node_exists(&vfs, &store, &child));
    assert!(node_exists(&vfs, &store, &parent));
}

#[test]
fn a_node_holding_only_an_unrecognized_entry_is_refused_before_begin() {
    // The listing skips it, so the node looks empty; the non-recursive remove would not.
    let (vfs, store) = fresh_store();
    let node = name("record/aa");
    let d = doc("record.json");
    seed(&store, &node, &d, b"v1");
    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(&node, d.clone())),
            &nonce("e"),
        )
        .unwrap();
    vfs.write(
        &namespace_root(&store, 1)
            .join("RECORD")
            .join("AA")
            .join(".foreign"),
        b"not ours",
    )
    .unwrap();

    let outcome = store
        .apply_transaction(&remove_node(&node), &nonce("t1"))
        .expect("apply");

    assert_eq!(
        outcome,
        TransactionOutcome::Rejected {
            node: "RECORD/AA".to_owned(),
            document: None,
            expected: RejectedPrecondition::NodeHoldsUnrecognizedEntries {
                entries: vec![".foreign".to_owned()],
            },
            current: None,
        }
    );
    assert!(!store.recovery_required());
    assert!(node_exists(&vfs, &store, &node));
}

#[test]
fn the_rejection_names_everything_that_would_remain() {
    // The reason emptiness carries a payload at all: "not empty" alone leaves a caller
    // guessing which of two quite different mistakes they made.
    let (_vfs, store) = fresh_store();
    let node = name("record");
    seed(&store, &node, &doc("bbb"), b"v1");
    seed(&store, &node, &doc("aaa"), b"v1");
    seed(&store, &name("record/child"), &doc("record.json"), b"v1");

    let outcome = store
        .apply_transaction(&remove_node(&node), &nonce("t1"))
        .expect("apply");

    let TransactionOutcome::Rejected { expected, .. } = outcome else {
        panic!("expected a rejection");
    };
    assert_eq!(
        expected,
        RejectedPrecondition::NodeNotEmpty {
            // Sorted, so the report does not depend on directory-read order.
            documents: vec!["AAA".to_owned(), "BBB".to_owned()],
            nodes: vec!["RECORD/CHILD".to_owned()],
        }
    );
}

#[test]
fn a_document_this_transaction_writes_keeps_the_node_from_being_removed() {
    // The post-transaction rule cuts both ways. A construction check already refuses a
    // write beneath the removed node, so this exercises the accounting at a *sibling*
    // node that the removal does not contain -- the emptiness arithmetic must still be
    // about the removed node alone.
    let (_vfs, store) = fresh_store();
    let node = name("record");
    seed(&store, &node, &doc("record.json"), b"v1");

    let swaps = SwapSet::with_node_removals(
        [Swap::new(name("other"), doc("record.json"), body(b"v1"))],
        [node.clone()],
    )
    .expect("a sibling write is allowed");
    let outcome = store
        .apply_transaction(&swaps, &nonce("t1"))
        .expect("apply");

    assert!(outcome.is_rejected(), "RECORD still holds its document");
    // All-or-nothing: the sibling write did not land either.
    assert_eq!(
        store.read_body(name("other"), &doc("record.json")).unwrap(),
        None
    );
}

#[test]
fn removing_an_absent_node_succeeds() {
    // Specified, not incidental: `D53` recovery replays a prepared transaction, so a
    // removal must be safe to apply twice. Making the second one an error would make
    // a crash between apply and `end` unrecoverable.
    let (_vfs, store) = fresh_store();
    let outcome = store
        .apply_transaction(&remove_node(&name("never/existed")), &nonce("t1"))
        .expect("apply");
    assert_eq!(outcome, TransactionOutcome::Committed);
}

#[test]
fn a_node_removal_is_idempotent_when_applied_twice() {
    let (_vfs, store) = fresh_store();
    let node = name("record/aa");
    let d = doc("record.json");
    seed(&store, &node, &d, b"v1");
    store
        .apply_transaction(&SwapSet::single(Swap::remove(&node, d)), &nonce("empty"))
        .expect("empty the node");

    for txn in ["t1", "t2"] {
        let outcome = store
            .apply_transaction(&remove_node(&node), &nonce(txn))
            .expect("apply");
        assert_eq!(outcome, TransactionOutcome::Committed, "pass {txn}");
        assert!(!node_exists(&_vfs, &store, &node));
    }
}

#[test]
fn a_rejected_node_removal_leaves_the_whole_transaction_undone() {
    // All-or-nothing across the two effect kinds, not just within one of them.
    let (_vfs, store) = fresh_store();
    let occupied = name("record");
    seed(&store, &occupied, &doc("record.json"), b"v1");

    let swaps = SwapSet::with_node_removals(
        [Swap::new(name("other"), doc("record.json"), body(b"new"))],
        [occupied.clone()],
    )
    .expect("valid");
    // The seed already wrote its own transaction, so "no log record" means no *new* one.
    let before = store.commit_log().read().unwrap().len();
    let outcome = store
        .apply_transaction(&swaps, &nonce("t1"))
        .expect("apply");

    assert!(outcome.is_rejected());
    assert!(node_exists(&_vfs, &store, &occupied));
    assert_eq!(
        store.read_body(name("other"), &doc("record.json")).unwrap(),
        None,
        "nothing from the set landed"
    );
    assert_eq!(
        store.commit_log().read().unwrap().len(),
        before,
        "a rejected transaction writes no log record"
    );
}

#[test]
fn removing_a_node_leaves_its_siblings_alone() {
    let (_vfs, store) = fresh_store();
    let going = name("tickets/aa");
    let staying = name("tickets/ab");
    let d = doc("record.json");
    seed(&store, &going, &d, b"v1");
    seed(&store, &staying, &d, b"v1");
    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(&going, d.clone())),
            &nonce("empty"),
        )
        .expect("empty the node");

    store
        .apply_transaction(&remove_node(&going), &nonce("t1"))
        .expect("apply");

    assert!(!node_exists(&_vfs, &store, &going));
    assert!(node_exists(&_vfs, &store, &staying));
    assert_eq!(store.read_body(&staying, &d).unwrap(), Some(body(b"v1")));
}

#[test]
fn removing_a_child_node_leaves_its_parent_behind() {
    // A node is not a durable object in its own right, so emptying a parent does not
    // cascade -- the parent's own removal is a separate decision the caller must make.
    let (_vfs, store) = fresh_store();
    let child = name("record/aa");
    let d = doc("record.json");
    seed(&store, &child, &d, b"v1");
    store
        .apply_transaction(&SwapSet::single(Swap::remove(&child, d)), &nonce("empty"))
        .expect("empty the node");

    store
        .apply_transaction(&remove_node(&child), &nonce("t1"))
        .expect("apply");

    assert!(!node_exists(&_vfs, &store, &child));
    assert!(
        store
            .list_children(name("record"))
            .expect("list")
            .is_empty(),
        "the parent is empty but still present"
    );
}

#[test]
fn a_removed_node_can_be_written_again() {
    // Removal takes a directory, not a name: nothing about the path is retired.
    let (_vfs, store) = fresh_store();
    let node = name("record/aa");
    let d = doc("record.json");
    seed(&store, &node, &d, b"v1");
    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(&node, d.clone())),
            &nonce("empty"),
        )
        .expect("empty the node");
    store
        .apply_transaction(&remove_node(&node), &nonce("t1"))
        .expect("remove");

    store
        .apply_transaction(
            &SwapSet::single(Swap::new(&node, d.clone(), body(b"v2"))),
            &nonce("t2"),
        )
        .expect("write again");

    assert!(node_exists(&_vfs, &store, &node));
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));
}

// -- The `D53` crash matrix ----------------------------------------------------

#[test]
fn recovery_rolls_forward_a_prepared_node_removal() {
    // The whole reason the commit log carries node removals: past the commit point the
    // removal is promised, and only the log can keep that promise across a crash.
    let (vfs, store) = fresh_store();
    let node = name("record/aa");
    let d = doc("record.json");
    seed(&store, &node, &d, b"v1");
    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(&node, d.clone())),
            &nonce("empty"),
        )
        .expect("empty the node");

    let txn = nonce("t1");
    let swaps = remove_node(&node);
    store.commit_log().append(&begin(&txn)).unwrap();
    store.stage_transaction_for_test(&txn, &swaps).unwrap();
    store
        .commit_log()
        .append(&LogRecord::Prepare {
            txn: txn.clone(),
            prepared: prepared(&swaps),
        })
        .unwrap();

    // Crash point: prepared, but the directory is still there.
    assert!(node_exists(&vfs, &store, &node));

    let recovered = reopen(&vfs, store).unwrap();
    assert!(
        !node_exists(&vfs, &recovered, &node),
        "the prepared node removal is rolled forward"
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
fn recovery_rolls_back_a_begun_but_unprepared_node_removal() {
    let (vfs, store) = fresh_store();
    let node = name("record/aa");
    let d = doc("record.json");
    seed(&store, &node, &d, b"v1");
    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(&node, d.clone())),
            &nonce("empty"),
        )
        .expect("empty the node");

    let txn = nonce("t1");
    store.commit_log().append(&begin(&txn)).unwrap();

    let recovered = reopen(&vfs, store).unwrap();
    assert!(
        node_exists(&vfs, &recovered, &node),
        "an unprepared removal never happened"
    );
}

#[test]
fn recovery_rolls_forward_a_mixed_prepared_transaction() {
    // Document effects and node removals in one prepared record, recovered together.
    let (vfs, store) = fresh_store();
    let going = name("record/aa");
    let d = doc("record.json");
    seed(&store, &going, &d, b"v1");

    let txn = nonce("t1");
    let swaps = SwapSet::with_node_removals(
        [
            Swap::remove(&going, d.clone()),
            Swap::new(name("other"), doc("record.json"), body(b"v2")),
        ],
        [going.clone()],
    )
    .expect("valid");
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
        recovered.read_body(&going, &d).unwrap(),
        None,
        "the document removal rolled forward"
    );
    assert!(
        !node_exists(&vfs, &recovered, &going),
        "the node removal rolled forward"
    );
    assert_eq!(
        recovered
            .read_body(name("other"), &doc("record.json"))
            .unwrap(),
        Some(body(b"v2")),
        "the write rolled forward"
    );
}

// -- The commit log's durable shape --------------------------------------------

#[test]
fn a_node_removal_round_trips_through_the_commit_log() {
    let (_vfs, store) = fresh_store();
    let log = store.commit_log();
    let swaps = SwapSet::with_node_removals(
        [Swap::remove(name("record"), doc("record.json"))],
        [name("record"), name("other/aa")],
    )
    .expect("valid");
    log.append(&LogRecord::Prepare {
        txn: nonce("t1"),
        prepared: prepared(&swaps),
    })
    .unwrap();

    let records = log.read().expect("read back");
    let LogRecord::Prepare { prepared: read, .. } = &records[0] else {
        panic!("expected a prepare record");
    };
    assert_eq!(read, &prepared(&swaps));
    assert_eq!(read.node_removals(), swaps.node_removals());
}

#[test]
fn a_prepare_record_without_node_removals_still_reads() {
    // The wire field is defaulted and omitted when empty, so a record written before
    // node removals existed reads back as the no-removals transaction it was.
    let (vfs, store) = fresh_store();
    let log = store.commit_log();
    log.append(&LogRecord::Prepare {
        txn: nonce("t1"),
        prepared: prepared(&SwapSet::single(Swap::new(
            name("record"),
            doc("record.json"),
            body(b"v1"),
        ))),
    })
    .unwrap();

    let raw = String::from_utf8(vfs.read(log.path()).unwrap()).unwrap();
    assert!(
        !raw.contains("remove_nodes"),
        "an empty removal list is not written: {raw}"
    );

    let records = log.read().expect("read back");
    let LogRecord::Prepare { prepared, .. } = &records[0] else {
        panic!("expected a prepare record");
    };
    assert!(prepared.node_removals().is_empty());
}
