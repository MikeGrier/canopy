// Copyright (c) Michael Grier.

//! `CVS-15` integration test: deletion rides transaction completion and replays on recovery
//! (`D328`).
//!
//! Three claims, and the third is the one that is easy to get wrong.
//!
//! **Superseded content goes, and only superseded content.** A second write to a document
//! leaves exactly one blob -- the one the namespace points at. A removal leaves none.
//!
//! **Deletion is ordered after `end`.** Deleting any earlier would destroy the body a
//! rollback returns to, and would leave the still-current pointer naming a blob that is gone.
//! A crash between `end` and the collection therefore leaves superseded blobs on disk, and
//! recovery is what finishes the job -- which is what makes a collector unnecessary: there is
//! no scan, and no orphan of a *completed* transaction outlives the next open.
//!
//! **Replay is order-independent.** This is the trap. Recovery replays ended transactions in
//! log order, so a log holding two writes to one document replays the first transaction's
//! collection before the second's. An implementation that kept the digest named in each
//! `prepare` record would delete the live blob during that first replay and produce a
//! dangling pointer -- the cleanup creating exactly the corruption `D327` exists to prevent.
//! Taking the keep-set from the namespace instead makes the question "what is live now",
//! which no replay order can get wrong.

use std::path::PathBuf;
use std::sync::Arc;

use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, Vfs, When};
use canopy::{
    BlobName, DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, ShireId, Store,
    StoreError, Swap, SwapSet,
};

fn node() -> NodePath {
    NodePath::parse("tickets/alpha").unwrap()
}

fn document() -> DocumentName {
    DocumentName::new("record.json").unwrap()
}

fn body(text: &str) -> DocumentBody {
    DocumentBody::new(text.as_bytes().to_vec())
}

fn config() -> GenesisConfig {
    GenesisConfig::new(ShireId::new("cvs15-shire").unwrap(), 3)
}

fn fresh() -> (Arc<dyn Vfs>, PathBuf, Store) {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(vfs.clone(), root.clone(), config()).expect("init");
    (vfs, root, store)
}

fn write(store: &Store, text: &str, txn: &str) {
    store
        .apply_transaction(
            &SwapSet::single(Swap::new(node(), document(), body(text))),
            &Nonce::new(txn).unwrap(),
        )
        .expect("an uncontended write commits");
}

fn remove(store: &Store, txn: &str) {
    store
        .apply_transaction(
            &SwapSet::single(Swap::remove(node(), document())),
            &Nonce::new(txn).unwrap(),
        )
        .expect("an uncontended removal commits");
}

/// Every blob this document owns, across every replica, as `(replica, file name)`.
fn blobs(vfs: &Arc<dyn Vfs>, store: &Store) -> Vec<(usize, String)> {
    let shard = canopy::PathHash::of(&node(), &document()).shard(store.shard_width());
    let mut found = Vec::new();
    for index in 0..store.blob_replica_count() {
        let dir = store
            .root()
            .join("files")
            .join(format!("p{index}"))
            .join(&shard);
        let Ok(entries) = vfs.read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let name = entry.file_name.to_string_lossy().into_owned();
            // Other documents may share the shard; only this one's blobs are the subject.
            if BlobName::parse(&name).map(|blob| blob.path_hash())
                == Ok(canopy::PathHash::of(&node(), &document()))
            {
                found.push((index, name));
            }
        }
    }
    found.sort();
    found
}

/// The body the namespace currently resolves to, or `None` when the document is absent.
fn resolved(store: &Store) -> Option<Vec<u8>> {
    match store.read_pointer(&node(), &document()) {
        Ok(read) => Some(
            store
                .read_blob(&read.pointer().target())
                .expect("a pointer must name a readable blob")
                .into_bytes(),
        ),
        Err(StoreError::PointerUnavailable { .. }) => None,
        Err(other) => panic!("unexpected pointer read failure: {other:?}"),
    }
}

#[test]
fn a_second_write_leaves_only_the_blob_the_namespace_points_at() {
    let (vfs, _root, store) = fresh();
    write(&store, "v1", "T1");
    assert_eq!(blobs(&vfs, &store).len(), store.blob_replica_count());

    write(&store, "v2", "T2");

    // One blob per replica, not two: v1 was superseded and collected.
    assert_eq!(
        blobs(&vfs, &store).len(),
        store.blob_replica_count(),
        "the superseded version was not collected: {:?}",
        blobs(&vfs, &store)
    );
    assert_eq!(resolved(&store).as_deref(), Some(b"v2".as_slice()));
}

#[test]
fn removing_a_document_takes_its_pointer_and_then_its_blobs() {
    let (vfs, _root, store) = fresh();
    write(&store, "v1", "T1");
    remove(&store, "T2");

    assert_eq!(
        resolved(&store),
        None,
        "a removed document still resolves through the namespace"
    );
    assert!(
        blobs(&vfs, &store).is_empty(),
        "a removed document left blobs behind: {:?}",
        blobs(&vfs, &store)
    );
    assert!(
        !store
            .list_documents()
            .expect("enumeration")
            .iter()
            .any(|(n, d)| *n == node() && *d == document()),
        "a removed document is still enumerable in the namespace"
    );
}

#[test]
fn removing_a_node_takes_its_directory_from_both_trees() {
    // A node is a directory in the partition tree and in the namespace, so removing it from
    // one and not the other leaves the two disagreeing about which nodes exist -- a child one
    // traversal reports and the other does not.
    let (vfs, _root, store) = fresh();
    write(&store, "v1", "T1");

    store
        .apply_transaction(
            &SwapSet::with_node_removals([Swap::remove(node(), document())], [node()])
                .expect("valid"),
            &Nonce::new("T2").unwrap(),
        )
        .expect("the node is empty once its only document goes");

    for index in 0..store.pointer_replica_count() {
        let dir = store
            .root()
            .join("namespace")
            .join(format!("p{index}"))
            .join(node().relative_path());
        assert!(
            !vfs.exists(&dir).unwrap(),
            "namespace replica {index} still holds the node directory"
        );
    }
}

#[test]
fn a_crash_between_end_and_the_collection_is_finished_by_recovery() {
    // The ordering claim, observed rather than assumed. Faulting the blob removal makes the
    // transaction report a fault *after* its commit point, leaving the superseded blob on
    // disk -- exactly the state a crash in that window produces. Recovery must then collect
    // it, because `end` is appended before the collection and so is no evidence it ran.
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::RemoveFile,
            PathMatcher::has_component("files"),
            Fault::io_error(),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(vfs.clone(), root.clone(), config()).expect("init");

    write(&store, "v1", "T1");
    let before = blobs(&vfs, &store);
    assert_eq!(before.len(), store.blob_replica_count());

    gate.open();
    store
        .apply_transaction(
            &SwapSet::single(Swap::new(node(), document(), body("v2"))),
            &Nonce::new("T2").unwrap(),
        )
        .expect_err("a failed collection is reported even though the transaction committed");

    // The transaction landed: the namespace points at v2, and v2's blob is durable. What did
    // not happen is the discard of v1.
    assert_eq!(resolved(&store).as_deref(), Some(b"v2".as_slice()));
    assert_eq!(
        blobs(&vfs, &store).len(),
        2 * store.blob_replica_count(),
        "the interrupted collection should have left the superseded blob in place"
    );

    gate.close();
    drop(store);
    let reopened = Store::open(vfs.clone(), root).expect("reopen replays the log");

    assert_eq!(
        blobs(&vfs, &reopened).len(),
        reopened.blob_replica_count(),
        "recovery did not finish the collection the crash interrupted: {:?}",
        blobs(&vfs, &reopened)
    );
    assert_eq!(resolved(&reopened).as_deref(), Some(b"v2".as_slice()));
}

#[test]
fn replaying_two_completed_writes_does_not_collect_the_live_blob() {
    // The trap. Both transactions are in the log and both ended, so recovery replays their
    // collections in order -- the first one's before the second's. A keep-set taken from the
    // `prepare` record would have the first replay delete v2, the blob the namespace points
    // at, leaving a dangling pointer produced by the cleanup itself.
    let (vfs, root, store) = fresh();
    write(&store, "v1", "T1");
    write(&store, "v2", "T2");

    // The log is only emptied at quiescence, which nothing has called, so both transactions
    // are still there to be replayed.
    assert!(
        !store.commit_log_is_empty().expect("read the log"),
        "this test needs a non-empty log to replay"
    );

    drop(store);
    let reopened = Store::open(vfs.clone(), root).expect("reopen replays the log");

    assert_eq!(
        resolved(&reopened).as_deref(),
        Some(b"v2".as_slice()),
        "replaying an earlier transaction's collection destroyed the live blob"
    );
    assert_eq!(
        blobs(&vfs, &reopened).len(),
        reopened.blob_replica_count(),
        "replay left the wrong number of blobs: {:?}",
        blobs(&vfs, &reopened)
    );
}

#[test]
fn a_rolled_back_transaction_leaves_the_previous_body_readable() {
    // Why deletion is ordered after `end` at all. A transaction that never reaches its commit
    // point must leave the document exactly as it was -- which it cannot do if the collection
    // has already discarded what the current pointer names.
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Append,
            PathMatcher::has_component(".commit-log"),
            Fault::io_error(),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(vfs.clone(), root.clone(), config()).expect("init");

    write(&store, "v1", "T1");

    // Every log append now fails, so the second transaction cannot reach `prepare`.
    gate.open();
    store
        .apply_transaction(
            &SwapSet::single(Swap::new(node(), document(), body("v2"))),
            &Nonce::new("T2").unwrap(),
        )
        .expect_err("a transaction that cannot journal cannot commit");

    gate.close();
    drop(store);
    let reopened = Store::open(vfs.clone(), root).expect("reopen rolls the transaction back");

    assert_eq!(
        resolved(&reopened).as_deref(),
        Some(b"v1".as_slice()),
        "a rolled-back transaction must leave the previous body readable"
    );
}
