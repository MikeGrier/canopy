// Copyright (c) Michael Grier.

//! `CVS-14` integration test: a pointer never names a blob that is not there (`D327`).
//!
//! The log shrinking from bodies to digests is the visible half of `CVS-14` and is covered by
//! unit tests. This file covers the half that matters: **blobs are durable before any pointer
//! names them.** A pointer whose blob is absent is a *dangling pointer*, and `D315` records
//! that nothing in the store detects or repairs one -- the content-addressed layout cannot
//! even describe the damage, because a blob's name is derived from its content, so a missing
//! blob is indistinguishable from one that was never written.
//!
//! An invariant that only holds when nothing goes wrong is not an invariant, so this does not
//! test the happy path and conclude. It interrupts a transaction at **every** rename it
//! performs, one at a time, and after each interruption asserts that every pointer on disk --
//! on *any* replica, not merely a winning majority -- names a blob that is present and
//! verifies. The ordering is a property of the phase structure rather than of a check, so the
//! way to test it is to cut the phases apart at each seam and look at what is left.
//!
//! **Every replica, deliberately.** Checking only the majority would let a half-written
//! pointer set pass unexamined, and a minority pointer is still a real pointer sitting in the
//! store -- a scrub or a later repair reads it, and an operator diagnosing a fault finds it.

use std::path::PathBuf;
use std::sync::Arc;

use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, Vfs, When};
use canopy::{
    Artifact, BlobName, DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, PathHash,
    Pointer, ShireId, Store, Swap, SwapSet,
};

/// Renames to interrupt, one per run.
///
/// A transaction over two writes performs, per write, `blob_replica_count` blob commits,
/// `partition_count` document commits and `pointer_replica_count` pointer commits -- 18 at the
/// default widths. The bound overshoots so the tail runs interrupt nothing and exercise the
/// uninterrupted path, which is the control: if every run were faulted, a bug that simply
/// wrote no pointers at all would satisfy every assertion here.
const CUT_POINTS: u32 = 24;

/// The documents one transaction writes. Two, so the interruption can land between them and
/// leave one target complete and the other not.
fn targets() -> Vec<(NodePath, DocumentName, DocumentBody)> {
    vec![
        (
            NodePath::parse("tickets/alpha").unwrap(),
            DocumentName::new("record.json").unwrap(),
            DocumentBody::new(b"alpha body".to_vec()),
        ),
        (
            NodePath::parse("tickets/bravo").unwrap(),
            DocumentName::new("record.json").unwrap(),
            DocumentBody::new(b"bravo body -- deliberately a different length".to_vec()),
        ),
    ]
}

fn swaps() -> SwapSet {
    SwapSet::new(
        targets()
            .into_iter()
            .map(|(node, document, body)| Swap::new(node, document, body)),
    )
    .expect("distinct targets")
}

/// A store whose `k`-th rename fails once the returned gate is opened.
///
/// The gate stays shut through `init` so the store's own setup is never faulted, and `OnCall`
/// counts only calls that match while the gate is open -- so `k` numbers the renames of the
/// transaction alone.
fn store_faulting_rename(k: u32) -> (Arc<dyn Vfs>, Store, FaultGate) {
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::predicate(|_| true),
            Fault::io_error(),
            When::OnCall(k),
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let config = GenesisConfig::new(ShireId::new("cvs14-shire").unwrap(), 3);
    let store = Store::init(vfs.clone(), root, config).expect("init");
    (vfs, store, gate)
}

/// Replica `index`'s namespace entry for (`node`, `document`).
fn pointer_path(store: &Store, index: usize, node: &NodePath, document: &DocumentName) -> PathBuf {
    store
        .root()
        .join("namespace")
        .join(format!("p{index}"))
        .join(node.relative_path())
        .join(document.as_str())
}

/// Asserts that every pointer on every replica names a blob that is present and reads back.
///
/// Returns how many pointers it found, so a caller can tell "the invariant held" from "there
/// was nothing to check".
fn assert_no_dangling_pointers(vfs: &Arc<dyn Vfs>, store: &Store, at: u32) -> usize {
    let mut found = 0;
    for (node, document, body) in targets() {
        for index in 0..store.pointer_replica_count() {
            let path = pointer_path(store, index, &node, &document);
            let Ok(bytes) = vfs.read(&path) else {
                continue;
            };
            found += 1;

            let pointer = Pointer::parse(&bytes, PathHash::of(&node, &document))
                .unwrap_or_else(|e| panic!("cut {at}: replica {index} holds an unreadable pointer for {node}/{}: {e:?}", document.as_str()));

            // The pointer exists, so its blob must. `read_blob` verifies each copy against
            // the digest in the name, so this is not merely a presence check: a blob that is
            // there but wrong fails here too.
            let read = store.read_blob(&pointer.target()).unwrap_or_else(|e| {
                panic!(
                    "cut {at}: dangling pointer at replica {index} for {node}/{} -- the blob it \
                     names cannot be read: {e:?}",
                    document.as_str()
                )
            });
            assert_eq!(
                read.bytes(),
                body.as_bytes(),
                "cut {at}: replica {index} pointer for {node}/{} resolves to the wrong body",
                document.as_str()
            );
        }
    }
    found
}

#[test]
fn no_interruption_leaves_a_pointer_naming_a_blob_that_is_not_there() {
    let mut interrupted = 0;
    let mut completed = 0;
    let mut pointers_seen = 0;

    for k in 1..=CUT_POINTS {
        let (vfs, store, gate) = store_faulting_rename(k);
        gate.open();

        match store.apply_transaction(&swaps(), &Nonce::new("T1").unwrap()) {
            Ok(_) => completed += 1,
            Err(_) => interrupted += 1,
        }

        // Asserted whatever the outcome. A transaction that failed still leaves whatever it
        // had already committed on disk, and that residue is exactly what the invariant is
        // about -- checking only the failures would miss a clean run that wrote a pointer
        // before its blob.
        pointers_seen += assert_no_dangling_pointers(&vfs, &store, k);
    }

    // The control described above: the run must have both faulted and not faulted, or the
    // assertions above were vacuous in one direction.
    assert!(
        interrupted > 0,
        "no cut point interrupted the transaction; the fault is not reaching the apply path"
    );
    assert!(
        completed > 0,
        "every cut point interrupted the transaction; raise CUT_POINTS past the rename count"
    );
    assert!(
        pointers_seen > 0,
        "no pointer was ever written, so the invariant was never actually tested"
    );
}

#[test]
fn a_committed_transaction_leaves_each_document_reachable_through_its_pointer() {
    // The happy path stated as the client-visible chain it creates: a transaction writes the
    // namespace entry, the entry names a blob, and the blob is the body that was written.
    // Without this, the test above is satisfied by a store that writes no pointers at all.
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        vfs,
        root,
        GenesisConfig::new(ShireId::new("cvs14-shire").unwrap(), 3),
    )
    .expect("init");

    store
        .apply_transaction(&swaps(), &Nonce::new("T1").unwrap())
        .expect("an uncontended transaction commits");

    for (node, document, body) in targets() {
        let read = store.read_pointer(&node, &document).expect("a pointer");
        assert!(
            read.is_unanimous(),
            "every replica should carry the pointer a completed transaction wrote"
        );
        let blob = store.read_blob(&read.pointer().target()).expect("the blob");
        assert_eq!(blob.bytes(), body.as_bytes());
        assert!(
            blob.was_first_choice(),
            "a freshly written blob should verify on the first replica tried"
        );
    }
}

#[test]
fn a_rejected_transaction_writes_neither_blob_nor_pointer() {
    // Preconditions are phase 1, before any blob is ingested, so a rejection must leave the
    // content store untouched rather than littering it with blobs for a transaction that
    // never happened. Nothing collects those blobs until `CVS-15`, so the cheap guarantee is
    // that they are never created.
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        vfs.clone(),
        root,
        GenesisConfig::new(ShireId::new("cvs14-shire").unwrap(), 3),
    )
    .expect("init");

    let (node, document, body) = targets().remove(0);
    // Where the blob would land if phase 2 ever ran for this swap.
    let would_be = BlobName::new(
        PathHash::of(&node, &document),
        body.digest(),
        Artifact::Body,
    )
    .relative_path(store.shard_width());

    let doomed = SwapSet::single(Swap::conditional(
        node.clone(),
        document.clone(),
        body,
        DocumentBody::new(b"a body this document has never held".to_vec()).digest(),
    ));

    let outcome = store
        .apply_transaction(&doomed, &Nonce::new("T1").unwrap())
        .expect("a failed precondition is a rejection, not an error");
    assert!(
        !matches!(outcome, canopy::TransactionOutcome::Committed),
        "the precondition cannot hold against an absent document"
    );

    for index in 0..store.pointer_replica_count() {
        assert!(
            !vfs.exists(&pointer_path(&store, index, &node, &document))
                .unwrap(),
            "replica {index} holds a pointer for a transaction that was rejected"
        );
    }
    for index in 0..store.blob_replica_count() {
        let path = store
            .root()
            .join("files")
            .join(format!("p{index}"))
            .join(&would_be);
        assert!(
            !vfs.exists(&path).unwrap(),
            "replica {index} holds a blob for a transaction that was rejected: {path:?}"
        );
    }
}
