// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! TXN-6: the public store surface -- the generic, ticket-free contract
//! (`D8`, `D53`, `D54`).

use super::*;

// -- Public store surface: the generic ticket-free contract (TXN-6, D8/D53/D54) ----

#[test]
fn update_document_commits_a_single_document() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    let outcome = store
        .update_document_as(
            Swap::new(node.clone(), d.clone(), body(b"payload")),
            &nonce("u1"),
        )
        .unwrap();
    assert_eq!(outcome, TransactionOutcome::Committed);
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"payload")));
}

#[test]
fn update_document_is_read_back_by_the_quorum_read() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    store
        .update_document_as(
            Swap::new(node.clone(), d.clone(), body(b"v1")),
            &nonce("u1"),
        )
        .unwrap();
    let read = store.read_body(&node, &d).unwrap();
    assert_eq!(read, Some(body(b"v1")));
}

#[test]
fn update_document_overwrites_a_prior_value_atomically() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    store
        .update_document_as(
            Swap::new(node.clone(), d.clone(), body(b"v1")),
            &nonce("u1"),
        )
        .unwrap();
    store
        .update_document_as(
            Swap::new(node.clone(), d.clone(), body(b"v2")),
            &nonce("u2"),
        )
        .unwrap();
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));
    // The overwrite replaces the one document rather than adding a second.
    assert_eq!(store.list_documents().unwrap().len(), 1);
}

#[test]
fn update_document_conditional_commits_when_the_precondition_holds() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    store
        .update_document_as(
            Swap::new(node.clone(), d.clone(), body(b"v1")),
            &nonce("u1"),
        )
        .unwrap();
    let outcome = store
        .update_document_as(
            Swap::conditional(node.clone(), d.clone(), body(b"v2"), body(b"v1").digest()),
            &nonce("u2"),
        )
        .unwrap();
    assert_eq!(outcome, TransactionOutcome::Committed);
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));
}

#[test]
fn update_document_conditional_rejects_on_a_digest_mismatch_and_changes_nothing() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    store
        .update_document_as(
            Swap::new(node.clone(), d.clone(), body(b"v1")),
            &nonce("u1"),
        )
        .unwrap();
    let outcome = store
        .update_document_as(
            Swap::conditional(
                node.clone(),
                d.clone(),
                body(b"v2"),
                body(b"stale").digest(),
            ),
            &nonce("u2"),
        )
        .unwrap();
    assert!(outcome.is_rejected());
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v1")));
}

#[test]
fn update_document_conditional_rejects_when_the_document_is_absent() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    let outcome = store
        .update_document_as(
            Swap::conditional(
                node.clone(),
                d.clone(),
                body(b"v1"),
                body(b"anything").digest(),
            ),
            &nonce("u1"),
        )
        .unwrap();
    match outcome {
        TransactionOutcome::Rejected { current, .. } => assert_eq!(current, None),
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert!(store.list_documents().unwrap().is_empty());
}

#[test]
fn list_documents_on_an_empty_store_is_empty() {
    let (_vfs, store) = fresh_store();
    assert!(store.list_documents().unwrap().is_empty());
}

#[test]
fn list_documents_returns_every_committed_document_sorted() {
    let (_vfs, store) = fresh_store();
    let d = doc("record.json");
    // Insert in non-sorted order; the traversal returns the union sorted.
    store
        .update_document_as(
            Swap::new(name("charlie"), d.clone(), body(b"c")),
            &nonce("u1"),
        )
        .unwrap();
    store
        .update_document_as(
            Swap::new(name("alpha"), d.clone(), body(b"a")),
            &nonce("u2"),
        )
        .unwrap();
    store
        .update_document_as(
            Swap::new(name("bravo"), d.clone(), body(b"b")),
            &nonce("u3"),
        )
        .unwrap();
    let listed = store.list_documents().unwrap();
    assert_eq!(
        listed,
        vec![
            (name("alpha"), d.clone()),
            (name("bravo"), d.clone()),
            (name("charlie"), d.clone()),
        ]
    );
}

#[test]
fn list_documents_reflects_a_multi_document_transaction() {
    let (_vfs, store) = fresh_store();
    let a = name("alpha");
    let b = name("bravo");
    let d = doc("record.json");
    let swaps = SwapSet::new([
        Swap::new(a.clone(), d.clone(), body(b"a")),
        Swap::new(b.clone(), d.clone(), body(b"b")),
    ])
    .expect("distinct targets");
    store.apply_transaction_as(&swaps, &nonce("t1")).unwrap();
    assert_eq!(
        store.list_documents().unwrap(),
        vec![(a, d.clone()), (b, d)]
    );
}

#[test]
fn list_documents_skips_the_shire_marker_and_swept_staging() {
    let (_vfs, store) = fresh_store();
    let node = name("record");
    let d = doc("record.json");
    // A committed update leaves the .shire marker in place and sweeps its staging tree;
    // the traversal reports only the real document, never those reserved entries.
    store
        .update_document_as(
            Swap::new(node.clone(), d.clone(), body(b"v1")),
            &nonce("u1"),
        )
        .unwrap();
    assert_eq!(store.list_documents().unwrap(), vec![(node, d)]);
}
#[test]
fn list_documents_skips_reserved_dotfile_shards() {
    let (vfs, store) = fresh_store();
    let hidden_node = namespace_root(&store, 0).join(".hidden").join("RECORD");
    vfs.create_dir_all(&hidden_node).unwrap();
    vfs.write(&hidden_node.join("RECORD.JSON"), b"foreign")
        .unwrap();

    assert!(store.list_documents().unwrap().is_empty());
}

#[test]
fn list_documents_skips_non_directory_shard_entries() {
    let (vfs, store) = fresh_store();
    vfs.write(&namespace_root(&store, 0).join("foreign"), b"not a shard")
        .unwrap();

    assert!(store.list_documents().unwrap().is_empty());
}
