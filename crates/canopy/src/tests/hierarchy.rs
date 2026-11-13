// Copyright (c) Michael Grier.

//! The hierarchical namespace: nesting, the one-namespace rule, and enumeration
//! over a tree (`BG-2`, `D222`).
//!
//! `NodePath`'s own validation is [`node_path`](super::node_path)'s; this covers what
//! the *store* does with a path -- where bytes land, which collisions are refused, and
//! what a traversal reports.

use super::*;

// -- Nesting -------------------------------------------------------------------

#[test]
fn documents_at_different_depths_are_independent() {
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(
            name("tickets"),
            &doc("marker"),
            &body(b"parent"),
            &nonce("h1"),
        )
        .expect("write parent");
    store
        .write_document_replicated_as(
            name("tickets/ab"),
            &doc("marker"),
            &body(b"child"),
            &nonce("h2"),
        )
        .expect("write child");

    // A parent node holding a document does not stop a child node existing beneath it,
    // because the two occupy different names: a directory `AB` beside a file `MARKER`.
    assert_eq!(
        store.read_body(name("tickets"), &doc("marker")).unwrap(),
        Some(body(b"parent"))
    );
    assert_eq!(
        store.read_body(name("tickets/ab"), &doc("marker")).unwrap(),
        Some(body(b"child"))
    );
}

#[test]
fn sibling_paths_do_not_collide_even_when_one_name_prefixes_another() {
    // The trap a string-prefix layout would fall into: `TICKETS` and `TICKETSX` are
    // unrelated nodes, and a document at one must not be visible at the other.
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(name("tickets"), &doc("record"), &body(b"a"), &nonce("h3"))
        .expect("write");
    store
        .write_document_replicated_as(name("ticketsx"), &doc("record"), &body(b"b"), &nonce("h4"))
        .expect("write");

    assert_eq!(
        store.read_body(name("tickets"), &doc("record")).unwrap(),
        Some(body(b"a"))
    );
    assert_eq!(
        store.read_body(name("ticketsx"), &doc("record")).unwrap(),
        Some(body(b"b"))
    );
}

#[test]
fn a_deep_path_round_trips_through_the_replicated_write() {
    let (_vfs, store) = fresh_store();
    let deep = name("a/b/c/d/e/f/g/h");
    store
        .write_document_replicated_as(&deep, &doc("record"), &body(b"deep"), &nonce("txn1"))
        .expect("replicated write");

    assert_eq!(
        store.read_body(&deep, &doc("record")).unwrap(),
        Some(body(b"deep"))
    );
}

// -- The one-namespace rule ----------------------------------------------------

#[test]
fn a_document_cannot_take_a_name_a_child_node_already_holds() {
    // `D222`: a name at a path is either a document or a child node, never both.
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(
            name("tickets/ab"),
            &doc("record"),
            &body(b"child"),
            &nonce("h5"),
        )
        .expect("write child");

    // `AB` is now a directory under `TICKETS`, so a document of that name is refused.
    let err = store
        .write_document_replicated_as(name("tickets"), &doc("ab"), &body(b"x"), &nonce("h6"))
        .expect_err("a child node's name is taken");
    match err {
        StoreError::NameIsChildNode { path, name } => {
            assert_eq!(path.to_string(), "TICKETS");
            assert_eq!(name, "AB");
        }
        other => panic!("expected a name-is-child-node refusal, got {other:?}"),
    }
}

#[test]
fn a_path_cannot_descend_through_a_document() {
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(
            name("tickets"),
            &doc("ab"),
            &body(b"a document"),
            &nonce("h7"),
        )
        .expect("write document");

    let err = store
        .write_document_replicated_as(
            name("tickets/ab"),
            &doc("record"),
            &body(b"x"),
            &nonce("h8"),
        )
        .expect_err("cannot descend through a document");
    match err {
        StoreError::PathIsDocument { path } => assert_eq!(path.to_string(), "TICKETS/AB"),
        other => panic!("expected a path-is-document refusal, got {other:?}"),
    }
}

#[test]
fn a_collision_reports_the_shallowest_offending_prefix() {
    // Walking outermost-first matters: a deeper report would name a directory that
    // only failed because of this one, sending a caller to the wrong place.
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(name("a"), &doc("b"), &body(b"a document"), &nonce("h9"))
        .expect("write document");

    let err = store
        .write_document_replicated_as(name("a/b/c/d"), &doc("record"), &body(b"x"), &nonce("h10"))
        .expect_err("cannot descend through a document");
    match err {
        StoreError::PathIsDocument { path } => assert_eq!(path.to_string(), "A/B"),
        other => panic!("expected a path-is-document refusal, got {other:?}"),
    }
}

#[test]
fn a_refused_write_leaves_nothing_behind() {
    // The check runs before staging, so a refusal costs no fsync and creates no
    // directory -- otherwise a rejected write would still reshape the namespace.
    let (vfs, store) = fresh_store();
    store
        .write_document_replicated_as(
            name("tickets"),
            &doc("ab"),
            &body(b"a document"),
            &nonce("h11"),
        )
        .expect("write document");

    assert!(
        store
            .write_document_replicated_as(
                name("tickets/ab"),
                &doc("record"),
                &body(b"x"),
                &nonce("t")
            )
            .is_err()
    );

    // `TICKETS/AB` is still the document it was, not a directory.
    let target = namespace_root(&store, 0).join("TICKETS").join("AB");
    assert!(vfs.metadata(&target).unwrap().is_file());
    assert_eq!(
        store.read_body(name("tickets"), &doc("ab")).unwrap(),
        Some(body(b"a document"))
    );
}

#[test]
fn the_rule_holds_for_the_replicated_write() {
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(
            name("tickets/ab"),
            &doc("record"),
            &body(b"c"),
            &nonce("t1"),
        )
        .expect("write child");

    assert!(matches!(
        store.write_document_replicated_as(name("tickets"), &doc("ab"), &body(b"x"), &nonce("t2")),
        Err(StoreError::NameIsChildNode { .. })
    ));
}

#[test]
fn a_collision_check_that_cannot_look_fails_rather_than_passing() {
    // An I/O error is not absence: a collision it hid would reach `prepare` and leave
    // recovery a roll-forward that can never succeed.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Metadata,
            PathMatcher::file_name("AB"),
            Fault::io_error(),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("collision-probe");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(vfs, root, test_config()).unwrap();
    store
        .write_document_replicated_as(
            name("tickets/ab"),
            &doc("record"),
            &body(b"c"),
            &nonce("t1"),
        )
        .expect("write child");

    gate.open();
    let refused = store.update_document_as(
        Swap::new(name("tickets"), doc("ab"), body(b"x")),
        &nonce("t2"),
    );
    gate.close();

    assert!(
        matches!(refused, Err(StoreError::Vfs(_))),
        "got {refused:?}"
    );
    assert!(
        store.commit_log_is_empty().unwrap(),
        "nothing was journalled"
    );
    assert!(!store.recovery_required());
}

#[test]
fn a_node_directory_whose_name_could_not_be_synced_is_synced_by_the_next_write() {
    // The retry finds the directory present, so without remembering the failed sync it
    // would never make the name durable, and a crash would drop the node from the replica.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let root = PathBuf::from("unsynced-node");
    let partition = root.join(crate::store::NAMESPACE_TREE_DIR).join("p0");
    let faulty = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new()
            .inject_gated(
                FaultOp::Fsync,
                PathMatcher::exact(&partition),
                Fault::io_error(),
                When::Once,
                gate.clone(),
            )
            .inject_gated(
                FaultOp::RemoveDir,
                PathMatcher::file_name("TICKETS"),
                Fault::io_error(),
                When::Always,
                gate.clone(),
            ),
    ));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(vfs, root, test_config()).unwrap();

    gate.open();
    store
        .write_document_replicated_as(name("tickets"), &doc("first"), &body(b"a"), &nonce("t1"))
        .expect_err("the node's parent sync fails and the directory cannot be removed");
    gate.close();
    store
        .write_document_replicated_as(name("tickets"), &doc("second"), &body(b"b"), &nonce("t2"))
        .expect("the retry succeeds");

    faulty.inner().simulate_crash();
    assert!(
        faulty.inner().exists(&partition.join("TICKETS")).unwrap(),
        "the node directory's name was never made durable"
    );
}

#[test]
fn removing_a_name_that_is_a_child_node_is_refused_before_begin() {
    // The removal could never apply, so letting it reach `prepare` would fence the store and
    // make every reopen retry it forever.
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(
            name("tickets/ab"),
            &doc("record"),
            &body(b"c"),
            &nonce("t1"),
        )
        .expect("write child");

    let refused = store.update_document_as(Swap::remove(name("tickets"), doc("ab")), &nonce("t2"));

    assert!(
        matches!(refused, Err(StoreError::NameIsChildNode { .. })),
        "got {refused:?}"
    );
    assert!(store.commit_log_is_empty().unwrap());
    assert!(!store.recovery_required());
}

#[test]
fn a_node_directory_whose_name_could_not_be_synced_is_synced_after_a_reopen() {
    // The debt is persisted, so a reopen -- which finds the directory present and would never
    // sync its name on its own -- still discharges it.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let root = PathBuf::from("unsynced-reopen");
    let partition = root.join(crate::store::NAMESPACE_TREE_DIR).join("p0");
    let faulty = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new()
            .inject_gated(
                FaultOp::Fsync,
                PathMatcher::exact(&partition),
                Fault::io_error(),
                When::Once,
                gate.clone(),
            )
            .inject_gated(
                FaultOp::RemoveDir,
                PathMatcher::file_name("TICKETS"),
                Fault::io_error(),
                When::Always,
                gate.clone(),
            ),
    ));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();

    gate.open();
    store
        .write_document_replicated_as(name("tickets"), &doc("first"), &body(b"a"), &nonce("t1"))
        .expect_err("the node's parent sync fails and the directory cannot be removed");
    gate.close();
    drop(store);

    let reopened = Store::open_unsourced(vfs, root).expect("reopen discharges the debt");
    reopened
        .write_document_replicated_as(name("tickets"), &doc("second"), &body(b"b"), &nonce("t2"))
        .expect("write after reopen");
    drop(reopened);

    faulty.inner().simulate_crash();
    assert!(
        faulty.inner().exists(&partition.join("TICKETS")).unwrap(),
        "the node directory's name was never made durable"
    );
}

#[test]
fn a_set_removing_a_name_another_of_its_writes_nests_under_is_refused_before_begin() {
    // The write would turn the removed name into a directory before the removal ran, so the
    // removal could never apply once `prepare` had made it a commitment.
    let (_vfs, store) = fresh_store();
    let swaps = SwapSet::new([
        Swap::new(name("a/b/c"), doc("x"), body(b"nested")),
        Swap::remove(name("a"), doc("b")),
    ])
    .unwrap();

    let refused = store.apply_transaction_as(&swaps, &nonce("t1"));

    assert!(
        matches!(refused, Err(StoreError::NameIsChildNode { ref name, .. }) if name == "B"),
        "got {refused:?}"
    );
    assert!(store.commit_log_is_empty().unwrap());
    assert!(!store.recovery_required());
}

#[test]
fn a_set_removing_beneath_a_document_it_writes_is_refused_before_begin() {
    let (_vfs, store) = fresh_store();
    let swaps = SwapSet::new([
        Swap::new(name("a"), doc("b"), body(b"a document")),
        Swap::remove(name("a/b/c"), doc("x")),
    ])
    .unwrap();

    assert!(matches!(
        store.apply_transaction_as(&swaps, &nonce("t1")),
        Err(StoreError::NameIsChildNode { .. })
    ));
    assert!(store.commit_log_is_empty().unwrap());
}

#[test]
fn a_directory_whose_creation_cannot_be_recorded_is_not_created() {
    // The record of a directory about to be created is what lets a reopen sync its name, so
    // a creation that cannot be recorded must not happen at all.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let root = PathBuf::from("unrecorded");
    let faulty = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Append,
            PathMatcher::file_name(crate::store::DIR_SYNC_DEBT_NAME),
            Fault::io_error(),
            When::Always,
            gate.clone(),
        ),
    ));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root, test_config()).unwrap();

    gate.open();
    store
        .write_document_replicated_as(name("tickets"), &doc("first"), &body(b"a"), &nonce("t1"))
        .expect_err("the record cannot be written");
    gate.close();

    for index in 0..store.pointer_replica_count() {
        assert!(
            !vfs.exists(&namespace_root(&store, index).join("TICKETS"))
                .unwrap(),
            "replica {index} created a directory it could not record"
        );
    }
    store
        .write_document_replicated_as(name("tickets"), &doc("first"), &body(b"a"), &nonce("t2"))
        .expect("the retry succeeds once the record can be written");
}

#[test]
fn a_stale_intent_below_what_is_now_a_document_does_not_block_reopening() {
    // A recorded intent outlives the directory it named when power is lost before its
    // unflushed truncation lands. If a later write has since made an ancestor a document, the
    // directory cannot exist any more, and the intent is obsolete rather than an error.
    let (vfs, store) = fresh_store();
    let root = store.root().to_path_buf();
    store
        .update_document_as(
            Swap::new(name("a/b/c"), doc("leaf"), body(b"x")),
            &nonce("t1"),
        )
        .unwrap();
    let swaps = SwapSet::with_node_removals(
        [Swap::remove(name("a/b/c"), doc("leaf"))],
        [name("a/b/c"), name("a/b")],
    )
    .unwrap();
    assert_eq!(
        store.apply_transaction_as(&swaps, &nonce("t2")).unwrap(),
        TransactionOutcome::Committed
    );
    store
        .update_document_as(Swap::new(name("a"), doc("b"), body(b"doc")), &nonce("t3"))
        .unwrap();
    drop(store);

    let mut stale = String::new();
    for index in 0..3 {
        stale.push_str(&format!(
            "{}/p{index}/A/B/C\n{}/p{index}/A/B/C/DEEPER\n",
            crate::store::NAMESPACE_TREE_DIR,
            crate::store::NAMESPACE_TREE_DIR
        ));
    }
    vfs.write(
        &root.join(crate::store::DIR_SYNC_DEBT_NAME),
        stale.as_bytes(),
    )
    .unwrap();

    let reopened = Store::open_unsourced(Arc::clone(&vfs), root.clone())
        .expect("an intent under a document is obsolete, not fatal");
    assert_ne!(
        reopened.read_document(name("a"), &doc("b")).unwrap(),
        crate::Read::Absent
    );
    assert!(
        vfs.read(&root.join(crate::store::DIR_SYNC_DEBT_NAME))
            .unwrap()
            .is_empty(),
        "the obsolete record was discharged"
    );
}

#[test]
fn a_path_beneath_a_document_reads_as_absent() {
    // POSIX reports a path under a file as "not a directory" rather than "not found". Either
    // way nothing is there, and a reader -- or recovery collecting an old transaction's
    // superseded body -- must see absence, not an I/O failure.
    let (_vfs, store) = fresh_store();
    store
        .update_document_as(Swap::new(name("a"), doc("b"), body(b"doc")), &nonce("t1"))
        .unwrap();

    assert_eq!(
        store.read_document(name("a/b/c"), &doc("leaf")).unwrap(),
        crate::Read::Absent
    );
    let children = store.list_children(name("a/b/c")).unwrap();
    assert!(children.documents().is_empty() && children.nodes().is_empty());
}

#[test]
fn an_unterminated_intent_tail_is_not_acted_on_at_reopen() {
    // A tail with no newline is an append that never completed, so nothing it names was
    // created on its authority; a reopen discharges the whole records only.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let root = PathBuf::from("tail-intent");
    let namespace = root.join(crate::store::NAMESPACE_TREE_DIR);
    let faulty = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Fsync,
            PathMatcher::exact(&namespace),
            Fault::io_error(),
            When::Always,
            gate.clone(),
        ),
    ));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    drop(Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap());
    let whole = std::path::Path::new(crate::store::NAMESPACE_TREE_DIR)
        .join("p1")
        .join("A");
    let torn = std::path::Path::new(crate::store::NAMESPACE_TREE_DIR).join("p0");
    vfs.write(
        &root.join(crate::store::DIR_SYNC_DEBT_NAME),
        format!("{}\n{}", whole.display(), torn.display()).as_bytes(),
    )
    .unwrap();

    gate.open();
    let reopened = Store::open_unsourced(Arc::clone(&vfs), root.clone());
    gate.close();
    reopened.expect("the unterminated tail is not followed into a failing sync");
    assert!(
        vfs.read(&root.join(crate::store::DIR_SYNC_DEBT_NAME))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_torn_intent_record_does_not_hide_the_directory_a_retry_creates() {
    // A short append leaves an unterminated prefix. Appending the retry's record onto it would
    // durably name a different path, so a reopen would discharge the wrong one and never sync
    // the directory the retry actually created.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let root = PathBuf::from("torn-intent");
    let partition = root.join(crate::store::NAMESPACE_TREE_DIR).join("p0");
    let faulty = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new()
            .inject_gated(
                FaultOp::Append,
                PathMatcher::file_name(crate::store::DIR_SYNC_DEBT_NAME),
                Fault::short_write(5),
                When::Once,
                gate.clone(),
            )
            .inject_gated(
                FaultOp::Fsync,
                PathMatcher::exact(&partition),
                Fault::io_error(),
                When::Once,
                gate.clone(),
            )
            .inject_gated(
                FaultOp::RemoveDir,
                PathMatcher::file_name("TICKETS"),
                Fault::io_error(),
                When::Always,
                gate.clone(),
            ),
    ));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    // Leave the blob shard behind but not the node, so the only directory the next write
    // creates -- and the only intent it records -- is the node's.
    store
        .update_document_as(
            Swap::new(name("tickets"), doc("first"), body(b"a")),
            &nonce("s1"),
        )
        .unwrap();
    let emptied = SwapSet::with_node_removals(
        [Swap::remove(name("tickets"), doc("first"))],
        [name("tickets")],
    )
    .unwrap();
    store.apply_transaction_as(&emptied, &nonce("s2")).unwrap();
    assert!(!vfs.exists(&partition.join("TICKETS")).unwrap());

    gate.open();
    store
        .write_document_replicated_as(name("tickets"), &doc("first"), &body(b"a"), &nonce("t1"))
        .expect_err("the record is torn");
    assert!(
        !vfs.read(&root.join(crate::store::DIR_SYNC_DEBT_NAME))
            .unwrap()
            .ends_with(b"\n"),
        "the short append left an unterminated tail"
    );
    store
        .write_document_replicated_as(name("tickets"), &doc("first"), &body(b"a"), &nonce("t2"))
        .expect_err("the node's parent sync fails and the directory cannot be removed");
    gate.close();
    let record = vfs
        .read(&root.join(crate::store::DIR_SYNC_DEBT_NAME))
        .unwrap();
    let record = String::from_utf8(record).unwrap();
    assert!(
        record.ends_with('\n')
            && record
                .lines()
                .all(|line| line.ends_with("TICKETS") && line.starts_with("namespace")),
        "every record is a whole path: {record:?}"
    );
    drop(store);

    let reopened =
        Store::open_unsourced(Arc::clone(&vfs), root).expect("reopen discharges the debt");
    reopened
        .write_document_replicated_as(name("tickets"), &doc("second"), &body(b"b"), &nonce("t3"))
        .expect("write after reopen");
    drop(reopened);

    faulty.inner().simulate_crash();
    assert!(
        faulty.inner().exists(&partition.join("TICKETS")).unwrap(),
        "the node directory's name was never made durable"
    );
}

#[test]
fn a_refused_replicated_write_ingests_no_blob() {
    // The collision check runs before the blob is ingested; otherwise every refusal would
    // leave behind a committed blob that no pointer will ever name.
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
    let (vfs, store) = fresh_store();
    store
        .write_document_replicated_as(
            name("tickets/ab"),
            &doc("record"),
            &body(b"c"),
            &nonce("t1"),
        )
        .expect("write child");
    let blobs = store.root().join(crate::store::BLOB_TREE_DIR);
    let before = committed_blobs(&vfs, &blobs);

    assert!(
        store
            .write_document_replicated_as(name("tickets"), &doc("ab"), &body(b"new"), &nonce("t2"))
            .is_err()
    );
    assert_eq!(committed_blobs(&vfs, &blobs), before);
}

// -- Enumeration over a tree ---------------------------------------------------

#[test]
fn enumeration_reports_documents_at_every_depth_with_their_full_paths() {
    let (_vfs, store) = fresh_store();
    for (path, document) in [
        ("shire", "permutation-key"),
        ("tickets/aa/aaa", "record"),
        ("tickets/ab/abc", "record"),
        ("tickets/ab/abc", "engine"),
        ("pending-work/0001", "entry"),
    ] {
        store
            .write_document_replicated_as(name(path), &doc(document), &body(b"x"), &nonce("t"))
            .expect("write");
    }

    let listed: Vec<String> = store
        .list_documents()
        .expect("list")
        .into_iter()
        .map(|(path, document)| format!("{path}/{document}"))
        .collect();

    // Sorted, component-wise, with each document under its own full path.
    assert_eq!(
        listed,
        [
            "PENDING-WORK/0001/ENTRY",
            "SHIRE/PERMUTATION-KEY",
            "TICKETS/AA/AAA/RECORD",
            "TICKETS/AB/ABC/ENGINE",
            "TICKETS/AB/ABC/RECORD",
        ]
    );
}

#[test]
fn enumeration_skips_staging_and_the_marker() {
    // The reserved dotfiles sit inside the partition root, which the walk now descends
    // into rather than treating as a fixed two-tier shape -- so the skip matters more
    // than it did before `D222`.
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(name("tickets/ab"), &doc("record"), &body(b"x"), &nonce("t"))
        .expect("write");

    let listed = store.list_documents().expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0.to_string(), "TICKETS/AB");
}

#[test]
fn enumeration_of_an_empty_store_is_empty() {
    let (_vfs, store) = fresh_store();
    assert!(store.list_documents().expect("list").is_empty());
}

#[test]
fn a_file_at_the_namespace_root_is_not_a_document() {
    // It has no node path to belong to, so it is skipped rather than reported under
    // some invented parent.
    let (vfs, store) = fresh_store();
    let namespace_root = store
        .root()
        .join(crate::store::NAMESPACE_TREE_DIR)
        .join("p0");
    vfs.write(&namespace_root.join("STRAY"), b"junk")
        .expect("write stray");
    assert!(store.list_documents().expect("list").is_empty());
}

// -- Child listing (`BG-3`) ----------------------------------------------------

#[test]
fn listing_a_prefix_reports_its_documents_and_child_nodes_separately() {
    let (_vfs, store) = fresh_store();
    for (path, document) in [
        ("tickets", "marker"),
        ("tickets", "index"),
        ("tickets/aa", "record"),
        ("tickets/ab", "record"),
        ("tickets/ab/deeper", "record"),
        ("shire", "permutation-key"),
    ] {
        store
            .write_document_replicated_as(name(path), &doc(document), &body(b"x"), &nonce("t"))
            .expect("write");
    }

    let children = store.list_children(name("tickets")).expect("list");

    // Documents at the path itself, as leaf names.
    let documents: Vec<&str> = children
        .documents()
        .iter()
        .map(DocumentName::as_str)
        .collect();
    assert_eq!(documents, ["INDEX", "MARKER"]);

    // Child nodes as full paths, one tier down only -- `TICKETS/AB/DEEPER` is a child
    // of `TICKETS/AB`, not of `TICKETS`.
    let nodes: Vec<String> = children.nodes().iter().map(NodePath::to_string).collect();
    assert_eq!(nodes, ["TICKETS/AA", "TICKETS/AB"]);
}

#[test]
fn listing_does_not_reach_into_sibling_prefixes() {
    // The property that makes this worth having: a collection's listing costs its own
    // size, not the store's.
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(
            name("pending-work/0001"),
            &doc("entry"),
            &body(b"x"),
            &nonce("t"),
        )
        .expect("write");
    for i in 0..20 {
        store
            .write_document_replicated_as(
                name(&format!("tickets/t{i:04}")),
                &doc("record"),
                &body(b"x"),
                &nonce("t"),
            )
            .expect("write");
    }

    let children = store.list_children(name("pending-work")).expect("list");
    assert!(children.documents().is_empty());
    assert_eq!(
        children
            .nodes()
            .iter()
            .map(NodePath::to_string)
            .collect::<Vec<_>>(),
        ["PENDING-WORK/0001"]
    );
}

#[test]
fn listing_an_absent_path_is_empty_rather_than_an_error() {
    // A node is not a durable object in its own right, so "no children" and "no such
    // node" are the same fact; inventing a distinction would invent state.
    let (_vfs, store) = fresh_store();
    let children = store.list_children(name("nothing/here")).expect("list");
    assert!(children.is_empty());
    assert!(children.documents().is_empty());
    assert!(children.nodes().is_empty());
}

#[test]
fn listing_the_root_level_reports_the_top_tier() {
    let (_vfs, store) = fresh_store();
    for path in ["tickets/ab", "shire", "pending-work/0001"] {
        store
            .write_document_replicated_as(name(path), &doc("record"), &body(b"x"), &nonce("t"))
            .expect("write");
    }

    // `SHIRE` is a node here, not a document, because a document was written *beneath*
    // it -- the one-namespace rule seen from the listing side.
    let children = store.list_children(name("shire")).expect("list");
    assert_eq!(
        children
            .documents()
            .iter()
            .map(DocumentName::as_str)
            .collect::<Vec<_>>(),
        ["RECORD"]
    );
}

#[test]
fn a_child_listing_agrees_with_the_full_traversal_filtered_to_that_path() {
    // The guard that keeps the two primitives from drifting: whatever
    // `list_documents` reports at a path must be exactly what `list_children` reports
    // as that path's documents.
    let (_vfs, store) = fresh_store();
    for (path, document) in [
        ("tickets", "marker"),
        ("tickets/aa", "record"),
        ("tickets/aa", "engine"),
        ("tickets/ab", "record"),
        ("shire", "key"),
    ] {
        store
            .write_document_replicated_as(name(path), &doc(document), &body(b"x"), &nonce("t"))
            .expect("write");
    }

    for path in ["tickets", "tickets/aa", "tickets/ab", "shire"] {
        let path = name(path);
        let mut from_traversal: Vec<DocumentName> = store
            .list_documents()
            .expect("list all")
            .into_iter()
            .filter(|(candidate, _)| *candidate == path)
            .map(|(_, document)| document)
            .collect();
        from_traversal.sort();

        let from_children = store.list_children(&path).expect("list children");
        assert_eq!(
            from_children.documents(),
            from_traversal.as_slice(),
            "disagreement at {path}"
        );
    }
}

#[test]
fn into_parts_returns_the_same_two_lists() {
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(name("tickets"), &doc("marker"), &body(b"x"), &nonce("t"))
        .expect("write");
    store
        .write_document_replicated_as(name("tickets/ab"), &doc("record"), &body(b"x"), &nonce("t"))
        .expect("write");

    let children = store.list_children(name("tickets")).expect("list");
    let documents = children.documents().to_vec();
    let nodes = children.nodes().to_vec();
    assert_eq!(children.into_parts(), (documents, nodes));
}
