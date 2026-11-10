// Copyright (c) Michael Grier.

//! `CVS-23` integration test: the non-transactional write path reaches both trees (`D332`).
//!
//! `CVS-14` wired blob ingest and pointer writes into `apply_transaction` only. canopy's other
//! write path -- `write_document_replicated`, reached through [`WriteMonitor::write`] and
//! [`WriteMonitor::conditional_write`] -- journals nothing and, until this item, wrote nothing
//! to the content store or the namespace. Callers drive it on their hot paths, so `CVS-19`
//! would have lost every document written that way.
//!
//! The tests go through the **monitor**, not the store, because that is the surface the
//! callers actually use: a test that called `write_document_replicated` directly would prove
//! the primitive was wired and say nothing about whether the path in production reaches it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, Vfs, When};
use canopy::{
    BlobName, DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, PathHash, ShireId, Store,
    StoreError, WriteMonitor,
};

fn node() -> NodePath {
    NodePath::parse("tickets/alpha").unwrap()
}

fn doc() -> DocumentName {
    DocumentName::new("record.json").unwrap()
}

fn body(text: &str) -> DocumentBody {
    DocumentBody::new(text.as_bytes().to_vec())
}

/// A monitor over a fresh store, plus the backend for inspecting what landed.
fn monitored() -> (Arc<dyn Vfs>, WriteMonitor) {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        vfs.clone(),
        root,
        GenesisConfig::new(ShireId::new("cvs23-shire").unwrap(), 3),
    )
    .expect("init");
    let next = AtomicU64::new(0);
    let monitor = WriteMonitor::spawn(store, move || {
        Nonce::new(&format!("N{}", next.fetch_add(1, Ordering::Relaxed))).unwrap()
    })
    .expect("spawn");
    (vfs, monitor)
}

/// What the namespace resolves the document to, following its pointer into the blob tree.
fn via_namespace(store: &Store) -> Option<Vec<u8>> {
    match store.read_pointer(&node(), &doc()) {
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

/// How many blobs the document owns across every replica.
fn blob_count(vfs: &Arc<dyn Vfs>, store: &Store) -> usize {
    let path_hash = PathHash::of(&node(), &doc());
    let shard = path_hash.shard(store.shard_width());
    let mut count = 0;
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
            if BlobName::parse(&name).map(|blob| blob.path_hash()) == Ok(path_hash) {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn a_monitor_write_is_reachable_through_the_namespace() {
    // The gap this item closed: before it, this document existed only in the partition tree
    // and `CVS-19` would have lost it.
    let (_vfs, monitor) = monitored();
    monitor
        .write(node(), doc(), body("v1"))
        .expect("the write commits");

    assert_eq!(
        via_namespace(monitor.store()).as_deref(),
        Some(b"v1".as_slice()),
        "a monitor write did not reach the content store and the namespace"
    );
    // And the namespace enumerates it, so a traversal of the new tree sees it too.
    assert!(
        monitor
            .store()
            .list_documents()
            .expect("enumeration")
            .contains(&(node(), doc())),
        "the document is not enumerable in the namespace"
    );
}

#[test]
fn a_conditional_write_reaches_both_trees_too() {
    // The other monitor entry point, and the one that would have been easy to miss
    // because it goes through a different function in the monitor.
    let (_vfs, monitor) = monitored();
    monitor
        .write(node(), doc(), body("v1"))
        .expect("the seed commits");

    monitor
        .conditional_write(node(), doc(), body("v2"), body("v1").digest())
        .expect("the conditional write is accepted");

    assert_eq!(
        via_namespace(monitor.store()).as_deref(),
        Some(b"v2".as_slice()),
        "a conditional write did not reach the new trees"
    );
}

#[test]
fn overwriting_does_not_strand_the_version_it_replaced() {
    // The leak this path would otherwise have on the **success** path, not merely after a
    // crash: without a collection, every overwrite would leave the previous blob behind
    // forever. On the ledger's hot path that is unbounded growth.
    let (vfs, monitor) = monitored();
    for generation in 1..=5 {
        monitor
            .write(node(), doc(), body(&format!("v{generation}")))
            .expect("the write commits");
        assert_eq!(
            blob_count(&vfs, monitor.store()),
            monitor.store().blob_replica_count(),
            "generation {generation} left a superseded blob behind"
        );
    }
    assert_eq!(
        via_namespace(monitor.store()).as_deref(),
        Some(b"v5".as_slice())
    );
}

#[test]
fn the_pointer_never_names_a_blob_that_is_not_there() {
    // `D327`'s invariant on a path with no journal to fall back on. The write is interrupted
    // at every rename it performs; after each, whatever the namespace names must resolve.
    let mut faulted = 0;
    for cut in 1..=18 {
        let gate = FaultGate::closed();
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
            MemFs::new(),
            FaultPolicy::new().inject_gated(
                FaultOp::Rename,
                PathMatcher::predicate(|_| true),
                Fault::io_error(),
                When::OnCall(cut),
                gate.clone(),
            ),
        ));
        let root = PathBuf::from("shire");
        vfs.create_dir_all(&root).unwrap();
        let store = Store::init(
            vfs.clone(),
            root,
            GenesisConfig::new(ShireId::new("cvs23-shire").unwrap(), 3),
        )
        .expect("init");

        store
            .write_document_replicated(node(), &doc(), &body("v1"), &Nonce::new("SEED").unwrap())
            .expect("the unfaulted seed write commits");

        gate.open();
        if store
            .write_document_replicated(node(), &doc(), &body("v2"), &Nonce::new("T2").unwrap())
            .is_err()
        {
            faulted += 1;
        }

        // There is no recovery to lean on here, so the invariant has to hold as the write
        // left it. Whichever pointer the majority holds -- the old one or the new -- its blob
        // was made durable before that pointer moved.
        let read = store
            .read_pointer(&node(), &doc())
            .unwrap_or_else(|e| panic!("cut {cut}: the namespace became unreadable: {e:?}"));
        let bytes = store
            .read_blob(&read.pointer().target())
            .unwrap_or_else(|e| {
                panic!("cut {cut}: the namespace names a blob that is not there: {e:?}")
            })
            .into_bytes();
        assert!(
            bytes == b"v1".to_vec() || bytes == b"v2".to_vec(),
            "cut {cut}: the document resolves to a body nothing ever wrote"
        );
    }
    assert!(
        faulted > 0,
        "no cut point interrupted the write; the fault is not reaching this path"
    );
}

#[test]
fn an_empty_body_round_trips() {
    // Zero bytes is still a body: it has a digest, it is ingested, and the pointer names it.
    // The failure this guards is treating an empty body as an absent one somewhere along the
    // write, which would read back as a document that was never written.
    let (_vfs, monitor) = monitored();
    monitor
        .write(node(), doc(), DocumentBody::new(Vec::new()))
        .expect("the write commits");

    assert_eq!(
        via_namespace(monitor.store()).as_deref(),
        Some(b"".as_slice())
    );
}

#[test]
fn a_node_holds_multiple_documents_independently() {
    // Two documents under one node share a node directory in the namespace but have distinct
    // path hashes, so neither write may disturb the other's pointer or blob.
    let (_vfs, monitor) = monitored();
    let other = DocumentName::new("ENGINE").expect("a valid document name");
    monitor
        .write(node(), doc(), body("the record"))
        .expect("the write commits");
    monitor
        .write(node(), other.clone(), body("pid+start"))
        .expect("the write commits");

    let store = monitor.store();
    assert_eq!(
        canopy::read_once(store, &node(), &doc()).expect("read"),
        Some(body("the record"))
    );
    assert_eq!(
        canopy::read_once(store, &node(), &other).expect("read"),
        Some(body("pid+start"))
    );
}
