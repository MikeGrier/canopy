// Copyright (c) Michael Grier.

//! `CVS-11`: pointer read and write over `namespace/` -- the majority, what it does not
//! buy, and the reporting obligation of `D326`.

use super::*;
use crate::body::ContentDigest;
use crate::naming::{Artifact, BlobName, BlobRef, PathHash};
use crate::pointer::Pointer;
use crate::read::{Contention, Read};
use crate::store::{Children, NAMESPACE_TREE_DIR};

fn node_of(path: &str) -> NodePath {
    NodePath::parse(path).expect("valid node path")
}

fn doc_of(name: &str) -> DocumentName {
    DocumentName::new(name).expect("valid document name")
}

fn nonce_of(token: &str) -> Nonce {
    Nonce::new(token).expect("valid nonce")
}

/// A pointer for (`node`, `document`) naming a blob that would hold `body`.
fn pointer_for(node: &NodePath, document: &DocumentName, body: &[u8]) -> Pointer {
    let path_hash = PathHash::of(node, document);
    let name = BlobName::new(path_hash, ContentDigest::of(body), Artifact::Body);
    Pointer::new(path_hash, BlobRef::new(name, 2).unwrap())
}

/// Partition `index`'s entry file for (`node`, `document`).
fn entry_path(
    root: &std::path::Path,
    index: usize,
    node: &NodePath,
    document: &DocumentName,
) -> PathBuf {
    root.join(NAMESPACE_TREE_DIR)
        .join(format!("p{index}"))
        .join(node.relative_path())
        .join(document.as_str())
}

/// A store with a pointer already written for `TICKETS/A`'s `record.json`.
fn seeded(
    body: &[u8],
) -> (
    Arc<dyn Vfs>,
    PathBuf,
    Store,
    NodePath,
    DocumentName,
    Pointer,
) {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    let node = node_of("TICKETS/A");
    let document = doc_of("record.json");
    let pointer = pointer_for(&node, &document, body);
    store
        .write_pointer(&node, &document, &pointer, &nonce_of("n1"))
        .expect("write");
    (vfs, root, store, node, document, pointer)
}

// -- write and read ------------------------------------------------------------

#[test]
fn a_written_pointer_reads_back_unanimously() {
    let (_vfs, _root, store, node, document, pointer) = seeded(b"body");

    let read = store.read_pointer(&node, &document).expect("read");
    assert_eq!(read.pointer(), pointer);
    assert!(read.is_unanimous());
    assert_eq!(read.agreed(), store.pointer_replica_count());
    assert!(read.stale().is_empty());
    assert!(read.rejected().is_empty());
    assert!(read.missing().is_empty());
}

#[test]
fn a_pointer_lands_on_every_replica_at_the_client_visible_path() {
    // `namespace/` is laid out by path, which is what lets `CVS-12` walk it rather than
    // consult an index.
    let (vfs, root, store, node, document, pointer) = seeded(b"body");

    for index in 0..store.pointer_replica_count() {
        let bytes = vfs
            .read(&entry_path(&root, index, &node, &document))
            .unwrap();
        assert_eq!(bytes, pointer.render(), "partition {index}");
    }
}

#[test]
fn rewriting_a_pointer_replaces_every_replica() {
    let (_vfs, _root, store, node, document, _) = seeded(b"first");
    let second = pointer_for(&node, &document, b"second");
    store
        .write_pointer(&node, &document, &second, &nonce_of("n2"))
        .expect("rewrite");

    let read = store.read_pointer(&node, &document).expect("read");
    assert_eq!(read.pointer(), second);
    assert!(read.is_unanimous());
}

#[test]
fn a_successful_write_leaves_no_staging_behind() {
    let (vfs, root, store, _, _, _) = seeded(b"body");
    for index in 0..store.pointer_replica_count() {
        let staging = root
            .join(NAMESPACE_TREE_DIR)
            .join(format!("p{index}"))
            .join(".staging");
        assert!(vfs.read_dir(&staging).unwrap_or_default().is_empty());
    }
}

#[test]
fn init_creates_the_namespace_partitions() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    for index in 0..store.pointer_replica_count() {
        let partition = root.join(NAMESPACE_TREE_DIR).join(format!("p{index}"));
        assert!(vfs.metadata(&partition).unwrap().is_dir());
    }
}

#[test]
fn an_absent_document_is_unavailable_rather_than_invented() {
    let (_vfs, _root, store, _, _, _) = seeded(b"body");
    match store.read_pointer(&node_of("TICKETS/A"), &doc_of("absent.json")) {
        Err(StoreError::PointerUnavailable {
            missing,
            rejected,
            disagreeing,
            ..
        }) => {
            assert_eq!(missing, 3);
            assert_eq!(rejected, 0);
            assert_eq!(disagreeing, 0);
        }
        other => panic!("expected an unavailable pointer, got {other:?}"),
    }
}

// -- the majority (`D320`) ------------------------------------------------------

#[test]
fn a_minority_that_is_behind_is_stale_and_is_not_repaired() {
    // `D320`: a replica that parses cleanly but differs from the majority is behind, not
    // broken. It is reported, and the repair queue is never the mechanism that converges
    // it -- that is the transaction completing, which recovery drives.
    let (vfs, root, store, node, document, current) = seeded(b"current");
    let old = pointer_for(&node, &document, b"old");
    vfs.write(&entry_path(&root, 1, &node, &document), &old.render())
        .unwrap();

    let read = store.read_pointer(&node, &document).expect("read");
    assert_eq!(read.pointer(), current);
    assert_eq!(read.agreed(), 2);
    assert_eq!(read.stale(), &[1]);
    assert!(
        read.rejected().is_empty(),
        "a stale pointer is valid: it must not be reported as damage"
    );
}

#[test]
fn a_mid_apply_read_returns_the_old_value_and_does_not_regress() {
    // Exactly what the majority buys and what it does not (`D320`). With one replica
    // advanced, the majority still holds the previous pointer -- so the read returns the
    // old value, the *same* old value every time. It is defined and monotonic, never
    // atomic, and must not be mistaken for a commit barrier.
    let (vfs, root, store, node, document, old) = seeded(b"old");
    let new = pointer_for(&node, &document, b"new");
    vfs.write(&entry_path(&root, 0, &node, &document), &new.render())
        .unwrap();

    for _ in 0..8 {
        let read = store.read_pointer(&node, &document).expect("read");
        assert_eq!(read.pointer(), old, "the answer moved inside one window");
        assert_eq!(read.agreed(), 2);
    }

    // Once a majority has advanced, so does the answer -- and never back again.
    vfs.write(&entry_path(&root, 1, &node, &document), &new.render())
        .unwrap();
    for _ in 0..8 {
        assert_eq!(store.read_pointer(&node, &document).unwrap().pointer(), new);
    }
}

#[test]
fn a_majority_survives_every_intermediate_state_of_an_apply() {
    // Why `disagreeing` is a strange condition rather than a routine one: an apply moves
    // replicas one at a time, and with an odd count some majority exists at every step.
    let (vfs, root, store, node, document, old) = seeded(b"old");
    let new = pointer_for(&node, &document, b"new");

    let mut answers = Vec::new();
    for advanced in 0..=store.pointer_replica_count() {
        if advanced > 0 {
            vfs.write(
                &entry_path(&root, advanced - 1, &node, &document),
                &new.render(),
            )
            .unwrap();
        }
        answers.push(
            store
                .read_pointer(&node, &document)
                .expect("read")
                .pointer(),
        );
    }

    assert_eq!(answers, vec![old, old, new, new]);
}

#[test]
fn a_read_survives_a_minority_of_damaged_replicas() {
    let (vfs, root, store, node, document, pointer) = seeded(b"body");
    vfs.write(&entry_path(&root, 2, &node, &document), b"not a pointer\n")
        .unwrap();

    let read = store.read_pointer(&node, &document).expect("read");
    assert_eq!(read.pointer(), pointer);
    assert_eq!(read.rejected(), &[2]);
    assert_eq!(read.agreed(), 2);
}

#[test]
fn a_majority_of_damaged_replicas_leaves_the_pointer_undeterminable() {
    let (vfs, root, store, node, document, _) = seeded(b"body");
    for index in [1usize, 2] {
        vfs.write(&entry_path(&root, index, &node, &document), b"rot\n")
            .unwrap();
    }

    match store.read_pointer(&node, &document) {
        Err(StoreError::PointerUnavailable {
            rejected,
            missing,
            disagreeing,
            ..
        }) => {
            assert_eq!(rejected, 2);
            assert_eq!(missing, 0);
            assert_eq!(
                disagreeing, 1,
                "the one survivor could not carry a majority"
            );
        }
        other => panic!("expected an unavailable pointer, got {other:?}"),
    }
}

#[test]
fn three_replicas_holding_three_different_pointers_form_no_majority() {
    let (vfs, root, store, node, document, _) = seeded(b"one");
    for (index, body) in [(1usize, &b"two"[..]), (2, b"three")] {
        let other = pointer_for(&node, &document, body);
        vfs.write(&entry_path(&root, index, &node, &document), &other.render())
            .unwrap();
    }

    match store.read_pointer(&node, &document) {
        Err(StoreError::PointerUnavailable { disagreeing, .. }) => assert_eq!(disagreeing, 3),
        other => panic!("expected an unavailable pointer, got {other:?}"),
    }
}

// -- the location binding on the read path -------------------------------------

#[test]
fn a_pointer_filed_under_the_wrong_document_is_refused() {
    // The binding `CVS-10` made unskippable, exercised where it matters: a perfectly
    // well-formed entry that belongs to another document is refused rather than served.
    let (vfs, root, store, node, document, _) = seeded(b"body");
    let elsewhere = doc_of("other.json");
    let theirs = pointer_for(&node, &elsewhere, b"body");

    for index in 0..store.pointer_replica_count() {
        vfs.write(
            &entry_path(&root, index, &node, &document),
            &theirs.render(),
        )
        .unwrap();
    }

    match store.read_pointer(&node, &document) {
        Err(StoreError::PointerUnavailable { rejected, .. }) => assert_eq!(rejected, 3),
        other => panic!("expected every replica refused, got {other:?}"),
    }
}

// -- reporting (`D326`) ---------------------------------------------------------

#[test]
fn a_rejection_is_logged_with_both_its_cause_and_what_the_read_did() {
    let (vfs, root, store, node, document, _) = seeded(b"body");
    vfs.write(&entry_path(&root, 2, &node, &document), b"rot\n")
        .unwrap();

    let (events, result) = komorebi::capture::with_capture(|| store.read_pointer(&node, &document));
    assert!(result.is_ok());

    let event = events
        .iter()
        .find(|event| event.field("event") == Some("pointer_replica_rejected"))
        .expect("the refusal was logged");
    assert_eq!(event.field("partition"), Some("2"));
    let cause = event.field("cause").expect("a cause");
    assert!(!cause.is_empty(), "a rejection must say what was wrong");
    assert_eq!(
        event.field("remediation"),
        Some("served the majority held by the remaining replicas")
    );
}

#[test]
fn a_rejection_the_read_could_not_recover_from_says_so() {
    // The distinction a warning emitted at the point of failure cannot make: the same
    // cause, and the two outcomes an operator actually needs to tell apart (`D326`).
    let (vfs, root, store, node, document, _) = seeded(b"body");
    for index in 0..store.pointer_replica_count() {
        vfs.write(&entry_path(&root, index, &node, &document), b"rot\n")
            .unwrap();
    }

    let (events, result) = komorebi::capture::with_capture(|| store.read_pointer(&node, &document));
    assert!(result.is_err());

    let event = events
        .iter()
        .find(|event| event.field("event") == Some("pointer_replica_rejected"))
        .expect("the refusal was logged");
    assert_eq!(
        event.field("remediation"),
        Some("none: no majority could be formed, the read failed")
    );
}

#[test]
fn the_logged_cause_distinguishes_truncation_from_misfiling() {
    // `D326`: the reader can only report the cause the parse gave it, so the causes have
    // to survive all the way to the log line.
    let (vfs, root, store, node, document, pointer) = seeded(b"body");

    let mut truncated = pointer.render();
    truncated.pop();
    vfs.write(&entry_path(&root, 0, &node, &document), &truncated)
        .unwrap();
    let misfiled = pointer_for(&node, &doc_of("other.json"), b"body");
    vfs.write(&entry_path(&root, 1, &node, &document), &misfiled.render())
        .unwrap();

    let (events, _) = komorebi::capture::with_capture(|| store.read_pointer(&node, &document));
    let causes: Vec<&str> = events
        .iter()
        .filter(|event| event.field("event") == Some("pointer_replica_rejected"))
        .filter_map(|event| event.field("cause"))
        .collect();

    assert_eq!(causes.len(), 2);
    assert_ne!(
        causes[0], causes[1],
        "two causes read identically: {causes:?}"
    );
    assert!(causes.iter().any(|cause| cause.contains("truncated")));
    assert!(causes.iter().any(|cause| cause.contains("read from")));
}

// -- traversal (`D222`, `CVS-12`) ----------------------------------------------

/// Writes a pointer for (`node`, `document`), so the namespace has an entry there.
fn place(store: &Store, node: &str, document: &str, token: &str) {
    let node = node_of(node);
    let document = doc_of(document);
    let pointer = pointer_for(&node, &document, document.as_str().as_bytes());
    store
        .write_pointer(&node, &document, &pointer, &nonce_of(token))
        .expect("write");
}

fn names(children: &Children) -> (Vec<String>, Vec<String>) {
    (
        children
            .documents()
            .iter()
            .map(|d| d.as_str().to_string())
            .collect(),
        children.nodes().iter().map(|n| n.to_string()).collect(),
    )
}

#[test]
fn children_are_documents_and_child_nodes_distinguished() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    place(&store, "TICKETS", "a.json", "n1");
    place(&store, "TICKETS", "b.json", "n2");
    place(&store, "TICKETS/DEEP", "c.json", "n3");
    place(&store, "TICKETS/OTHER", "d.json", "n4");

    let (documents, nodes) = names(&store.list_children(node_of("TICKETS")).unwrap());
    assert_eq!(documents, vec!["A.JSON", "B.JSON"]);
    assert_eq!(nodes, vec!["TICKETS/DEEP", "TICKETS/OTHER"]);
}

#[test]
fn children_are_sorted_so_a_walk_is_deterministic() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    for (i, name) in ["z.json", "m.json", "a.json"].iter().enumerate() {
        place(&store, "TICKETS", name, &format!("n{i}"));
    }

    let (documents, _) = names(&store.list_children(node_of("TICKETS")).unwrap());
    assert_eq!(documents, vec!["A.JSON", "M.JSON", "Z.JSON"]);
}

#[test]
fn a_node_that_exists_nowhere_lists_empty_rather_than_erroring() {
    // A node is not a durable object in its own right, so "no children" and "no such
    // node" are the same fact; inventing a distinction would invent state.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    let children = store
        .list_children(node_of("NEVER/WRITTEN"))
        .expect("listing an absent node is not an error");
    assert!(children.is_empty());
}

#[test]
fn traversal_unions_across_replicas_rather_than_taking_a_majority() {
    // The one place the namespace does not vote. A replica that has not caught up holds
    // an entry that is *real*; dropping it because it is not yet everywhere would hide a
    // document from a caller enumerating the store.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    place(&store, "TICKETS", "everywhere.json", "n1");

    // A document present on exactly one replica -- a minority of one.
    let node = node_of("TICKETS");
    let document = doc_of("lonely.json");
    let pointer = pointer_for(&node, &document, b"lonely");
    vfs.write(&entry_path(&root, 1, &node, &document), &pointer.render())
        .unwrap();

    let (documents, _) = names(&store.list_children(node.clone()).unwrap());
    assert_eq!(
        documents,
        vec!["EVERYWHERE.JSON", "LONELY.JSON"],
        "a minority entry must still be enumerable"
    );
}

#[test]
fn traversal_reports_a_document_once_even_when_replicas_disagree_about_its_value() {
    // Presence is what a traversal answers; which pointer is current is a different
    // question, and `read_pointer` is what answers it.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    place(&store, "TICKETS", "a.json", "n1");

    let node = node_of("TICKETS");
    let document = doc_of("a.json");
    let other = pointer_for(&node, &document, b"a different target");
    vfs.write(&entry_path(&root, 0, &node, &document), &other.render())
        .unwrap();

    let (documents, _) = names(&store.list_children(node).unwrap());
    assert_eq!(documents, vec!["A.JSON"]);
}

#[test]
fn reserved_dotfiles_and_foreign_entries_are_skipped() {
    // A foreign tree must not derail a traversal.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    place(&store, "TICKETS", "real.json", "n1");

    let dir = root
        .join(NAMESPACE_TREE_DIR)
        .join("p0")
        .join(node_of("TICKETS").relative_path());
    vfs.write(&dir.join(".hidden"), b"dotfile").unwrap();
    vfs.write(&dir.join("not a name"), b"spaces").unwrap();
    vfs.write(&dir.join("hello.world.txt"), b"foreign").unwrap();
    vfs.create_dir_all(&dir.join("bad name")).unwrap();

    let (documents, nodes) = names(&store.list_children(node_of("TICKETS")).unwrap());
    assert_eq!(documents, vec!["HELLO.WORLD.TXT", "REAL.JSON"]);
    assert!(nodes.is_empty(), "{nodes:?}");
}

#[test]
fn every_document_in_the_namespace_is_enumerable() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    let placed = [
        ("TICKETS", "a.json"),
        ("TICKETS/DEEP", "b.json"),
        ("TICKETS/DEEP/DEEPER", "c.json"),
        ("OTHER", "d.json"),
    ];
    for (i, (node, document)) in placed.iter().enumerate() {
        place(&store, node, document, &format!("n{i}"));
    }

    let listed: Vec<(String, String)> = store
        .list_documents()
        .expect("enumerate")
        .into_iter()
        .map(|(node, document)| (node.to_string(), document.as_str().to_string()))
        .collect();

    assert_eq!(
        listed,
        vec![
            ("OTHER".to_string(), "D.JSON".to_string()),
            ("TICKETS".to_string(), "A.JSON".to_string()),
            ("TICKETS/DEEP".to_string(), "B.JSON".to_string()),
            ("TICKETS/DEEP/DEEPER".to_string(), "C.JSON".to_string()),
        ]
    );
}

#[test]
fn an_empty_namespace_enumerates_to_nothing() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    assert!(store.list_documents().unwrap().is_empty());
}

// -- the one-namespace rule (`D222`) --------------------------------------------

#[test]
fn a_document_may_not_take_a_name_a_child_node_already_holds() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    place(&store, "TICKETS/A", "inner.json", "n1");

    // `A` is a child node of `TICKETS`, so it cannot also be a document there.
    let node = node_of("TICKETS");
    let document = doc_of("a");
    let pointer = pointer_for(&node, &document, b"x");
    match store.write_pointer(&node, &document, &pointer, &nonce_of("n2")) {
        Err(StoreError::NameIsChildNode { path, name }) => {
            assert_eq!(path, node);
            assert_eq!(name, "A");
        }
        other => panic!("expected a namespace collision, got {other:?}"),
    }
}

#[test]
fn a_node_may_not_be_created_beneath_a_document() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    place(&store, "TICKETS", "a", "n1");

    // `TICKETS/A` is a document, so nothing may live beneath it.
    let node = node_of("TICKETS/A");
    let document = doc_of("inner.json");
    let pointer = pointer_for(&node, &document, b"x");
    match store.write_pointer(&node, &document, &pointer, &nonce_of("n2")) {
        Err(StoreError::PathIsDocument { path }) => assert_eq!(path, node_of("TICKETS/A")),
        other => panic!("expected a namespace collision, got {other:?}"),
    }
}

#[test]
fn a_collision_reports_the_shallowest_offending_prefix() {
    // A deeper report would name a directory that only failed because of this one.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    place(&store, "TICKETS", "a", "n1");

    let node = node_of("TICKETS/A/B/C");
    let document = doc_of("inner.json");
    let pointer = pointer_for(&node, &document, b"x");
    match store.write_pointer(&node, &document, &pointer, &nonce_of("n2")) {
        Err(StoreError::PathIsDocument { path }) => assert_eq!(path, node_of("TICKETS/A")),
        other => panic!("expected the shallowest conflict, got {other:?}"),
    }
}

#[test]
fn a_refused_write_leaves_nothing_behind() {
    // The check runs before any staging, so a refusal costs no fsync.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    place(&store, "TICKETS/A", "inner.json", "n1");

    let node = node_of("TICKETS");
    let document = doc_of("a");
    let pointer = pointer_for(&node, &document, b"x");
    assert!(
        store
            .write_pointer(&node, &document, &pointer, &nonce_of("n2"))
            .is_err()
    );

    for index in 0..store.pointer_replica_count() {
        let staging = root
            .join(NAMESPACE_TREE_DIR)
            .join(format!("p{index}"))
            .join(".staging");
        assert!(vfs.read_dir(&staging).unwrap_or_default().is_empty());
    }
    // And the node that was already there is untouched.
    let (documents, _) = names(&store.list_children(node_of("TICKETS/A")).unwrap());
    assert_eq!(documents, vec!["INNER.JSON"]);
}

/// The replica index each read touches first, for `rounds` reads of one document.
///
/// Observed through the filesystem rather than recomputed from the rotation formula: a test
/// that reimplements the arithmetic it is checking only proves the arithmetic equals itself.
fn first_replica_per_read(rounds: usize) -> Vec<usize> {
    use acadia::{Fault, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    use std::sync::Mutex;

    let touched: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let observed = touched.clone();
    let policy = FaultPolicy::new().inject_matching(
        FaultOp::Read,
        PathMatcher::has_component(NAMESPACE_TREE_DIR),
        Fault::observe_with(move |_, path| {
            // The component immediately after `namespace/` is the replica directory.
            let mut parts = path
                .components()
                .filter_map(|part| part.as_os_str().to_str());
            if parts.any(|part| part == NAMESPACE_TREE_DIR)
                && let Some(index) = parts
                    .next()
                    .and_then(|part| part.strip_prefix('p'))
                    .and_then(|digits| digits.parse::<usize>().ok())
            {
                observed.lock().expect("lock").push(index);
            }
        }),
        When::Always,
    );

    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).expect("create root");
    let store = Store::init_unsourced(vfs.clone(), root, test_config()).expect("init");
    let node = node_of("TICKETS/A");
    let document = doc_of("record.json");
    store
        .write_pointer(
            &node,
            &document,
            &pointer_for(&node, &document, b"body"),
            &nonce_of("W1"),
        )
        .expect("write pointer");

    let mut firsts = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        touched.lock().expect("lock").clear();
        store.read_pointer(&node, &document).expect("read pointer");
        let order = touched.lock().expect("lock").clone();
        firsts.push(*order.first().expect("a read touches a replica"));
    }
    firsts
}

#[test]
fn a_pointer_read_does_not_always_begin_at_the_same_replica() {
    // The `D317` property the namespace was missing: a fixed `0..N` walk meets the same
    // replica first on every read, so one device bears every first touch and the order
    // rejections surface in never varies.
    let firsts = first_replica_per_read(crate::store::POINTER_EPOCH_READS as usize * 3 + 1);
    let distinct: std::collections::BTreeSet<usize> = firsts.iter().copied().collect();
    assert!(
        distinct.len() > 1,
        "every read began at replica {:?}; the rotation is not turning",
        distinct
    );
}

#[test]
fn a_pointer_reads_starting_replica_is_stable_within_an_epoch() {
    // Coarse for the same reason the blob epoch is: the starting replica must not move
    // between two reads of one document inside a write window.
    let firsts = first_replica_per_read(crate::store::POINTER_EPOCH_READS as usize);
    let distinct: std::collections::BTreeSet<usize> = firsts.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        1,
        "the starting replica moved inside one epoch: {distinct:?}"
    );
}

#[test]
fn rotating_the_visit_order_does_not_move_the_majority() {
    // Rotation decides which replica is touched first, never which pointer wins: every
    // replica is read and the winner is a strict majority, so the answer is order-free.
    let (_vfs, _root, store, node, document, pointer) = seeded(b"body");
    for _ in 0..8 {
        assert_eq!(
            store
                .read_pointer(&node, &document)
                .expect("read pointer")
                .pointer(),
            pointer,
            "the rotation changed which pointer the vote returned"
        );
    }
}

// A canopy-level test of the overtaken read wants the superseding write to land between the
// reader's two steps. Driving it from inside the reader's own fault observer deadlocks, and
// a second thread cannot be placed there deterministically. The race is covered instead
// through the monitor, by the concurrent snapshot/write regressions in `monitor.rs`.

// -- The modification counter (`D335`) -----------------------------------------

#[test]
fn a_write_raises_the_modification_count_and_a_read_does_not() {
    let (_vfs, _root, store, node, document, _pointer) = seeded(b"body");

    let after_seed = store.modifications();
    assert!(after_seed > 0, "seeding the store modified nothing");

    store.read_pointer(&node, &document).expect("read");
    store.read_pointer(&node, &document).expect("read again");
    assert_eq!(
        store.modifications(),
        after_seed,
        "reading moved the count, so a reader cannot use it to detect writers"
    );

    store
        .write_pointer(
            &node,
            &document,
            &pointer_for(&node, &document, b"second"),
            &nonce_of("W2"),
        )
        .expect("write");
    assert!(
        store.modifications() > after_seed,
        "a durable write did not raise the count"
    );
}

#[test]
fn a_disagreement_with_a_still_count_is_damage_rather_than_a_race() {
    // The distinction the counter exists to make. Three replicas are left holding three
    // different pointers with no writer running; the vote fails, and the count is unmoved
    // across the whole read -- so a caller knows reading again will not settle it, and that
    // the store really has diverged.
    let (vfs, root, store, node, document, _pointer) = seeded(b"body");
    for index in 0..store.pointer_replica_count() {
        let scattered = pointer_for(&node, &document, format!("copy-{index}").as_bytes());
        vfs.write(
            &entry_path(&root, index, &node, &document),
            &scattered.render(),
        )
        .expect("scatter a replica");
    }

    let before = store.modifications();
    let error = store
        .read_pointer(&node, &document)
        .expect_err("three distinct pointers form no majority");
    let after = store.modifications();

    assert!(
        matches!(
            error,
            StoreError::PointerUnavailable {
                missing: 0,
                rejected: 0,
                disagreeing: 3,
                ..
            }
        ),
        "expected a clean three-way disagreement: {error:?}"
    );
    assert_eq!(
        before, after,
        "nothing was written, so the count must be still and the caller must read this as damage"
    );
}

#[test]
fn a_writer_paused_between_replicas_is_never_mistaken_for_no_writer() {
    // Sampled while the write has moved some replicas but not others: the count must read
    // as "in progress", or a reader straddling the pause would call a busy store damaged.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let samples: Arc<std::sync::Mutex<Vec<u64>>> = Arc::default();
    let handle: Arc<std::sync::OnceLock<Store>> = Arc::default();
    let gate = FaultGate::closed();
    let (recorded, store_for_probe) = (Arc::clone(&samples), Arc::clone(&handle));
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(NAMESPACE_TREE_DIR),
            Fault::observe_with(move |_, _| {
                if let Some(store) = store_for_probe.get() {
                    recorded.lock().unwrap().push(store.modifications());
                }
            }),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("paused");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root, test_config()).unwrap();
    let node = name("tickets");
    let document = doc("record");
    let _ = handle.set(store.clone());

    let before = store.modifications();
    gate.open();
    store
        .write_pointer(
            &node,
            &document,
            &pointer_for(&node, &document, b"body"),
            &nonce("w1"),
        )
        .unwrap();
    gate.close();
    let after = store.modifications();

    let samples = samples.lock().unwrap().clone();
    assert_eq!(
        samples.len(),
        store.pointer_replica_count(),
        "one sample per replica"
    );
    for mid in samples {
        assert!(
            !Store::undisturbed(before, mid) && !Store::undisturbed(mid, mid),
            "a sample taken mid-write read as undisturbed: before {before}, mid {mid}"
        );
    }
    assert!(Store::undisturbed(after, after));
    assert!(!Store::undisturbed(before, after));
}

#[test]
fn a_write_between_two_reads_is_visible_as_a_moved_count() {
    // The other half: the caller took a count, the read failed, and the count moved -- so it
    // was raced and reading again may settle it. Driving the write between two reads rather
    // than inside one is what makes this deterministic; what the caller compares is the same
    // pair of values either way.
    let (_vfs, _root, store, node, document, _pointer) = seeded(b"body");

    let before = store.modifications();
    store.read_pointer(&node, &document).expect("first read");
    store
        .write_pointer(
            &node,
            &document,
            &pointer_for(&node, &document, b"second"),
            &nonce_of("W2"),
        )
        .expect("write between the reads");
    store.read_pointer(&node, &document).expect("second read");

    assert!(
        store.modifications() > before,
        "a writer ran between the two reads and the count did not say so"
    );
}

// -- Absence is a candidate (`D338`) -------------------------------------------

#[test]
fn a_create_that_has_reached_a_minority_reads_as_absent_until_the_majority_lands() {
    // The create window, built by hand so the state is exact: one replica holds the new
    // entry, two hold nothing. The two holding nothing are holding the document's previous
    // Written through the real path, because the second half reads a *body*: a pointer
    // planted on its own names a blob that was never ingested, which is a different failure
    // and would prove nothing about the vote.
    let (vfs, store) = fresh_store();
    let node = node_of("TICKETS/A");
    let document = doc_of("record.json");
    store
        .write_document_replicated_as(&node, &document, &body(b"created"), &nonce_of("n1"))
        .expect("seed");
    let root = store.root().to_path_buf();

    let stripped: Vec<Vec<u8>> = [1usize, 2]
        .iter()
        .map(|index| {
            let path = entry_path(&root, *index, &node, &document);
            let entry = vfs.read(&path).expect("an entry to strip");
            vfs.remove_file(&path)
                .expect("strip a replica back to the pre-create state");
            entry
        })
        .collect();

    assert_eq!(
        store.read_document(&node, &document).expect("read"),
        Read::Absent
    );

    // A second replica landing carries the majority the other way, and the answer moves once
    // and does not move back -- which is the whole of what makes this safe rather than merely
    // convenient.
    vfs.write(&entry_path(&root, 1, &node, &document), &stripped[0])
        .expect("land a second replica");
    assert_eq!(
        store.read_document(&node, &document).expect("read"),
        Read::Found(body(b"created"))
    );
    // state, so they carry the vote and the read is decided rather than undetermined.
}

#[test]
fn a_majority_losing_its_entry_reads_as_absent_rather_than_as_damage() {
    // **The deliberate consequence of counting absence, recorded rather than discovered.**
    // Two replicas losing their entry is indistinguishable on disk from a removal that has
    // reached a majority, so the read answers the same way for both: absent. It has no
    // grounds to say otherwise, and `D319` does not entitle it to reach a damage verdict --
    // that is the scrub's, which can see the blobs still sitting there with nothing naming
    // them.
    let (vfs, root, store, node, document, _) = seeded(b"body");
    for index in [0, 2] {
        vfs.remove_file(&entry_path(&root, index, &node, &document))
            .expect("lose a replica's entry");
    }

    assert_eq!(
        store.read_document(&node, &document).expect("read"),
        Read::Absent
    );
}

#[test]
fn replicas_that_all_disagree_are_contended_rather_than_an_error() {
    // Three adjacent versions caught in one sample set: every replica parses, none commands a
    // majority, and not enough are absent to carry it either. Nothing here is decidable, so
    // the read says so in the one shape a caller's loop can wait out (`D336`).
    let (vfs, root, store, node, document, _) = seeded(b"v1");
    for (index, body) in [(1usize, b"v2".as_slice()), (2, b"v3".as_slice())] {
        let pointer = pointer_for(&node, &document, body);
        vfs.write(
            &entry_path(&root, index, &node, &document),
            &pointer.render(),
        )
        .expect("write a divergent replica");
    }

    match store.read_document(&node, &document).expect("read") {
        Read::Contended(contended) => assert_eq!(
            contended.cause(),
            Contention::Undetermined {
                present: 3,
                missing: 0
            }
        ),
        other => panic!("three disagreeing replicas are contention, not {other:?}"),
    }
}

#[test]
fn an_unparseable_replica_keeps_the_read_an_error_even_when_absence_has_a_majority() {
    // The guard that stops absence-as-a-candidate from swallowing damage. Two replicas hold
    // nothing and would carry the vote on their own; the third holds bytes that will not
    // parse, which is damage rather than a state. Answering "not there" over an unreadable
    // namespace would turn a recoverable fault into silent data loss, so the vote is poisoned
    // and the caller is told.
    let (vfs, root, store, node, document, _) = seeded(b"body");
    for index in [1, 2] {
        vfs.remove_file(&entry_path(&root, index, &node, &document))
            .expect("lose a replica's entry");
    }
    vfs.write(&entry_path(&root, 0, &node, &document), b"not a pointer")
        .expect("corrupt the surviving replica");

    assert!(matches!(
        store.read_document(&node, &document),
        Err(StoreError::PointerUnavailable { rejected: 1, .. })
    ));
}

#[test]
fn a_pointer_repair_from_an_obsolete_report_writes_nothing() {
    // The report was taken before an overwrite. Replaying it would put the superseded
    // pointer back over a replica that is now current.
    let (vfs, store) = fresh_store();
    let node = name("tickets");
    let d = doc("record");
    store
        .write_document_replicated_as(&node, &d, &body(b"v1"), &nonce("w1"))
        .unwrap();
    let entry = namespace_root(&store, 0).join("TICKETS").join("RECORD");
    vfs.remove_file(&entry).unwrap();
    let obsolete = store.read_pointer(&node, &d).unwrap();
    assert_eq!(obsolete.missing(), [0]);

    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .unwrap();
    let outcome = store
        .repair_pointer_as(&node, &d, &obsolete, &nonce("r1"))
        .unwrap();

    assert!(outcome.is_noop(), "an obsolete report repaired {outcome:?}");
    assert!(store.read_pointer(&node, &d).unwrap().is_unanimous());
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));
}

#[test]
fn a_pointer_repair_restores_what_is_still_missing() {
    let (vfs, store) = fresh_store();
    let node = name("tickets");
    let d = doc("record");
    store
        .write_document_replicated_as(&node, &d, &body(b"v1"), &nonce("w1"))
        .unwrap();
    let entry = namespace_root(&store, 1).join("TICKETS").join("RECORD");
    vfs.remove_file(&entry).unwrap();
    let report = store.read_pointer(&node, &d).unwrap();

    let outcome = store
        .repair_pointer_as(&node, &d, &report, &nonce("r1"))
        .unwrap();

    assert_eq!(outcome.replaced(), [1]);
    assert!(store.read_pointer(&node, &d).unwrap().is_unanimous());
}

#[test]
fn a_removal_replayed_over_an_absent_entry_still_syncs_its_parent() {
    // The removal landed but its parent sync failed. A reopen over the same live state finds
    // the entry gone; if it skipped the sync, the blob collection after `end` would leave a
    // crash able to resurrect a pointer whose blob no longer exists.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let root = PathBuf::from("removal-sync");
    let parent = root
        .join(crate::store::NAMESPACE_TREE_DIR)
        .join("p0")
        .join("TICKETS");
    let faulty = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Fsync,
            PathMatcher::exact(&parent),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    let config = GenesisConfig::new(ShireId::new("removal-sync").unwrap(), 0)
        .with_pointer_replica_count(1)
        .unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root.clone(), config).unwrap();
    let node = name("tickets");
    let d = doc("record");
    store
        .update_document_as(Swap::new(&node, d.clone(), body(b"v1")), &nonce("w1"))
        .unwrap();

    gate.open();
    store
        .update_document_as(Swap::remove(&node, d.clone()), &nonce("w2"))
        .expect_err("the removal's parent sync fails after `prepare`");
    gate.close();
    assert!(store.recovery_required());
    drop(store);

    let reopened =
        Store::open_unsourced(Arc::clone(&vfs), root.clone()).expect("recovery completes");
    drop(reopened);
    faulty.inner().simulate_crash();

    let after = Store::open_unsourced(vfs, root).expect("reopen after the crash");
    assert_eq!(after.read_body(&node, &d).unwrap(), None);
}

#[test]
fn a_pointer_repair_is_visible_to_the_modification_count() {
    let (vfs, store) = fresh_store();
    let node = name("tickets");
    let d = doc("record");
    store
        .write_document_replicated_as(&node, &d, &body(b"v1"), &nonce("w1"))
        .unwrap();
    vfs.remove_file(&namespace_root(&store, 2).join("TICKETS").join("RECORD"))
        .unwrap();
    let report = store.read_pointer(&node, &d).unwrap();

    let before = store.modifications();
    let outcome = store
        .repair_pointer_as(&node, &d, &report, &nonce("r1"))
        .unwrap();
    let after = store.modifications();

    assert_eq!(outcome.replaced(), [2]);
    assert!(
        !Store::undisturbed(before, after),
        "a repair wrote and the count stood still"
    );
    assert!(Store::undisturbed(after, after));
}

#[test]
fn an_obsolete_pointer_repair_leaves_the_count_alone() {
    // Nothing is written when the report is obsolete, so nothing is announced either.
    let (vfs, store) = fresh_store();
    let node = name("tickets");
    let d = doc("record");
    store
        .write_document_replicated_as(&node, &d, &body(b"v1"), &nonce("w1"))
        .unwrap();
    vfs.remove_file(&namespace_root(&store, 0).join("TICKETS").join("RECORD"))
        .unwrap();
    let obsolete = store.read_pointer(&node, &d).unwrap();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .unwrap();

    let before = store.modifications();
    store
        .repair_pointer_as(&node, &d, &obsolete, &nonce("r1"))
        .unwrap();
    assert!(Store::undisturbed(before, store.modifications()));
}

#[test]
fn collection_never_deletes_the_blob_a_durable_pointer_still_names() {
    // A non-transactional overwrite renames its pointer but cannot sync the parent, so the
    // live pointer names the new body while the durable one still names the old. Recovery
    // re-collects the earlier transaction from the live keep-set; unless that keep-set is made
    // durable first, a crash restores a pointer to the blob just deleted.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let root = PathBuf::from("collect-durable");
    let parent = root
        .join(crate::store::NAMESPACE_TREE_DIR)
        .join("p0")
        .join("TICKETS");
    let faulty = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Fsync,
            PathMatcher::exact(&parent),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    let config = GenesisConfig::new(ShireId::new("collect-durable").unwrap(), 0)
        .with_pointer_replica_count(1)
        .unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root.clone(), config).unwrap();
    let node = name("tickets");
    let d = doc("record");
    store
        .update_document_as(Swap::new(&node, d.clone(), body(b"v1")), &nonce("w1"))
        .unwrap();
    assert!(
        !store.commit_log_is_empty().unwrap(),
        "the ended transaction stays journalled"
    );

    gate.open();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("the pointer's parent sync fails after the rename");
    gate.close();
    drop(store);

    drop(Store::open_unsourced(Arc::clone(&vfs), root.clone()).expect("recovery re-collects w1"));
    faulty.inner().simulate_crash();

    let after = Store::open_unsourced(vfs, root).expect("reopen after the crash");
    let read = after
        .read_body(&node, &d)
        .expect("the durable pointer's blob survived");
    assert!(
        read == Some(body(b"v1")) || read == Some(body(b"v2")),
        "got {read:?}"
    );
}
