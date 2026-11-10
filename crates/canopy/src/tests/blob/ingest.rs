// Copyright (c) Michael Grier.
// Split from blob.rs at 844ebb37.

//! `CVS-6`: blob ingest -- the caller's stream in, our digest out.

use super::*;
use std::io::{self, Read};

fn node_of(path: &str) -> NodePath {
    NodePath::parse(path).expect("valid node path")
}

fn doc_of(name: &str) -> DocumentName {
    DocumentName::new(name).expect("valid document name")
}

fn nonce_of(token: &str) -> Nonce {
    Nonce::new(token).expect("valid nonce")
}

fn ingest(store: &Store, body: &[u8], token: &str) -> BlobName {
    store
        .ingest_blob(
            &node_of("TICKETS/A"),
            &doc_of("record.json"),
            Artifact::Body,
            body,
            &nonce_of(token),
        )
        .expect("ingest")
}

fn blob_path(root: &std::path::Path, index: usize, name: &BlobName, width: usize) -> PathBuf {
    root.join(BLOB_TREE_DIR)
        .join(format!("p{index}"))
        .join(name.relative_path(width))
}

// -- the digest is ours -------------------------------------------------------

#[test]
fn ingest_returns_the_digest_it_computed_over_what_it_wrote() {
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let body = b"the body bytes";
    let name = ingest(&store, body, "n1");

    assert_eq!(name.digest(), ContentDigest::of(body));
    assert_eq!(
        name.path_hash(),
        PathHash::of(&node_of("TICKETS/A"), &doc_of("record.json"))
    );
    assert_eq!(name.artifact(), Artifact::Body);
}

#[test]
fn the_stored_bytes_hash_to_the_name_they_were_stored_under() {
    // The property that makes a blob self-validating (`D315`): the name is checkable
    // against the content without reference to anything else.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"self-validating", "n1");
    let width = store.shard_width();
    for index in 0..store.blob_replica_count() {
        let bytes = vfs.read(&blob_path(&root, index, &name, width)).unwrap();
        assert_eq!(
            ContentDigest::of(&bytes),
            name.digest(),
            "partition {index} does not hash to its own name"
        );
    }
}

#[test]
fn a_source_read_in_many_chunks_hashes_the_same_as_one_read() {
    // The single pass is over the caller's stream, not over a buffer canopy assembled --
    // so how the stream chooses to deliver its bytes must not change the digest.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let dribbled = ingest(&store, &body, "n1");

    struct Dribble<'a> {
        bytes: &'a [u8],
        at: usize,
    }
    impl Read for Dribble<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.at >= self.bytes.len() {
                return Ok(0);
            }
            // A short read that is not the buffer size, repeatedly: exactly what a socket
            // or a pipe does, and what a reader that assumed one `read` would miss.
            let take = out.len().min(7).min(self.bytes.len() - self.at);
            out[..take].copy_from_slice(&self.bytes[self.at..self.at + take]);
            self.at += take;
            Ok(take)
        }
    }

    let by_chunks = store
        .ingest_blob(
            &node_of("TICKETS/B"),
            &doc_of("record.json"),
            Artifact::Body,
            Dribble {
                bytes: &body,
                at: 0,
            },
            &nonce_of("n2"),
        )
        .expect("ingest");

    assert_eq!(by_chunks.digest(), dribbled.digest());
    assert_eq!(by_chunks.digest(), ContentDigest::of(&body));
}

#[test]
fn an_empty_body_ingests_to_the_empty_digest() {
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"", "n1");
    assert_eq!(name.digest(), ContentDigest::of(b""));
    let width = store.shard_width();
    assert!(
        vfs.read(&blob_path(&root, 0, &name, width))
            .unwrap()
            .is_empty()
    );
}

// -- replication ---------------------------------------------------------------

#[test]
fn a_blob_spanning_many_chunks_is_replicated_byte_for_byte() {
    let (vfs, root) = fresh_root();
    let config = test_config().with_blob_replica_count(3).unwrap();
    let store = Store::init(vfs.clone(), root.clone(), config).expect("init");

    // Several whole chunks and a ragged remainder, so every copy boundary is exercised.
    let body: Vec<u8> = (0..(3 * 64 * 1024 + 17) as u32)
        .map(|i| (i % 253) as u8)
        .collect();
    let name = ingest(&store, &body, "n1");

    let width = store.shard_width();
    for index in 0..3 {
        assert_eq!(
            vfs.read(&blob_path(&root, index, &name, width)).unwrap(),
            body,
            "partition {index}"
        );
    }
}

#[test]
fn replicating_a_staged_blob_never_reads_it_whole() {
    use acadia::{Fault, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, VfsErrorKind, When};

    // A whole-file read of a staging temp is refused outright, so the copy must go by
    // bounded positioned reads to succeed at all.
    let staging_temp = PathMatcher::predicate(|path| {
        path.components()
            .any(|c| c.as_os_str() == crate::store::STAGING_DIR)
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("blob.tmp."))
    });
    let policy = FaultPolicy::new().inject_matching(
        FaultOp::Read,
        staging_temp,
        Fault::Fail(VfsErrorKind::Other),
        When::Always,
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let config = test_config().with_blob_replica_count(2).unwrap();
    let store = Store::init(vfs.clone(), root.clone(), config).expect("init");

    let body: Vec<u8> = (0..(2 * 64 * 1024 + 5) as u32).map(|i| i as u8).collect();
    let name = ingest(&store, &body, "n1");
    let width = store.shard_width();
    assert_eq!(vfs.read(&blob_path(&root, 1, &name, width)).unwrap(), body);
}

#[test]
fn every_replica_is_present_and_byte_identical_before_ingest_returns() {
    // `D315`: a blob is durable before any pointer names it. There is no window in which
    // a name could be published while a copy is still missing.
    let (vfs, root) = fresh_root();
    let config = test_config().with_blob_replica_count(4).unwrap();
    let store = Store::init(vfs.clone(), root.clone(), config).expect("init");

    let body = b"replicated";
    let name = ingest(&store, body, "n1");

    assert_eq!(store.blob_replica_count(), 4);
    let width = store.shard_width();
    for index in 0..4 {
        assert_eq!(
            vfs.read(&blob_path(&root, index, &name, width)).unwrap(),
            body,
            "partition {index}"
        );
    }
}

#[test]
fn an_even_replica_count_is_honoured() {
    // Blobs are verified rather than voted on, so parity means nothing here (`D316`).
    let (vfs, root) = fresh_root();
    let config = test_config().with_blob_replica_count(2).unwrap();
    let store = Store::init(vfs.clone(), root.clone(), config).expect("init");

    let name = ingest(&store, b"two copies", "n1");
    let width = store.shard_width();
    for index in 0..2 {
        assert!(vfs.metadata(&blob_path(&root, index, &name, width)).is_ok());
    }
}

// -- layout --------------------------------------------------------------------

#[test]
fn a_blob_lands_under_its_shard_and_the_shard_is_a_prefix_of_the_name() {
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"sharded", "n1");
    let width = store.shard_width();
    let shard = name.path_hash().shard(width);

    let expected = root
        .join(BLOB_TREE_DIR)
        .join("p0")
        .join(&shard)
        .join(name.file_name());
    assert!(vfs.metadata(&expected).unwrap().is_file());
    assert!(
        name.file_name().starts_with(&shard),
        "a human must be able to read the shard off the filename (`D321`)"
    );
}

#[test]
fn init_creates_the_blob_partitions_but_no_shard_directories() {
    // Shards are created lazily, so an empty store carries 1024 absent directories
    // rather than 1024 empty ones (`D322`).
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    for index in 0..store.blob_replica_count() {
        let partition = root.join(BLOB_TREE_DIR).join(format!("p{index}"));
        assert!(vfs.metadata(&partition).unwrap().is_dir());
        assert!(
            vfs.read_dir(&partition).unwrap().is_empty(),
            "partition {index} should have no shards yet"
        );
    }
}

// -- idempotence and supersession ----------------------------------------------

#[test]
fn identical_content_at_the_same_path_lands_on_the_same_name() {
    // Why `D315`'s cleanup is a set difference: a rewrite of identical content is the
    // same blob, so "delete the old one" would delete the new one.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let first = ingest(&store, b"same", "n1");
    let second = ingest(&store, b"same", "n2");

    assert_eq!(first.file_name(), second.file_name());
    let width = store.shard_width();
    assert_eq!(
        vfs.read(&blob_path(&root, 0, &first, width)).unwrap(),
        b"same"
    );
}

#[test]
fn a_new_version_does_not_disturb_the_old_blob() {
    // Blobs are immutable (`D314`): a second version is a second file, and removing the
    // first is a separate act scheduled as `CVS-8`.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let old = ingest(&store, b"old", "n1");
    let new = ingest(&store, b"new", "n2");
    assert_ne!(old.file_name(), new.file_name());

    let width = store.shard_width();
    assert_eq!(vfs.read(&blob_path(&root, 0, &old, width)).unwrap(), b"old");
    assert_eq!(vfs.read(&blob_path(&root, 0, &new, width)).unwrap(), b"new");
}

#[test]
fn different_documents_with_identical_content_get_different_blobs() {
    // `D315`: blobs are never shared, and the owner hash is what enforces it rather than
    // merely intending it.
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let a = store
        .ingest_blob(
            &node_of("TICKETS/A"),
            &doc_of("record.json"),
            Artifact::Body,
            &b"identical"[..],
            &nonce_of("n1"),
        )
        .unwrap();
    let b = store
        .ingest_blob(
            &node_of("TICKETS/B"),
            &doc_of("record.json"),
            Artifact::Body,
            &b"identical"[..],
            &nonce_of("n2"),
        )
        .unwrap();

    assert_eq!(a.digest(), b.digest(), "the content really is identical");
    assert_ne!(
        a.file_name(),
        b.file_name(),
        "and yet the two documents own different files"
    );
    let width = store.shard_width();
    assert_ne!(
        blob_path(&root, 0, &a, width),
        blob_path(&root, 0, &b, width)
    );
}

// -- staging and failure --------------------------------------------------------

#[test]
fn a_successful_ingest_leaves_no_staging_behind() {
    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");
    ingest(&store, b"clean up", "n1");

    for index in 0..store.blob_replica_count() {
        let staging = root
            .join(BLOB_TREE_DIR)
            .join(format!("p{index}"))
            .join(".staging");
        let leftovers = vfs.read_dir(&staging).unwrap_or_default();
        assert!(leftovers.is_empty(), "partition {index}: {leftovers:?}");
    }
}

#[test]
fn a_failing_source_is_reported_as_the_callers_failure_and_commits_nothing() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("the caller's pipe broke"))
        }
    }

    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let error = store
        .ingest_blob(
            &node_of("TICKETS/A"),
            &doc_of("record.json"),
            Artifact::Body,
            Broken,
            &nonce_of("n1"),
        )
        .expect_err("a broken source fails the ingest");

    // Not a `Vfs` error: the store's storage is fine, the bytes stopped arriving.
    assert!(
        matches!(error, StoreError::IngestSource { .. }),
        "expected an ingest-source failure, got {error:?}"
    );

    let partition = root.join(BLOB_TREE_DIR).join("p0");
    let entries = vfs.read_dir(&partition).unwrap();
    assert!(
        entries.iter().all(|e| e.file_name == ".staging"),
        "nothing was committed: {entries:?}"
    );
}

#[test]
fn a_reused_nonce_after_a_failure_does_not_concatenate_two_bodies() {
    // The staging temp is truncated before the stream is appended, so a survivor of an
    // earlier attempt cannot silently prefix the next one into a body that hashes to
    // neither.
    struct HalfThenBroken(bool);
    impl Read for HalfThenBroken {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                return Err(io::Error::other("stopped"));
            }
            self.0 = true;
            out[..5].copy_from_slice(b"first");
            Ok(5)
        }
    }

    let (vfs, root) = fresh_root();
    let store = Store::init(vfs.clone(), root.clone(), test_config()).expect("init");

    let _ = store.ingest_blob(
        &node_of("TICKETS/A"),
        &doc_of("record.json"),
        Artifact::Body,
        HalfThenBroken(false),
        &nonce_of("n1"),
    );

    let name = ingest(&store, b"second", "n1");
    assert_eq!(name.digest(), ContentDigest::of(b"second"));
    let width = store.shard_width();
    assert_eq!(
        vfs.read(&blob_path(&root, 0, &name, width)).unwrap(),
        b"second"
    );
}
