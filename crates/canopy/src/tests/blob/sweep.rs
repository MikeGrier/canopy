// Copyright (c) Michael Grier.
// Split from blob.rs at 844ebb37.

//! `CVS-8`: blob deletion -- the set-difference sweep.

use super::*;

// -- sweeping (`D315`, `CVS-8`) ------------------------------------------------

/// Every file in `name`'s shard directory in partition `index`, sorted.
fn shard_entries(
    store: &Store,
    vfs: &Arc<dyn Vfs>,
    root: &std::path::Path,
    index: usize,
    name: &BlobName,
) -> Vec<String> {
    let shard = root
        .join(BLOB_TREE_DIR)
        .join(format!("p{index}"))
        .join(name.path_hash().shard(store.shard_width()));
    let mut names: Vec<String> = vfs
        .read_dir(&shard)
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.file_name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_sweep_removes_superseded_versions_and_keeps_the_survivor() {
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let v1 = ingest(&store, b"one", "n1");
    let v2 = ingest(&store, b"two", "n2");
    let v3 = ingest(&store, b"three", "n3");
    assert_eq!(shard_entries(&store, &vfs, &root, 0, &v3).len(), 3);

    let removed = store
        .retain_blobs(v3.path_hash(), &[v3.digest()])
        .expect("sweep");

    // Two superseded versions, in each of three replicas.
    assert_eq!(removed, 6);
    assert_eq!(
        shard_entries(&store, &vfs, &root, 0, &v3),
        vec![v3.file_name()]
    );
    assert_eq!(
        store.read_blob(&reference(&store, &v3)).unwrap().bytes(),
        b"three"
    );
    for stale in [&v1, &v2] {
        assert!(matches!(
            store.read_blob(&reference(&store, stale)),
            Err(StoreError::BlobUnavailable { missing: 3, .. })
        ));
    }
}

#[test]
fn an_empty_keep_set_removes_every_blob_the_document_owns() {
    // Which is what deleting a document is.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"doomed", "n1");
    let removed = store.retain_blobs(name.path_hash(), &[]).expect("sweep");

    assert_eq!(removed, 3);
    for index in 0..store.blob_replica_count() {
        assert!(shard_entries(&store, &vfs, &root, index, &name).is_empty());
    }
}

#[test]
fn a_rewrite_of_identical_content_survives_its_own_sweep() {
    // The case that makes "delete the old one" destructive and a set difference correct:
    // the old blob and the new one are the same file, so the naive phrasing would remove
    // the live one (`D315`).
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let first = ingest(&store, b"same", "n1");
    let second = ingest(&store, b"same", "n2");
    assert_eq!(first.file_name(), second.file_name());

    let removed = store
        .retain_blobs(second.path_hash(), &[second.digest()])
        .expect("sweep");

    assert_eq!(removed, 0, "there was nothing to remove");
    assert_eq!(
        store
            .read_blob(&reference(&store, &second))
            .unwrap()
            .bytes(),
        b"same"
    );
}

#[test]
fn a_sweep_collects_artifact_types_this_build_does_not_know() {
    // `D315`: removal is by prefix so cleanup stays complete as the vocabulary grows. If
    // the sweep assembled the names to remove, a blob written by a newer build would
    // survive every sweep an older one ran -- accumulating forever, owned by a version
    // that no longer exists.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let old = ingest(&store, b"old", "n1");
    let new = ingest(&store, b"new", "n2");

    // A sidecar of the superseded version, in an artifact type `Artifact` has never heard
    // of. `BlobName::parse` refuses it; the sweep must not.
    let width = store.shard_width();
    let future = format!(
        "{}.{}.CHUNKMAP",
        old.path_hash().render(),
        old.digest().to_base32()
    );
    assert!(BlobName::parse(&future).is_err(), "unknown to this build");
    for index in 0..store.blob_replica_count() {
        let shard = root
            .join(BLOB_TREE_DIR)
            .join(format!("p{index}"))
            .join(old.path_hash().shard(width));
        vfs.write(&shard.join(&future), b"a map of chunks").unwrap();
    }

    let removed = store
        .retain_blobs(new.path_hash(), &[new.digest()])
        .expect("sweep");

    assert_eq!(
        removed, 6,
        "the body and the unknown sidecar, in three copies"
    );
    assert_eq!(
        shard_entries(&store, &vfs, &root, 0, &new),
        vec![new.file_name()]
    );
}

#[test]
fn a_sweep_keeps_every_artifact_of_a_surviving_version() {
    // The complement: an unknown artifact of the *current* version is not collateral.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"current", "n1");
    let width = store.shard_width();
    let sidecar = format!(
        "{}.{}.CHUNKMAP",
        name.path_hash().render(),
        name.digest().to_base32()
    );
    let shard = root
        .join(BLOB_TREE_DIR)
        .join("p0")
        .join(name.path_hash().shard(width));
    vfs.write(&shard.join(&sidecar), b"still current").unwrap();

    let removed = store
        .retain_blobs(name.path_hash(), &[name.digest()])
        .expect("sweep");

    assert_eq!(removed, 0);
    assert!(shard_entries(&store, &vfs, &root, 0, &name).contains(&sidecar));
}

#[test]
fn a_sweep_does_not_touch_another_documents_blobs() {
    // Ownership is the whole point of the owner hash leading the name (`D315`).
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let mine = ingest(&store, b"mine", "n1");
    let theirs = store
        .ingest_blob(
            &node_of("TICKETS/B"),
            &doc_of("record.json"),
            Artifact::Body,
            &b"theirs"[..],
            &nonce_of("n2"),
        )
        .unwrap();

    let removed = store.retain_blobs(mine.path_hash(), &[]).expect("sweep");
    assert_eq!(removed, 3, "only mine");
    assert_eq!(
        store
            .read_blob(&reference(&store, &theirs))
            .unwrap()
            .bytes(),
        b"theirs"
    );
}

#[test]
fn a_sweep_leaves_a_foreign_file_alone() {
    // A cleanup must never collect something it does not own, however conveniently it is
    // sitting in the same directory.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"ours", "n1");
    let shard = root
        .join(BLOB_TREE_DIR)
        .join("p0")
        .join(name.path_hash().shard(store.shard_width()));
    for foreign in ["README.txt", "notes", "a.b.c"] {
        vfs.write(&shard.join(foreign), b"not ours").unwrap();
    }

    store.retain_blobs(name.path_hash(), &[]).expect("sweep");

    let left = shard_entries(&store, &vfs, &root, 0, &name);
    assert_eq!(left, vec!["README.txt", "a.b.c", "notes"]);
}

#[test]
fn a_sweep_over_an_untouched_document_is_a_no_op() {
    // A shard directory that was never created is a clean sweep, not a failure.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs, root, test_config()).expect("init");

    let never_written = PathHash::of(&node_of("TICKETS/Z"), &doc_of("absent.json"));
    assert_eq!(store.retain_blobs(never_written, &[]).expect("sweep"), 0);
}

#[test]
fn a_sweep_is_idempotent() {
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    ingest(&store, b"one", "n1");
    let survivor = ingest(&store, b"two", "n2");

    assert_eq!(
        store
            .retain_blobs(survivor.path_hash(), &[survivor.digest()])
            .unwrap(),
        3
    );
    assert_eq!(
        store
            .retain_blobs(survivor.path_hash(), &[survivor.digest()])
            .unwrap(),
        0,
        "a second sweep has nothing left to do"
    );
    assert_eq!(
        store
            .read_blob(&reference(&store, &survivor))
            .unwrap()
            .bytes(),
        b"two"
    );
}

#[test]
fn a_sweep_can_keep_more_than_one_version() {
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let a = ingest(&store, b"a", "n1");
    let b = ingest(&store, b"b", "n2");
    let c = ingest(&store, b"c", "n3");

    let removed = store
        .retain_blobs(a.path_hash(), &[a.digest(), c.digest()])
        .expect("sweep");

    assert_eq!(removed, 3, "only b");
    assert_eq!(
        store.read_blob(&reference(&store, &a)).unwrap().bytes(),
        b"a"
    );
    assert_eq!(
        store.read_blob(&reference(&store, &c)).unwrap().bytes(),
        b"c"
    );
    assert!(store.read_blob(&reference(&store, &b)).is_err());
}

#[test]
fn owner_and_version_does_not_require_a_known_artifact() {
    let path_hash = PathHash::of(&node_of("TICKETS/A"), &doc_of("record.json"));
    let digest = ContentDigest::of(b"x");
    let rendered = digest.to_base32();

    for extension in ["BODY", "CHUNKMAP", "SOMETHINGELSE"] {
        let file_name = format!("{}.{rendered}.{extension}", path_hash.render());
        assert_eq!(
            BlobName::owner_and_version(&file_name),
            Some((path_hash.render().as_str(), rendered.as_str()))
        );
    }

    // And still refuses what is not shaped like one of ours.
    assert_eq!(BlobName::owner_and_version("README.txt"), None);
    assert_eq!(BlobName::owner_and_version("a.b.c"), None);
    assert_eq!(
        BlobName::owner_and_version(&format!("{}.{rendered}", path_hash.render())),
        None,
        "a two-field name is not one of ours"
    );
}
