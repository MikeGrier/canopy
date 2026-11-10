// Copyright (c) Michael Grier.

//! `CVS-16` integration test: rolling back across the split (`D329`).
//!
//! A transaction ingests its blobs during staging -- durably, deliberately, because `D327`
//! requires content to be on disk before the record that names it. A transaction that then
//! dies before its commit point therefore leaves content behind that nothing points at.
//!
//! **The tests here are built so that only the rollback can clean that up.** The abandoned
//! transaction writes a *different* document from any completed one, because a completed
//! transaction's own collection (`D328`) sweeps every blob its target owns and would happily
//! collect the orphan as a side effect -- passing the test while proving nothing. Isolating
//! the documents is what makes these tests about rollback rather than about `CVS-15`.
//!
//! The second test is the one that justifies recording *targets* in the `begin` record rather
//! than the blob names about to be written. An abandoned write whose content happens to equal
//! what the namespace already points at lands on the very same content-addressed name, so a
//! rollback that deleted "the blob this transaction ingested" would delete the live one.

use std::path::PathBuf;
use std::sync::Arc;

use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, Vfs, When};
use canopy::{
    BlobName, DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, PathHash, ShireId, Store,
    StoreError, Swap, SwapSet,
};

/// The document written by a transaction that completes.
fn settled() -> NodePath {
    NodePath::parse("tickets/settled").unwrap()
}

/// The document written by the transaction that is abandoned mid-flight.
fn abandoned() -> NodePath {
    NodePath::parse("tickets/abandoned").unwrap()
}

fn doc() -> DocumentName {
    DocumentName::new("record.json").unwrap()
}

fn body(text: &str) -> DocumentBody {
    DocumentBody::new(text.as_bytes().to_vec())
}

fn swaps(node: NodePath, text: &str) -> SwapSet {
    SwapSet::single(Swap::new(node, doc(), body(text)))
}

/// A store whose commit-log appends fail from the `k`-th one after the gate opens.
///
/// `OnCall(2)` is the shape these tests want: a transaction's `begin` lands and is fsynced,
/// staging runs and ingests the blob, and the `prepare` append then fails -- so the commit
/// point is never reached and the blob has no pointer. That is precisely the crash window
/// rollback exists for.
fn store_faulting_log_append(k: u32) -> (Arc<dyn Vfs>, PathBuf, Store, FaultGate) {
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Append,
            PathMatcher::has_component(".commit-log"),
            Fault::io_error(),
            When::OnCall(k),
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        vfs.clone(),
        root.clone(),
        GenesisConfig::new(ShireId::new("cvs16-shire").unwrap(), 3),
    )
    .expect("init");
    (vfs, root, store, gate)
}

/// The blob file names this document owns, across every replica.
fn blobs(vfs: &Arc<dyn Vfs>, store: &Store, node: &NodePath) -> Vec<String> {
    let path_hash = PathHash::of(node, &doc());
    let shard = path_hash.shard(store.shard_width());
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
            if BlobName::parse(&name).map(|blob| blob.path_hash()) == Ok(path_hash) {
                found.push(name);
            }
        }
    }
    found.sort();
    found
}

/// The body the namespace resolves this document to, or `None` when it is absent.
fn resolved(store: &Store, node: &NodePath) -> Option<Vec<u8>> {
    match store.read_pointer(node, &doc()) {
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
fn a_rolled_back_transaction_leaves_no_blob_behind() {
    let (vfs, root, store, gate) = store_faulting_log_append(2);
    store
        .apply_transaction(&swaps(settled(), "v1"), &Nonce::new("T1").unwrap())
        .expect("the first transaction commits");

    // From here the next transaction's `begin` lands but its `prepare` does not.
    gate.open();
    store
        .apply_transaction(&swaps(abandoned(), "v1"), &Nonce::new("T2").unwrap())
        .expect_err("a transaction whose prepare cannot be journalled cannot commit");

    // Staging ran, but the failure was before `prepare`, so the live rollback collected it
    // without waiting for a reopen, and the store stays writable.
    assert!(
        blobs(&vfs, &store, &abandoned()).is_empty(),
        "the live rollback left the abandoned blob behind"
    );
    assert!(!store.recovery_required());
    assert_eq!(resolved(&store, &abandoned()), None);

    gate.close();
    drop(store);
    let reopened = Store::open(vfs.clone(), root).expect("reopen rolls the transaction back");

    assert!(
        blobs(&vfs, &reopened, &abandoned()).is_empty(),
        "rollback left content no transaction ever committed: {:?}",
        blobs(&vfs, &reopened, &abandoned())
    );
    // The completed transaction is untouched by its neighbour's rollback.
    assert_eq!(
        resolved(&reopened, &settled()).as_deref(),
        Some(b"v1".as_slice())
    );
}

#[test]
fn a_rollback_keeps_the_blob_the_namespace_still_points_at() {
    // The reason `begin` records targets rather than blob names. This abandoned write carries
    // the content the document already holds, so it lands on the identical content-addressed
    // name -- there is only one blob, and it is live. A rollback that deleted "what this
    // transaction ingested" would delete it and leave the pointer dangling.
    let (vfs, root, store, gate) = store_faulting_log_append(2);
    store
        .apply_transaction(&swaps(settled(), "v1"), &Nonce::new("T1").unwrap())
        .expect("the first transaction commits");
    let before = blobs(&vfs, &store, &settled());

    gate.open();
    store
        .apply_transaction(&swaps(settled(), "v1"), &Nonce::new("T2").unwrap())
        .expect_err("a transaction whose prepare cannot be journalled cannot commit");

    gate.close();
    drop(store);
    let reopened = Store::open(vfs.clone(), root).expect("reopen rolls the transaction back");

    assert_eq!(
        resolved(&reopened, &settled()).as_deref(),
        Some(b"v1".as_slice()),
        "the rollback deleted the blob its own document still points at"
    );
    assert_eq!(
        blobs(&vfs, &reopened, &settled()),
        before,
        "the live blob set changed across a rollback that should not have touched it"
    );
}

#[test]
fn a_rolled_back_transaction_leaves_no_staging_in_any_tree() {
    // Each tree stages differently and each primitive sweeps only the paths it completes.
    // What a crash leaves behind is the paths it did not, in whichever tree it died in.
    let (vfs, root, store, gate) = store_faulting_log_append(2);
    gate.open();
    store
        .apply_transaction(&swaps(abandoned(), "v1"), &Nonce::new("T2").unwrap())
        .expect_err("a transaction whose prepare cannot be journalled cannot commit");

    gate.close();
    drop(store);
    let reopened = Store::open(vfs.clone(), root.clone()).expect("reopen rolls back");

    for index in 0..reopened.pointer_replica_count() {
        assert!(
            !vfs.exists(
                &root
                    .join("namespace")
                    .join(format!("p{index}"))
                    .join(".staging")
                    .join("T2")
            )
            .unwrap(),
            "namespace replica {index} kept its staging tree"
        );
    }
    for index in 0..reopened.blob_replica_count() {
        assert!(
            !vfs.exists(
                &root
                    .join("files")
                    .join(format!("p{index}"))
                    .join(".staging")
                    .join("blob.tmp.T2")
            )
            .unwrap(),
            "blob replica {index} kept its staging temp"
        );
    }
}

#[test]
fn a_transaction_that_dies_before_ingesting_anything_rolls_back_cleanly() {
    // `begin` lands, then the process dies before staging writes anything. There is nothing
    // to collect, and recovery must say so by completing rather than by failing on the
    // absence it finds.
    let (vfs, root, store, gate) = store_faulting_log_append(1);
    gate.open();
    store
        .apply_transaction(&swaps(abandoned(), "v1"), &Nonce::new("T2").unwrap())
        .expect_err("a transaction that cannot journal its begin cannot proceed");

    gate.close();
    drop(store);
    let reopened = Store::open(vfs.clone(), root).expect("reopen rolls back an empty transaction");

    assert!(blobs(&vfs, &reopened, &abandoned()).is_empty());
    assert_eq!(resolved(&reopened, &abandoned()), None);
}
