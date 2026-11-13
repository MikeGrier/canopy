// Copyright (c) Michael Grier.

//! The content store end to end at scale: ingest, read, verify, delete.
//!
//! Drives hundreds of documents and versions through the blob path against `acadia`'s
//! `MemFs`, then injects corruption across replicas and proves the read identifies a bad
//! copy, falls through, and serves correct bytes.
//!
//! **What distinguishes this from a quorum store**, and what the scale is here to show: a
//! copy is convicted by examining it alone (`D316`). Corrupting a *majority* of the copies
//! of a document changes nothing about what a read returns, because there is no vote to
//! lose. A store that byte-compared copies would fail every assertion below.
//!
//! Repair is not exercised here; what this file pins is that the read *identifies* the failing
//! replicas, which is the input a repair consumes.
//!
//! This module's time budget is 2 s rather than the usual 1 s, kept by populating one store
//! per replica count and forking it per test (DESIGN-NOTES.md, "The content-store scale tests
//! are unit tests with a 2 s budget").

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use acadia::{MemFs, Vfs};

use crate::{
    Artifact, BlobName, BlobRef, ContentDigest, DocumentName, GenesisConfig, NodePath, Nonce,
    ShireId, Store, StoreError,
};

/// Documents driven through the store.
const DOCUMENTS: usize = 400;

/// Versions written per document, so supersession and the sweep see real depth.
const VERSIONS: usize = 3;

/// Where every store here is rooted.
const ROOT: &str = "shire";

/// A new store with `replicas` replicas on `vfs`.
fn init(vfs: Arc<dyn Vfs>, replicas: usize) -> Store {
    let root = PathBuf::from(ROOT);
    vfs.create_dir_all(&root).unwrap();
    let config = GenesisConfig::new(ShireId::new("cvs9-shire").unwrap(), 11)
        .with_blob_replica_count(replicas)
        .unwrap();
    Store::init_unsourced(vfs, root, config).unwrap()
}

/// A populated store's filesystem and the names it holds, built once and forked per test.
struct Fixture {
    fs: Arc<MemFs>,
    names: Vec<Vec<BlobName>>,
}

/// A store over a private copy of a fully populated store with `replicas` replicas.
///
/// Populating is most of a test's cost, and every test would populate identically, so the
/// first caller builds the fixture and each test opens a fork of it.
fn populated(replicas: usize) -> (Arc<dyn Vfs>, Store, Vec<Vec<BlobName>>) {
    static THREE: OnceLock<Fixture> = OnceLock::new();
    static FIVE: OnceLock<Fixture> = OnceLock::new();
    let cell = match replicas {
        3 => &THREE,
        5 => &FIVE,
        _ => panic!("no fixture with {replicas} replicas"),
    };
    let fixture = cell.get_or_init(|| {
        let fs = Arc::new(MemFs::new());
        let store = init(fs.clone(), replicas);
        let names = populate(&store);
        Fixture { fs, names }
    });
    let vfs: Arc<dyn Vfs> = Arc::new(fixture.fs.fork());
    let store = Store::open_unsourced(vfs.clone(), ROOT).unwrap();
    (vfs, store, fixture.names.clone())
}

fn node_of(i: usize) -> NodePath {
    NodePath::parse(&format!("tickets/node{i:04}")).unwrap()
}

fn doc_of(i: usize) -> DocumentName {
    DocumentName::new(&format!("record{i:04}.json")).unwrap()
}

/// The body of version `v` of document `i` -- distinct per (document, version) so a
/// mis-served blob is detectable rather than coincidentally equal.
fn body_of(i: usize, v: usize) -> Vec<u8> {
    format!("document {i:04} version {v} -- {}", "x".repeat(i % 97)).into_bytes()
}

/// Where partition `index` keeps `name`.
fn replica_path(store: &Store, index: usize, name: &BlobName) -> PathBuf {
    store
        .root()
        .join("files")
        .join(format!("p{index}"))
        .join(name.relative_path(store.shard_width()))
}

fn reference(store: &Store, name: &BlobName) -> BlobRef {
    BlobRef::parse(&name.relative_path(store.shard_width())).expect("our own path parses")
}

/// Ingests every version of every document, returning the name of each.
fn populate(store: &Store) -> Vec<Vec<BlobName>> {
    let mut names = Vec::with_capacity(DOCUMENTS);
    for i in 0..DOCUMENTS {
        let mut versions = Vec::with_capacity(VERSIONS);
        for v in 0..VERSIONS {
            let nonce = Nonce::new(&format!("N{i}-{v}")).unwrap();
            versions.push(
                store
                    .ingest_blob(
                        &node_of(i),
                        &doc_of(i),
                        Artifact::Body,
                        body_of(i, v).as_slice(),
                        &nonce,
                    )
                    .expect("ingest"),
            );
        }
        names.push(versions);
    }
    names
}

#[test]
fn every_ingested_version_reads_back_exactly() {
    let (_vfs, store, names) = populated(3);

    for (i, versions) in names.iter().enumerate() {
        for (v, name) in versions.iter().enumerate() {
            let read = store.read_blob(&reference(&store, name)).expect("read");
            assert_eq!(read.bytes(), body_of(i, v).as_slice(), "document {i} v{v}");
            assert!(read.was_first_choice());
        }
    }
}

#[test]
fn every_blob_hashes_to_the_name_it_is_stored_under() {
    // The property that makes a blob self-validating, checked over the whole store rather
    // than one hand-placed example.
    let (vfs, store, names) = populated(3);

    for versions in &names {
        for name in versions {
            for index in 0..store.blob_replica_count() {
                let bytes = vfs.read(&replica_path(&store, index, name)).unwrap();
                assert_eq!(ContentDigest::of(&bytes), name.digest());
            }
        }
    }
}

#[test]
fn documents_never_share_a_blob_even_when_their_content_is_identical() {
    // `D315` enforces non-sharing with the owner hash rather than merely intending it.
    // Every document here gets the same body, so a content-addressed store would collapse
    // them all onto one file.
    let store = init(Arc::new(MemFs::new()), 3);
    let mut names = BTreeSet::new();
    for i in 0..DOCUMENTS {
        let nonce = Nonce::new(&format!("S{i}")).unwrap();
        let name = store
            .ingest_blob(
                &node_of(i),
                &doc_of(i),
                Artifact::Body,
                &b"every document has exactly these bytes"[..],
                &nonce,
            )
            .unwrap();
        names.insert(name.file_name());
    }
    assert_eq!(
        names.len(),
        DOCUMENTS,
        "blobs were shared between documents"
    );
}

#[test]
fn a_read_falls_through_corruption_spread_across_every_replica() {
    // Faults are spread rather than confined to one partition: each document loses a
    // different replica, so over the store every partition is both a victim and a
    // survivor. A read that preferred a fixed partition would pass this only by luck.
    let (vfs, store, names) = populated(3);
    let replicas = store.blob_replica_count();

    for (i, versions) in names.iter().enumerate() {
        for name in versions {
            let victim = i % replicas;
            vfs.write(&replica_path(&store, victim, name), b"rot")
                .unwrap();
        }
    }

    for (i, versions) in names.iter().enumerate() {
        for (v, name) in versions.iter().enumerate() {
            let read = store.read_blob(&reference(&store, name)).expect("read");
            assert_eq!(read.bytes(), body_of(i, v).as_slice());
            assert_ne!(
                read.served_by(),
                i % replicas,
                "the corrupt replica must never be the one that served"
            );
        }
    }
}

#[test]
fn corrupting_a_majority_changes_nothing() {
    // The assertion a quorum store cannot make. Two of three copies are rotted, so a
    // byte-comparing read would take the corrupt pair as truth; a verifying read convicts
    // both by examining them and serves the survivor (`D316`).
    let (vfs, store, names) = populated(3);

    for versions in &names {
        for name in versions {
            for index in [0usize, 1] {
                vfs.write(&replica_path(&store, index, name), b"agreed but wrong")
                    .unwrap();
            }
        }
    }

    for (i, versions) in names.iter().enumerate() {
        for (v, name) in versions.iter().enumerate() {
            let read = store.read_blob(&reference(&store, name)).expect("read");
            assert_eq!(read.bytes(), body_of(i, v).as_slice());
            assert_eq!(read.served_by(), 2, "only partition 2 still verifies");
        }
    }
}

#[test]
fn the_read_names_the_replicas_a_repair_will_need() {
    // What `CVS-18` consumes. The read does not repair, but it must report precisely which
    // copies failed and how, or the repair path would have to re-discover it.
    let (vfs, store, names) = populated(5);

    for versions in &names {
        for name in versions {
            vfs.write(&replica_path(&store, 0, name), b"rot").unwrap();
            vfs.remove_file(&replica_path(&store, 1, name)).unwrap();
        }
    }

    let mut saw_corrupt = 0;
    let mut saw_missing = 0;
    for versions in &names {
        for name in versions {
            let read = store.read_blob(&reference(&store, name)).expect("read");
            assert!(!read.corrupt().contains(&read.served_by()));
            assert!(!read.missing().contains(&read.served_by()));
            if read.corrupt().contains(&0) {
                saw_corrupt += 1;
            }
            if read.missing().contains(&1) {
                saw_missing += 1;
            }
        }
    }
    // Rotation means not every read passes over the faulted partitions, but across the
    // whole store both faults must have been observed and classified.
    assert!(saw_corrupt > 0, "no read ever reported the rotted replica");
    assert!(saw_missing > 0, "no read ever reported the absent replica");
}

#[test]
fn a_document_whose_every_replica_is_gone_is_unavailable_rather_than_wrong() {
    // canopy invents no body. With no verified copy the read fails, and says which way.
    let (vfs, store, names) = populated(3);
    let doomed = &names[7][0];

    for index in 0..store.blob_replica_count() {
        vfs.write(&replica_path(&store, index, doomed), b"all rotted")
            .unwrap();
    }

    match store.read_blob(&reference(&store, doomed)) {
        Err(StoreError::BlobUnavailable {
            corrupt, missing, ..
        }) => {
            assert_eq!(corrupt, 3);
            assert_eq!(missing, 0);
        }
        other => panic!("expected an unavailable blob, got {other:?}"),
    }

    // And nothing else was disturbed.
    let survivor = &names[8][0];
    assert_eq!(
        store
            .read_blob(&reference(&store, survivor))
            .unwrap()
            .bytes(),
        body_of(8, 0).as_slice()
    );
}

#[test]
fn sweeping_to_the_latest_version_leaves_exactly_one_blob_per_document() {
    let (vfs, store, names) = populated(3);

    let mut removed = 0;
    for versions in &names {
        let latest = versions.last().unwrap();
        removed += store
            .retain_blobs(latest.path_hash(), &[latest.digest()])
            .expect("sweep");
    }

    // Two superseded versions per document, in each of three replicas.
    assert_eq!(removed, DOCUMENTS * (VERSIONS - 1) * 3);

    for (i, versions) in names.iter().enumerate() {
        let latest = versions.last().unwrap();
        assert_eq!(
            store.read_blob(&reference(&store, latest)).unwrap().bytes(),
            body_of(i, VERSIONS - 1).as_slice()
        );
        for superseded in &versions[..VERSIONS - 1] {
            assert!(
                store.read_blob(&reference(&store, superseded)).is_err(),
                "document {i}: a superseded version survived the sweep"
            );
            for index in 0..store.blob_replica_count() {
                assert!(
                    vfs.metadata(&replica_path(&store, index, superseded))
                        .is_err()
                );
            }
        }
    }
}

#[test]
fn deleting_every_document_empties_the_blob_tree_of_our_blobs() {
    let (vfs, store, names) = populated(3);

    let mut removed = 0;
    for versions in &names {
        removed += store
            .retain_blobs(versions[0].path_hash(), &[])
            .expect("sweep");
    }
    assert_eq!(removed, DOCUMENTS * VERSIONS * 3);

    for versions in &names {
        for name in versions {
            for index in 0..store.blob_replica_count() {
                assert!(vfs.metadata(&replica_path(&store, index, name)).is_err());
            }
        }
    }
}

#[test]
fn a_sweep_after_corruption_still_removes_the_corrupt_copies() {
    // A rotted blob is still a blob the owner holds: the sweep matches on the name, which
    // is intact, not on the content, which is not. Corruption must not make garbage
    // immortal.
    let (vfs, store, names) = populated(3);

    for versions in &names {
        for name in versions {
            vfs.write(&replica_path(&store, 1, name), b"rot").unwrap();
        }
    }

    let mut removed = 0;
    for versions in &names {
        removed += store
            .retain_blobs(versions[0].path_hash(), &[])
            .expect("sweep");
    }
    assert_eq!(removed, DOCUMENTS * VERSIONS * 3);
}

#[test]
fn the_full_cycle_holds_over_the_whole_store() {
    // Ingest, read, corrupt, read through the corruption, supersede, sweep, read again --
    // the sequence a document actually lives through, at scale.
    let (vfs, store, names) = populated(3);

    for (i, versions) in names.iter().enumerate() {
        let victim = i % 3;
        for name in versions {
            vfs.write(&replica_path(&store, victim, name), b"rot")
                .unwrap();
        }
    }

    for i in 0..DOCUMENTS {
        let nonce = Nonce::new(&format!("F{i}")).unwrap();
        let fresh_body = format!("document {i:04} final");
        let latest = store
            .ingest_blob(
                &node_of(i),
                &doc_of(i),
                Artifact::Body,
                fresh_body.as_bytes(),
                &nonce,
            )
            .expect("ingest");

        // A fresh ingest rewrites every replica, so the document is whole again even
        // though two thirds of its history is rotted.
        let read = store.read_blob(&reference(&store, &latest)).expect("read");
        assert_eq!(read.bytes(), fresh_body.as_bytes());
        assert!(read.was_first_choice());

        assert!(
            store
                .retain_blobs(latest.path_hash(), &[latest.digest()])
                .expect("sweep")
                > 0
        );
        assert_eq!(
            store
                .read_blob(&reference(&store, &latest))
                .unwrap()
                .bytes(),
            fresh_body.as_bytes()
        );
    }
}
