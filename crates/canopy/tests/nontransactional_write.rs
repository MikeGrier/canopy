// Copyright (c) Michael Grier.

//! `CVS-23` integration test: the non-transactional write path reaches both trees (`D332`).
//!
//! canopy's write path outside the commit log, `write_document_replicated`, journals nothing.
//! Callers drive it on their hot paths, so it must reach both the content store and the
//! namespace just as a logged commit does, or a traversal of the namespace would lose every
//! document written that way. The tests go through the public verb a caller uses.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, Vfs, When};
use canopy::{
    BlobName, DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, PathHash, ShireId, Store,
    StoreError, StoreOptions, Swap,
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

/// Options drawing nonces `N0`, `N1`, ... .
fn options() -> StoreOptions {
    let next = AtomicU64::new(0);
    StoreOptions::with_nonce_source(move || {
        Nonce::new(&format!("N{}", next.fetch_add(1, Ordering::Relaxed))).unwrap()
    })
}

/// A fresh store, plus the backend for inspecting what landed.
fn fresh() -> (Arc<dyn Vfs>, Store) {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        vfs.clone(),
        root,
        GenesisConfig::new(ShireId::new("cvs23-shire").unwrap(), 3),
        options(),
    )
    .expect("init");
    (vfs, store)
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
fn an_unlogged_write_is_reachable_through_the_namespace() {
    let (_vfs, store) = fresh();
    store
        .write_document_replicated(node(), &doc(), &body("v1"))
        .expect("the write commits");

    assert_eq!(
        via_namespace(&store).as_deref(),
        Some(b"v1".as_slice()),
        "an unlogged write did not reach the content store and the namespace"
    );
    // And the namespace enumerates it, so a traversal of the new tree sees it too.
    assert!(
        store
            .list_documents()
            .expect("enumeration")
            .contains(&(node(), doc())),
        "the document is not enumerable in the namespace"
    );
}

#[test]
fn a_compare_and_set_over_an_unlogged_write_reaches_both_trees_too() {
    let (_vfs, store) = fresh();
    store
        .write_document_replicated(node(), &doc(), &body("v1"))
        .expect("the seed commits");

    let commit = store
        .update_document(Swap::conditional(
            node(),
            doc(),
            body("v2"),
            body("v1").digest(),
        ))
        .expect("the conditional write is accepted");
    assert!(commit.is_committed());

    assert_eq!(
        via_namespace(&store).as_deref(),
        Some(b"v2".as_slice()),
        "a conditional write did not reach the new trees"
    );
}

#[test]
fn overwriting_does_not_strand_the_version_it_replaced() {
    // The leak this path would otherwise have on the **success** path, not merely after a
    // crash: without a collection, every overwrite would leave the previous blob behind
    // forever. On the ledger's hot path that is unbounded growth.
    let (vfs, store) = fresh();
    for generation in 1..=5 {
        store
            .write_document_replicated(node(), &doc(), &body(&format!("v{generation}")))
            .expect("the write commits");
        assert_eq!(
            blob_count(&vfs, &store),
            store.blob_replica_count(),
            "generation {generation} left a superseded blob behind"
        );
    }
    assert_eq!(via_namespace(&store).as_deref(), Some(b"v5".as_slice()));
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
            options(),
        )
        .expect("init");

        store
            .write_document_replicated(node(), &doc(), &body("v1"))
            .expect("the unfaulted seed write commits");

        gate.open();
        if store
            .write_document_replicated(node(), &doc(), &body("v2"))
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
    let (_vfs, store) = fresh();
    store
        .write_document_replicated(node(), &doc(), &DocumentBody::new(Vec::new()))
        .expect("the write commits");

    assert_eq!(via_namespace(&store).as_deref(), Some(b"".as_slice()));
}

#[test]
fn a_node_holds_multiple_documents_independently() {
    // Two documents under one node share a node directory in the namespace but have distinct
    // path hashes, so neither write may disturb the other's pointer or blob.
    let (_vfs, store) = fresh();
    let other = DocumentName::new("ENGINE").expect("a valid document name");
    store
        .write_document_replicated(node(), &doc(), &body("the record"))
        .expect("the write commits");
    store
        .write_document_replicated(node(), &other, &body("pid+start"))
        .expect("the write commits");

    assert_eq!(
        canopy::read_once(&store, &node(), &doc()).expect("read"),
        Some(body("the record"))
    );
    assert_eq!(
        canopy::read_once(&store, &node(), &other).expect("read"),
        Some(body("pid+start"))
    );
}
