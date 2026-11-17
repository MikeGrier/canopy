// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! `Swap` / `SwapSet` construction and duplicate-target rejection.

use super::*;

// -- Swap / SwapSet ------------------------------------------------------------

#[test]
fn unconditional_swap_has_no_expected_digest() {
    let swap = Swap::new(name("aaa"), doc("record"), body(b"one"));
    assert_eq!(swap.node().to_string(), "AAA");
    assert_eq!(swap.document().as_str(), "RECORD");
    assert_eq!(
        swap.body().expect("a write carries a body").as_bytes(),
        b"one"
    );
    assert_eq!(swap.expected(), None);
}

#[test]
fn conditional_swap_carries_the_expected_digest() {
    let expected = ContentDigest::of(b"old");
    let swap = Swap::conditional(name("bbb"), doc("record"), body(b"new"), expected);
    assert_eq!(swap.expected(), Some(expected));
}

#[test]
fn single_swap_set_has_one_element() {
    let set = SwapSet::single(Swap::new(name("aaa"), doc("record"), body(b"x")));
    assert_eq!(set.len(), 1);
    assert!(!set.is_empty());
    assert_eq!(set.swaps()[0].node().to_string(), "AAA");
    assert_eq!(set.swaps()[0].document().as_str(), "RECORD");
}

#[test]
fn multi_document_swap_set_preserves_order() {
    let set = SwapSet::new([
        Swap::new(name("aaa"), doc("record"), body(b"1")),
        Swap::new(name("bbb"), doc("record"), body(b"2")),
        Swap::new(name("ccc"), doc("record"), body(b"3")),
    ])
    .expect("distinct targets");
    assert_eq!(set.len(), 3);
    let nodes: Vec<String> = set.swaps().iter().map(|s| s.node().to_string()).collect();
    assert_eq!(nodes, ["AAA", "BBB", "CCC"]);
}

#[test]
fn two_documents_at_one_node_are_distinct_targets() {
    // record.json and engine at the same node are two swaps, not a duplicate (D62).
    let set = SwapSet::new([
        Swap::new(name("aaa"), doc("record"), body(b"1")),
        Swap::new(name("aaa"), doc("engine"), body(b"2")),
    ])
    .expect("distinct (node, document) targets");
    assert_eq!(set.len(), 2);
    let docs: Vec<&str> = set.swaps().iter().map(|s| s.document().as_str()).collect();
    assert_eq!(docs, ["RECORD", "ENGINE"]);
}

#[test]
fn empty_swap_set_is_a_permitted_no_op() {
    let set = SwapSet::new(std::iter::empty()).expect("empty is allowed");
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
}

#[test]
fn swap_set_rejects_a_duplicate_target() {
    let err = SwapSet::new([
        Swap::new(name("aaa"), doc("record"), body(b"1")),
        Swap::new(name("aaa"), doc("record"), body(b"2")),
    ])
    .expect_err("duplicate (node, document) target");
    assert_eq!(
        err,
        SwapSetError::DuplicateTarget {
            node: "AAA".to_owned(),
            document: "RECORD".to_owned(),
        }
    );
}

#[test]
fn swap_set_duplicate_detection_uses_the_normalized_name() {
    // "abc" and "ABC" normalize to the same node, and the document also folds, so
    // the two swaps collide on one (node, document) target.
    let err = SwapSet::new([
        Swap::new(name("abc"), doc("record"), body(b"1")),
        Swap::new(name("ABC"), doc("RECORD"), body(b"2")),
    ])
    .expect_err("case-folded duplicate");
    assert_eq!(
        err,
        SwapSetError::DuplicateTarget {
            node: "ABC".to_owned(),
            document: "RECORD".to_owned(),
        }
    );
}

#[test]
fn swap_set_error_displays_the_offending_target() {
    let err = SwapSetError::DuplicateTarget {
        node: "AAA".to_owned(),
        document: "record".to_owned(),
    };
    assert_eq!(
        format!("{err}"),
        "swap set names the document \"record\" at node \"AAA\" more than once"
    );
}

// -- Opaque transactions -----------------------------------------------------

#[test]
fn an_empty_builder_produces_an_empty_transaction() {
    let (_vfs, store) = seeded_store();
    assert!(store.transaction().build().unwrap().is_empty());
}

#[test]
fn a_read_makes_a_transaction_non_empty() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    assert_eq!(
        builder.read_document(name("alpha"), &doc("one")).unwrap(),
        None
    );
    assert!(!builder.build().unwrap().is_empty());
}

#[test]
fn a_transaction_writes_multiple_documents_atomically() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"1")).write(
        name("alpha"),
        doc("two"),
        body(b"2"),
    );
    let transaction = builder.build().unwrap();
    let outcome = store
        .transact(&transaction, crate::TransactionOptions)
        .unwrap();
    assert!(outcome.is_committed());
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("1"));
    assert_eq!(text(&store, "alpha", "two").as_deref(), Some("2"));
}

#[test]
fn a_read_check_rejects_after_the_document_changes() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"before")))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    let mut builder = store.transaction();
    builder.read_document(name("alpha"), &doc("one")).unwrap();
    let transaction = builder.build().unwrap();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"after")))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    let outcome = store
        .transact(&transaction, crate::TransactionOptions)
        .unwrap();
    assert!(outcome.is_rejected());
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("after"));
}

#[test]
fn an_absence_check_rejects_after_the_document_is_created() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder.read_document(name("alpha"), &doc("one")).unwrap();
    let transaction = builder.build().unwrap();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"created")))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(
        store
            .transact(&transaction, crate::TransactionOptions)
            .unwrap()
            .is_rejected()
    );
}

#[test]
fn a_check_can_guard_a_write_at_another_document() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("guard"), body(b"stable")))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    let mut builder = store.transaction();
    builder.read_document(name("alpha"), &doc("guard")).unwrap();
    builder.write(name("alpha"), doc("value"), body(b"written"));
    let transaction = builder.build().unwrap();
    assert!(
        store
            .transact(&transaction, crate::TransactionOptions)
            .unwrap()
            .is_committed()
    );
    assert_eq!(text(&store, "alpha", "value").as_deref(), Some("written"));
}

#[test]
fn builder_duplicate_targets_are_rejected_at_build() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"1")).write(
        name("ALPHA"),
        doc("ONE"),
        body(b"2"),
    );
    assert_eq!(
        builder.build().unwrap_err(),
        SwapSetError::DuplicateTarget {
            node: "ALPHA".to_owned(),
            document: "ONE".to_owned(),
        }
    );
}

#[test]
fn builder_duplicate_node_removals_are_rejected_at_build() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder
        .remove_node(name("alpha"))
        .remove_node(name("ALPHA"));
    assert_eq!(
        builder.build().unwrap_err(),
        SwapSetError::DuplicateNodeRemoval {
            node: "ALPHA".to_owned(),
        }
    );
}

#[test]
fn builder_refuses_a_write_beneath_a_removed_node() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder
        .write(name("alpha/child"), doc("one"), body(b"1"))
        .remove_node(name("alpha"));
    assert!(matches!(
        builder.build(),
        Err(SwapSetError::WriteBeneathRemovedNode { .. })
    ));
}

#[test]
fn building_and_holding_a_transaction_does_not_hold_writer_exclusion() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder.write(name("alpha"), doc("one"), body(b"transaction"));
    store
        .update_document(Swap::new(name("beta"), doc("one"), body(b"other")))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    let transaction = builder.build().unwrap();
    assert!(
        store
            .transact(&transaction, crate::TransactionOptions)
            .unwrap()
            .is_committed()
    );
}

#[test]
fn a_check_only_transaction_commits_without_changing_data() {
    let (_vfs, store) = seeded_store();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"kept")))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    let mut builder = store.transaction();
    builder.read_document(name("alpha"), &doc("one")).unwrap();
    let transaction = builder.build().unwrap();
    assert!(
        store
            .transact(&transaction, crate::TransactionOptions)
            .unwrap()
            .is_committed()
    );
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("kept"));
}

// -- Store-evaluated primitives ---------------------------------------------

#[test]
fn append_creates_an_absent_document_from_empty_bytes() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder.append(name("alpha"), doc("one"), b"first".to_vec());
    let transaction = builder.build().unwrap();
    assert!(
        store
            .transact(&transaction, crate::TransactionOptions)
            .unwrap()
            .is_committed()
    );
    assert_eq!(
        store.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(b"first"))
    );
}

#[test]
fn append_preserves_existing_binary_bytes_in_order() {
    let (_vfs, store) = seeded_store();
    let _ = store
        .update_document(Swap::new(name("alpha"), doc("one"), body(&[0, 1, 255])))
        .unwrap();
    let mut builder = store.transaction();
    builder.append(name("alpha"), doc("one"), vec![2, 0, 254]);
    let transaction = builder.build().unwrap();
    let _ = store
        .transact(&transaction, crate::TransactionOptions)
        .unwrap();
    assert_eq!(
        store.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(&[0, 1, 255, 2, 0, 254]))
    );
}

#[test]
fn appends_to_two_documents_land_together() {
    let (_vfs, store) = seeded_store();
    let _ = store
        .apply_transaction(
            &SwapSet::new([
                Swap::new(name("alpha"), doc("one"), body(b"a")),
                Swap::new(name("alpha"), doc("two"), body(b"b")),
            ])
            .unwrap(),
        )
        .unwrap();
    let mut builder = store.transaction();
    builder
        .append(name("alpha"), doc("one"), b"1".to_vec())
        .append(name("alpha"), doc("two"), b"2".to_vec());
    let transaction = builder.build().unwrap();
    assert!(
        store
            .transact(&transaction, crate::TransactionOptions)
            .unwrap()
            .is_committed()
    );
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("a1"));
    assert_eq!(text(&store, "alpha", "two").as_deref(), Some("b2"));
}

#[test]
fn append_input_rejects_exactly_one_byte_over_its_limit() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder.append(
        name("alpha"),
        doc("one"),
        vec![0; crate::MAX_APPEND_BYTES + 1],
    );
    assert!(matches!(
        builder.build(),
        Err(SwapSetError::AppendInputTooLarge {
            bytes,
            limit,
            ..
        }) if bytes == crate::MAX_APPEND_BYTES + 1 && limit == crate::MAX_APPEND_BYTES
    ));
}

#[test]
fn append_accepts_an_input_exactly_at_its_limit() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder.append(name("alpha"), doc("one"), vec![7; crate::MAX_APPEND_BYTES]);
    let transaction = builder.build().unwrap();
    let _ = store
        .transact(&transaction, crate::TransactionOptions)
        .unwrap();
    assert_eq!(
        store
            .read_body(name("alpha"), &doc("one"))
            .unwrap()
            .unwrap()
            .as_bytes(),
        &vec![7; crate::MAX_APPEND_BYTES]
    );
}

#[test]
fn append_rejects_a_result_one_byte_over_its_limit_without_changing_the_document() {
    let (_vfs, store) = seeded_store();
    let original = vec![3; crate::MAX_APPEND_RESULT_BYTES];
    let _ = store
        .update_document(Swap::new(name("alpha"), doc("one"), body(&original)))
        .unwrap();
    let mut builder = store.transaction();
    builder.append(name("alpha"), doc("one"), vec![4]);
    let transaction = builder.build().unwrap();
    assert!(matches!(
        store.transact(&transaction, crate::TransactionOptions),
        Err(StoreError::AppendResultTooLarge {
            bytes,
            limit,
            ..
        }) if bytes == crate::MAX_APPEND_RESULT_BYTES + 1
            && limit == crate::MAX_APPEND_RESULT_BYTES
    ));
    assert_eq!(
        store
            .read_body(name("alpha"), &doc("one"))
            .unwrap()
            .unwrap()
            .as_bytes(),
        original
    );
}

#[test]
fn append_accepts_a_result_exactly_at_its_limit() {
    let (_vfs, store) = seeded_store();
    let original = vec![5; crate::MAX_APPEND_RESULT_BYTES - 1];
    let _ = store
        .update_document(Swap::new(name("alpha"), doc("one"), body(&original)))
        .unwrap();
    let mut builder = store.transaction();
    builder.append(name("alpha"), doc("one"), vec![6]);
    let transaction = builder.build().unwrap();
    let _ = store
        .transact(&transaction, crate::TransactionOptions)
        .unwrap();
    let result = store
        .read_body(name("alpha"), &doc("one"))
        .unwrap()
        .unwrap();
    assert_eq!(result.as_bytes().len(), crate::MAX_APPEND_RESULT_BYTES);
    assert_eq!(result.as_bytes()[result.as_bytes().len() - 1], 6);
}

#[test]
fn append_and_write_cannot_target_the_same_document() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder
        .append(name("alpha"), doc("one"), b"a".to_vec())
        .write(name("ALPHA"), doc("ONE"), body(b"b"));
    assert!(matches!(
        builder.build(),
        Err(SwapSetError::DuplicateTarget { .. })
    ));
}

#[test]
fn append_beneath_a_removed_node_is_refused_at_build() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder
        .append(name("alpha/child"), doc("one"), b"a".to_vec())
        .remove_node(name("alpha"));
    assert!(matches!(
        builder.build(),
        Err(SwapSetError::WriteBeneathRemovedNode { .. })
    ));
}

#[test]
fn a_stale_check_rejects_an_append_without_changing_other_targets() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder.read_document(name("alpha"), &doc("guard")).unwrap();
    builder
        .append(name("alpha"), doc("one"), b"a".to_vec())
        .write(name("alpha"), doc("two"), body(b"b"));
    let transaction = builder.build().unwrap();
    let _ = store
        .update_document(Swap::new(name("alpha"), doc("guard"), body(b"new")))
        .unwrap();
    assert!(
        store
            .transact(&transaction, crate::TransactionOptions)
            .unwrap()
            .is_rejected()
    );
    assert_eq!(store.read_body(name("alpha"), &doc("one")).unwrap(), None);
    assert_eq!(store.read_body(name("alpha"), &doc("two")).unwrap(), None);
}

#[test]
fn concurrent_append_transactions_lose_no_bytes() {
    let (_vfs, store) = seeded_store();
    let (threads, each) = (8, 32);
    thread::scope(|scope| {
        for actor in 0..threads {
            let store = store.clone();
            scope.spawn(move || {
                for _ in 0..each {
                    let mut builder = store.transaction();
                    builder.append(name("alpha"), doc("log"), vec![actor as u8]);
                    let _ = store
                        .transact(&builder.build().unwrap(), crate::TransactionOptions)
                        .unwrap();
                }
            });
        }
    });
    let result = store
        .read_body(name("alpha"), &doc("log"))
        .unwrap()
        .unwrap();
    assert_eq!(result.as_bytes().len(), threads * each);
    for actor in 0..threads {
        assert_eq!(
            result
                .as_bytes()
                .iter()
                .filter(|&&byte| byte == actor as u8)
                .count(),
            each
        );
    }
}

#[test]
fn transact_does_not_retry_a_rejected_transaction() {
    let (_vfs, store) = seeded_store();
    let mut builder = store.transaction();
    builder.read_document(name("alpha"), &doc("one")).unwrap();
    builder.write(name("alpha"), doc("one"), body(b"attempt"));
    let transaction = builder.build().unwrap();
    store
        .update_document(Swap::new(name("alpha"), doc("one"), body(b"winner")))
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    let outcome = store
        .transact(&transaction, crate::TransactionOptions)
        .unwrap();
    assert!(outcome.is_rejected());
    assert_eq!(text(&store, "alpha", "one").as_deref(), Some("winner"));
}
