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
