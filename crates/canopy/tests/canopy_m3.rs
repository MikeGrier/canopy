// Copyright (c) Michael Grier.

//! CANOPY-M3 crash-recovery integration test (TXN-7): drive the D53 two-phase
//! commit through its phase boundaries at scale, injecting a fault that aborts an
//! apply part-way, then reopen the store and prove recovery reconciles it.
//!
//! Crashes are modelled by interrupting a real [`apply_transaction`] through the
//! `acadia` [`FaultyFs`] seam (D55): a single injected fault fails one filesystem
//! operation, so the public apply path aborts exactly at a chosen phase boundary --
//! after `begin` but before the `prepare`/commit point, or after `prepare` while the
//! cross-partition renames are in progress -- leaving the on-disk log and staging in
//! the same shape a crash there would. Reopening the store then runs the D46/TXN-4
//! recovery replay over that state. This is the recovery-*logic* complement to the
//! power-loss durability stress that CANOPY-M4's fault-scripted harness will add; see
//! [DESIGN-NOTES.md](../../../DESIGN-NOTES.md) decision D74.
//!
//! The scenarios exercised:
//! - a crash after the commit point rolls the whole transaction **forward** (every
//!   document lands in every partition), idempotently across repeated opens;
//! - a crash before the commit point rolls the whole transaction **back** (no document
//!   is committed) -- the all-or-nothing guarantee's rollback half;
//! - a crash mid-rename completes the remaining partitions on recovery;
//! - a cleanly applied (ended) transaction reopens as an idempotent sweep;
//! - a cleanly quiesced restart reads an empty log and skips replay (the O(1)-startup
//!   commitment, TXN-3).

use std::path::PathBuf;
use std::sync::Arc;

use acadia::{Fault, FaultOp, FaultPolicy, FaultyFs, MemFs, Vfs, VfsErrorKind, When};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, ShireId, Store, StoreOptions, Swap,
    SwapSet, read_once,
};

/// The store root, created before init like the LAY-8 harness.
const ROOT: &str = "shire";
/// The single document name every node in these tests carries.
const DOC: &str = "d.json";
/// The commit log's on-disk name at the store root (D53), read directly to prove the
/// quiescent fast path leaves it empty.
const LOG_NAME: &str = ".commit-log";

/// The node holding document `i`, canonical `NODEnnn`.
fn node_name(i: usize) -> NodePath {
    NodePath::parse(&format!("node{i:03}")).unwrap()
}

/// The shared document name.
fn doc_name() -> DocumentName {
    DocumentName::new(DOC).unwrap()
}

/// A document body from `s`.
fn body_of(s: &str) -> DocumentBody {
    DocumentBody::new(s.as_bytes().to_vec())
}

/// The genesis for a store replicated `count` ways.
fn genesis(count: usize) -> GenesisConfig {
    GenesisConfig::new(ShireId::new("it-shire").unwrap(), 7)
        .with_pointer_replica_count(count)
        .unwrap()
        .with_blob_replica_count(count)
        .unwrap()
}

/// Options drawing nonces `N1`, `N2`, ... .
fn options() -> StoreOptions {
    let mut n: u64 = 0;
    StoreOptions::with_nonce_source(move || {
        n += 1;
        Nonce::new(&format!("N{n}")).unwrap()
    })
}

/// A fresh `count`-partition store on a plain in-memory filesystem, plus the shared
/// backend handle for reopening it.
fn fresh_store(count: usize) -> (Arc<MemFs>, Store) {
    let mem = Arc::new(MemFs::new());
    let vfs: Arc<dyn Vfs> = mem.clone();
    vfs.create_dir_all(&PathBuf::from(ROOT)).unwrap();
    let store = Store::init(mem.clone(), PathBuf::from(ROOT), genesis(count), options()).unwrap();
    (mem, store)
}

/// A fresh `count`-partition store whose filesystem applies `policy`, plus the
/// [`FaultyFs`] handle for reopening it (the fault has fired by then, so recovery
/// runs unimpeded).
fn faulty_store(count: usize, policy: FaultPolicy) -> (Arc<FaultyFs<MemFs>>, Store) {
    let faulty = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&PathBuf::from(ROOT)).unwrap();
    let store = Store::init(
        faulty.clone(),
        PathBuf::from(ROOT),
        genesis(count),
        options(),
    )
    .unwrap();
    (faulty, store)
}

/// The on-disk directory holding namespace replica `index`'s entries for `node` -- the
/// target the commit-phase `fsync` (and staging-phase `create_dir_all`) operates on.
///
/// Mirrors the store's published layout so a fault rule can name the exact path an apply
/// will touch, before the store exists to be asked.
fn node_dir_path(index: usize, node: &NodePath) -> PathBuf {
    PathBuf::from(ROOT)
        .join("namespace")
        .join(format!("p{index}"))
        .join(node.relative_path())
}

/// Blob replica `index`'s staging directory, created by the first ingest of a transaction.
///
/// Phase 2 of an apply is the ingest, and it is the only work between `begin` and the
/// `prepare` commit point -- so this is where a fault models a crash that must roll back.
fn blob_staging_dir(index: usize) -> PathBuf {
    PathBuf::from(ROOT)
        .join("files")
        .join(format!("p{index}"))
        .join(".staging")
}

/// Reopens the store at [`ROOT`] over `vfs`, running the recovery replay.
///
/// The caller must have dropped the previous handle: since `D116` a root has one writer, and
/// a "reopen" that kept the old handle alive was never a restart.
fn reopen(vfs: Arc<dyn Vfs>) -> Store {
    Store::open(vfs, PathBuf::from(ROOT), options()).unwrap()
}

#[test]
fn a_crash_after_prepare_rolls_the_whole_transaction_forward() {
    const PARTITIONS: usize = 5;
    const DOCS: usize = 200;
    // Fail the durability fsync of the very first committed rename: the apply reaches
    // the prepare/commit point, then aborts with the transaction prepared but unended.
    let policy = FaultPolicy::new().inject(
        FaultOp::Fsync,
        node_dir_path(0, &node_name(0)),
        Fault::io_error(),
        When::Once,
    );
    let (faulty, store) = faulty_store(PARTITIONS, policy);
    let doc = doc_name();
    let swaps = SwapSet::new(
        (0..DOCS).map(|i| Swap::new(node_name(i), doc.clone(), body_of(&format!("body-{i}")))),
    )
    .unwrap();
    store.apply_transaction(&swaps).unwrap_err();

    // Reopen: recovery replays the prepared swap set, rolling every document forward
    // to every partition.
    drop(store);
    let recovered = reopen(faulty.clone());
    for i in 0..DOCS {
        let read = read_once(&recovered, &node_name(i), &doc).unwrap();
        assert_eq!(
            read,
            Some(body_of(&format!("body-{i}"))),
            "doc {i} did not roll forward"
        );
    }

    // Reopening again is an idempotent sweep no-op that keeps every body.
    drop(recovered);
    let again = reopen(faulty.clone());
    for i in 0..DOCS {
        assert_eq!(
            read_once(&again, &node_name(i), &doc).unwrap(),
            Some(body_of(&format!("body-{i}")))
        );
    }
}

#[test]
fn a_crash_before_prepare_rolls_the_whole_transaction_back() {
    const PARTITIONS: usize = 5;
    const DOCS: usize = 200;
    // Fail the blob ingest's staging directory creation: phase 2 is the only work that
    // happens between `begin` and the `prepare` commit point, so the apply aborts with
    // nothing committed. Faulting the namespace instead would land *after* the commit
    // point, where recovery correctly rolls forward rather than back.
    let policy = FaultPolicy::new().inject(
        FaultOp::CreateDirAll,
        blob_staging_dir(0),
        Fault::io_error(),
        When::Once,
    );
    let (faulty, store) = faulty_store(PARTITIONS, policy);
    let doc = doc_name();
    let swaps = SwapSet::new(
        (0..DOCS).map(|i| Swap::new(node_name(i), doc.clone(), body_of(&format!("body-{i}")))),
    )
    .unwrap();
    store.apply_transaction(&swaps).unwrap_err();

    // Reopen: recovery rolls the begun-but-unprepared transaction back -- no committed
    // copy exists for any document.
    drop(store);
    let recovered = reopen(faulty.clone());
    for i in 0..DOCS {
        let read = read_once(&recovered, &node_name(i), &doc).unwrap();
        assert_eq!(read, None, "doc {i} present after roll-back");
    }
    assert!(
        recovered.list_documents().unwrap().is_empty(),
        "the store holds documents after a full roll-back"
    );
}

#[test]
fn a_cleanly_applied_transaction_reopens_idempotently() {
    const PARTITIONS: usize = 5;
    const DOCS: usize = 200;
    let (mem, store) = fresh_store(PARTITIONS);
    let doc = doc_name();
    let swaps = SwapSet::new(
        (0..DOCS).map(|i| Swap::new(node_name(i), doc.clone(), body_of(&format!("body-{i}")))),
    )
    .unwrap();
    assert!(store.apply_transaction(&swaps).unwrap().is_committed());

    // Reopen twice: recovery sees the ended transaction and only sweeps residue,
    // keeping every committed body across repeated opens.
    drop(store);
    for _ in 0..2 {
        let recovered = reopen(mem.clone());
        for i in 0..DOCS {
            assert_eq!(
                read_once(&recovered, &node_name(i), &doc).unwrap(),
                Some(body_of(&format!("body-{i}")))
            );
        }
    }
}

#[test]
fn a_cleanly_quiesced_restart_reads_an_empty_log() {
    const PARTITIONS: usize = 5;
    const DOCS: usize = 200;
    let mem = Arc::new(MemFs::new());
    let vfs: Arc<dyn Vfs> = mem.clone();
    vfs.create_dir_all(&PathBuf::from(ROOT)).unwrap();
    let store = Store::init(
        mem.clone(),
        PathBuf::from(ROOT),
        genesis(PARTITIONS),
        options(),
    )
    .unwrap();
    let doc = doc_name();
    for i in 0..DOCS {
        store
            .update_document(Swap::new(
                node_name(i),
                doc.clone(),
                body_of(&format!("body-{i}")),
            ))
            .unwrap();
    }

    // Each write empties the log before releasing the writer gate, so once the last write
    // has returned the journal is empty.
    match mem.read(&PathBuf::from(ROOT).join(LOG_NAME)) {
        Ok(bytes) => assert!(bytes.is_empty(), "log not empty at quiescence"),
        Err(e) => assert_eq!(e.kind(), VfsErrorKind::NotFound),
    }
    drop(store);

    // Reopen: an empty log is the O(1) fast path -- recovery skips replay and every
    // document is intact.
    let recovered = reopen(mem.clone());
    for i in 0..DOCS {
        assert_eq!(
            read_once(&recovered, &node_name(i), &doc).unwrap(),
            Some(body_of(&format!("body-{i}")))
        );
    }
}
