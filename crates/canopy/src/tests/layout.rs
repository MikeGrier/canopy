// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! LAY-1: store layout -- genesis `init` and `open` over the marker and the replica trees.

use super::*;
use crate::store::{BLOB_TREE_DIR, NAMESPACE_TREE_DIR};
use crate::{
    DEFAULT_BLOB_REPLICA_COUNT, DEFAULT_POINTER_REPLICA_COUNT, DEFAULT_SHARD_WIDTH,
    MAX_REPLICA_COUNT, MAX_SHARD_WIDTH,
};
use acadia::{
    Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, VfsError, VfsErrorKind, When,
};
use std::error::Error as _;

// -- Store layout: init / open (LAY-1) -----------------------------------------

#[test]
fn init_creates_the_marker_and_the_replica_trees() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");

    assert!(vfs.exists(&root.join(MARKER_NAME)).unwrap());
    for i in 0..DEFAULT_POINTER_REPLICA_COUNT {
        assert!(vfs.metadata(&namespace_root(&store, i)).unwrap().is_dir());
    }
    // No replica beyond the recorded count, in either tree.
    assert!(
        !vfs.exists(&namespace_root(&store, DEFAULT_POINTER_REPLICA_COUNT))
            .unwrap()
    );
    for i in 0..DEFAULT_BLOB_REPLICA_COUNT {
        let partition = root.join(BLOB_TREE_DIR).join(format!("p{i}"));
        assert!(vfs.metadata(&partition).unwrap().is_dir());
    }
    assert!(
        !vfs.exists(
            &root
                .join(BLOB_TREE_DIR)
                .join(format!("p{DEFAULT_BLOB_REPLICA_COUNT}"))
        )
        .unwrap()
    );
}

#[test]
fn init_records_the_configured_parameters() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    assert_eq!(store.shire_id(), "test-shire");
    assert_eq!(store.created_at_unix_ms(), 12_345);
}

#[test]
fn genesis_config_accessors_return_the_configured_values() {
    let config = GenesisConfig::new(ShireId::new("configured").unwrap(), 98_765)
        .with_blob_replica_count(4)
        .unwrap()
        .with_pointer_replica_count(5)
        .unwrap();

    assert_eq!(config.blob_replica_count(), 4);
    assert_eq!(config.pointer_replica_count(), 5);
    assert_eq!(config.created_at_unix_ms(), 98_765);
    assert_eq!(config.shire_id().to_string(), "configured");
}

#[test]
fn store_debug_reports_its_identity_and_layout() {
    let (_vfs, store) = fresh_store();
    let debug = format!("{store:?}");

    for expected in [
        "Store",
        "root",
        "shire_id",
        "test-shire",
        "created_at_unix_ms",
    ] {
        assert!(
            debug.contains(expected),
            "missing {expected:?} from {debug}"
        );
    }
}

#[test]
fn store_errors_report_context_and_preserve_nested_sources() {
    let plain = StoreError::EmptyShireId;
    assert_eq!(plain.to_string(), "a Shire identity must not be empty");
    assert!(plain.source().is_none());

    let vfs = StoreError::Vfs(VfsError::new(
        VfsErrorKind::Other,
        "read",
        "injected failure",
    ));
    assert!(vfs.to_string().starts_with("store filesystem error:"));
    assert!(vfs.source().is_some());

    let json =
        StoreError::Json(serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err());
    assert!(
        json.to_string()
            .starts_with("store genesis marker is not valid JSON:")
    );
    assert!(json.source().is_some());

    let commit_log = StoreError::CommitLog(CommitLogError::Malformed("bad record".to_owned()));
    assert!(
        commit_log
            .to_string()
            .starts_with("store commit-log error:")
    );
    assert!(commit_log.source().is_some());
}

#[test]
fn a_blob_replica_count_is_a_durability_choice_with_no_parity_rule() {
    // `D316`: a blob is named by its digest and validates alone, so there is no tie to
    // break and nothing for an odd count to buy.
    assert!(test_config().with_blob_replica_count(2).is_ok());
    assert!(test_config().with_blob_replica_count(1).is_ok());
    assert!(matches!(
        test_config().with_blob_replica_count(0),
        Err(StoreError::InvalidBlobReplicaCount { count: 0 })
    ));
    assert!(matches!(
        test_config().with_blob_replica_count(MAX_REPLICA_COUNT + 1),
        Err(StoreError::InvalidBlobReplicaCount { .. })
    ));
}

#[test]
fn a_pointer_replica_count_must_be_odd_because_pointer_reads_vote() {
    // `D320`: verification cannot separate a stale pointer from a current one, so the
    // namespace is the one place a majority has to exist.
    assert!(matches!(
        test_config().with_pointer_replica_count(2),
        Err(StoreError::InvalidPointerReplicaCount { count: 2 })
    ));
    assert!(matches!(
        test_config().with_pointer_replica_count(0),
        Err(StoreError::InvalidPointerReplicaCount { count: 0 })
    ));
    assert!(test_config().with_pointer_replica_count(1).is_ok());
    assert!(test_config().with_pointer_replica_count(5).is_ok());
}

#[test]
fn a_shard_width_is_a_character_count_within_the_supported_range() {
    for width in 1..=MAX_SHARD_WIDTH {
        assert!(
            test_config().with_shard_width(width).is_ok(),
            "width {width} is within range"
        );
    }
    assert!(matches!(
        test_config().with_shard_width(0),
        Err(StoreError::InvalidShardWidth { width: 0 })
    ));
    assert!(matches!(
        test_config().with_shard_width(MAX_SHARD_WIDTH + 1),
        Err(StoreError::InvalidShardWidth { .. })
    ));
}

#[test]
fn init_records_the_layout_parameters_and_open_reads_them_back() {
    let (vfs, root) = fresh_root();
    let config = test_config()
        .with_blob_replica_count(4)
        .unwrap()
        .with_pointer_replica_count(5)
        .unwrap()
        .with_shard_width(3)
        .unwrap();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), config).expect("init");
    assert_eq!(store.blob_replica_count(), 4);
    assert_eq!(store.pointer_replica_count(), 5);
    assert_eq!(store.shard_width(), 3);
    drop(store);

    // The marker is the record, not the handle that wrote it.
    let reopened = Store::open_unsourced(vfs, root).expect("open");
    assert_eq!(reopened.blob_replica_count(), 4);
    assert_eq!(reopened.pointer_replica_count(), 5);
    assert_eq!(reopened.shard_width(), 3);
}

#[test]
fn the_layout_parameters_default_without_being_asked_for() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    assert_eq!(store.blob_replica_count(), DEFAULT_BLOB_REPLICA_COUNT);
    assert_eq!(store.pointer_replica_count(), DEFAULT_POINTER_REPLICA_COUNT);
    assert_eq!(store.shard_width(), DEFAULT_SHARD_WIDTH);
}

#[test]
fn init_refuses_a_root_that_is_already_a_shire() {
    let (vfs, root) = fresh_root();
    Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("first init");
    let err = Store::init_unsourced(vfs, root, test_config()).expect_err("second init");
    assert!(matches!(err, StoreError::AlreadyInitialized { .. }));
}

#[test]
fn init_refuses_a_root_holding_a_non_dotfile_entry() {
    let (vfs, root) = fresh_root();
    vfs.write(&root.join("junk"), b"x").unwrap();
    let err = Store::init_unsourced(vfs, root, test_config()).expect_err("not empty");
    assert!(matches!(err, StoreError::NotEmpty { .. }));
}

#[test]
fn init_tolerates_ignorable_dotfiles_in_the_root() {
    let (vfs, root) = fresh_root();
    vfs.write(&root.join(".hidden"), b"x").unwrap();
    Store::init_unsourced(vfs, root, test_config()).expect("dotfiles are ignorable");
}

#[test]
fn init_requires_the_root_to_exist() {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let err = Store::init_unsourced(vfs, "nope", test_config()).expect_err("absent root");
    assert!(matches!(err, StoreError::RootNotFound { .. }));
}

#[test]
fn init_refuses_a_root_path_that_is_a_file() {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    vfs.write(PathBuf::from("shire").as_path(), b"not a directory")
        .unwrap();

    let err = Store::init_unsourced(vfs, "shire", test_config()).expect_err("root is a file");
    assert!(matches!(err, StoreError::RootNotADirectory { .. }));
}

#[test]
fn init_propagates_root_metadata_errors() {
    let root = PathBuf::from("shire");
    let policy =
        FaultPolicy::new().inject(FaultOp::Metadata, &root, Fault::io_error(), When::Always);
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));

    match Store::init_unsourced(vfs, root, test_config()) {
        Err(StoreError::Vfs(error)) => assert_eq!(error.kind(), VfsErrorKind::Other),
        other => panic!("expected Vfs(Other), got {other:?}"),
    }
}

#[test]
fn open_reads_back_the_genesis_parameters() {
    let (vfs, root) = fresh_root();
    Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    let store = Store::open_unsourced(vfs, root.clone()).expect("open");
    assert_eq!(store.root(), root);
    assert_eq!(store.pointer_replica_count(), DEFAULT_POINTER_REPLICA_COUNT);
    assert_eq!(store.shire_id(), "test-shire");
    assert_eq!(store.created_at_unix_ms(), 12_345);
}

#[test]
fn open_refuses_a_directory_that_is_not_a_shire() {
    let (vfs, root) = fresh_root();
    let err = Store::open_unsourced(vfs, root).expect_err("no marker");
    assert!(matches!(err, StoreError::NotAShire { .. }));
}

#[test]
fn open_propagates_marker_read_errors() {
    let gate = FaultGate::closed();
    let root = PathBuf::from("shire");
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Read,
        PathMatcher::exact(root.join(MARKER_NAME)),
        Fault::io_error(),
        When::Always,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).unwrap();
    drop(store);
    gate.open();

    match Store::open_unsourced(vfs, root) {
        Err(StoreError::Vfs(error)) => assert_eq!(error.kind(), VfsErrorKind::Other),
        other => panic!("expected Vfs(Other), got {other:?}"),
    }
}

#[test]
fn open_refuses_a_marker_that_is_not_valid_json() {
    let (vfs, root) = fresh_root();
    vfs.write(&root.join(MARKER_NAME), b"not json").unwrap();
    let err = Store::open_unsourced(vfs, root).expect_err("bad json");
    assert!(matches!(err, StoreError::Json(_)));
}

#[test]
fn open_refuses_an_unknown_genesis_version() {
    let (vfs, root) = fresh_root();
    let marker =
        br#"{"format_version":999,"partition_count":3,"shire_id":"x","created_at_unix_ms":0}"#;
    vfs.write(&root.join(MARKER_NAME), marker).unwrap();
    // Note the marker carries none of v3's fields. A future version is refused on its
    // version, never on its field set (`D325`) -- this one would not deserialize.
    let err = Store::open_unsourced(vfs, root).expect_err("unknown version");
    assert!(matches!(
        err,
        StoreError::UnsupportedFormatVersion { found: 999, .. }
    ));
}

#[test]
fn open_refuses_a_pre_hierarchy_store() {
    // `D222` bumped the genesis version because a v1 store's directory layout carries
    // a synthesized shard tier this build would misread as an ordinary path component
    // -- every node would appear one level deeper than it is. Refusing is the whole
    // reason the version moved, so it is pinned rather than left to the generic
    // version check.
    let (vfs, root) = fresh_root();
    let marker =
        br#"{"format_version":1,"partition_count":3,"shard_depth":2,"shire_id":"x","created_at_unix_ms":0}"#;
    vfs.write(&root.join(MARKER_NAME), marker).unwrap();
    let err = Store::open_unsourced(vfs, root).expect_err("a v1 store is not readable");
    match err {
        StoreError::UnsupportedFormatVersion { found, expected } => {
            assert_eq!(found, 1);
            assert_eq!(expected, crate::GENESIS_FORMAT_VERSION);
        }
        other => panic!("expected an unsupported-version refusal, got {other:?}"),
    }
}

/// The `CVS-4` mirror of the v1 refusal above. `D318` chose refusal over migration, so the
/// version bump *is* the whole compatibility story and is pinned the same way.
///
/// A v2 marker carries none of the layout fields this build's marker requires, which is why
/// this test is not redundant with the generic unknown-version case: deserializing the marker
/// as this build's shape before checking the version would report it as malformed JSON, naming
/// a missing field rather than the version (`D325`).
#[test]
fn open_refuses_a_v2_store_on_its_version_rather_than_its_missing_fields() {
    let (vfs, root) = fresh_root();
    let marker =
        br#"{"format_version":2,"partition_count":3,"shire_id":"x","created_at_unix_ms":0}"#;
    vfs.write(&root.join(MARKER_NAME), marker).unwrap();
    let err = Store::open_unsourced(vfs, root).expect_err("a v2 store is not readable");
    match err {
        StoreError::UnsupportedFormatVersion { found, expected } => {
            assert_eq!(found, 2);
            assert_eq!(expected, crate::GENESIS_FORMAT_VERSION);
        }
        other => panic!("expected an unsupported-version refusal, got {other:?}"),
    }
}

/// The other side of the v2 case: a v4 marker is this build's marker plus the retired
/// `partition_count` field. An extra field is not what decides the refusal -- the version is
/// (`D325`) -- and a v4 store's partition tree is not something this build can read.
#[test]
fn open_refuses_a_v4_store_on_its_version_rather_than_its_retired_field() {
    let (vfs, root) = fresh_root();
    let marker = br#"{"format_version":4,"partition_count":3,"blob_replica_count":3,
         "pointer_replica_count":3,"shard_width":2,"shire_id":"x","created_at_unix_ms":0}"#;
    vfs.write(&root.join(MARKER_NAME), marker).unwrap();
    let err = Store::open_unsourced(vfs, root).expect_err("a v4 store is not readable");
    match err {
        StoreError::UnsupportedFormatVersion { found, expected } => {
            assert_eq!(found, 4);
            assert_eq!(expected, crate::GENESIS_FORMAT_VERSION);
        }
        other => panic!("expected an unsupported-version refusal, got {other:?}"),
    }
}

#[test]
fn open_refuses_a_marker_with_an_even_pointer_replica_count() {
    // The namespace votes (`D320`), so an even count has no majority to take.
    let (vfs, root) = fresh_root();
    let marker = br#"{"format_version":5,"blob_replica_count":3,
         "pointer_replica_count":2,"shard_width":2,"shire_id":"x","created_at_unix_ms":0}"#;
    vfs.write(&root.join(MARKER_NAME), marker).unwrap();
    let err = Store::open_unsourced(vfs, root).expect_err("even pointer replica count");
    assert!(matches!(err, StoreError::MalformedMarker { .. }));
}

#[test]
fn open_accepts_a_marker_with_an_even_blob_replica_count() {
    // The counterpart to the test above, and the point of keeping the two counts apart:
    // blobs are verified rather than voted on, so parity means nothing to them (`D316`).
    let (vfs, root) = fresh_root();
    let marker = br#"{"format_version":5,"blob_replica_count":2,
         "pointer_replica_count":3,"shard_width":2,"shire_id":"x","created_at_unix_ms":0}"#;
    vfs.write(&root.join(MARKER_NAME), marker).unwrap();
    for index in 0..3 {
        vfs.create_dir_all(&root.join(NAMESPACE_TREE_DIR).join(format!("p{index}")))
            .unwrap();
    }
    let store = Store::open_unsourced(vfs, root).expect("an even blob replica count is valid");
    assert_eq!(store.blob_replica_count(), 2);
}

#[test]
fn open_refuses_a_marker_with_a_shard_width_outside_the_supported_range() {
    let (vfs, root) = fresh_root();
    let marker = format!(
        "{{\"format_version\":5,\"blob_replica_count\":3,\
         \"pointer_replica_count\":3,\"shard_width\":{},\
         \"shire_id\":\"x\",\"created_at_unix_ms\":0}}",
        crate::MAX_SHARD_WIDTH + 1
    );
    vfs.write(&root.join(MARKER_NAME), marker.as_bytes())
        .unwrap();
    let err = Store::open_unsourced(vfs, root).expect_err("shard width past the ceiling");
    assert!(matches!(err, StoreError::MalformedMarker { .. }));
}

#[test]
fn open_reports_a_missing_namespace_replica() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    let replica = namespace_root(&store, 1);
    drop(store);
    vfs.remove_dir_all(&replica).unwrap();
    let err = Store::open_unsourced(vfs, root).expect_err("missing namespace replica");
    assert!(matches!(err, StoreError::MissingPartition { index: 1, .. }));
}

#[test]
fn open_refuses_a_namespace_replica_path_that_is_a_file() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).unwrap();
    let replica = namespace_root(&store, 1);
    drop(store);
    vfs.remove_dir_all(&replica).unwrap();
    vfs.write(&replica, b"not a directory").unwrap();

    let err = Store::open_unsourced(vfs, root).expect_err("namespace replica is a file");
    assert!(matches!(err, StoreError::MalformedMarker { .. }));
}

#[test]
fn open_propagates_namespace_replica_metadata_errors() {
    let gate = FaultGate::closed();
    let root = PathBuf::from("shire");
    let replica = root.join(NAMESPACE_TREE_DIR).join("p1");
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Metadata,
        PathMatcher::exact(replica),
        Fault::io_error(),
        When::Always,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).unwrap();
    drop(store);
    gate.open();

    match Store::open_unsourced(vfs, root) {
        Err(StoreError::Vfs(error)) => assert_eq!(error.kind(), VfsErrorKind::Other),
        other => panic!("expected Vfs(Other), got {other:?}"),
    }
}

#[test]
fn open_accepts_the_maximum_supported_replica_count() {
    // `D79`'s ceiling is inclusive, so the largest recorded count a marker may carry still
    // opens rather than being read as the malformed one past it.
    let (vfs, root) = fresh_root();
    let marker = format!(
        "{{\"format_version\":5,\
         \"blob_replica_count\":{MAX_REPLICA_COUNT},\"pointer_replica_count\":3,\"shard_width\":2,\
         \"shire_id\":\"max-shire\",\"created_at_unix_ms\":1}}"
    );
    vfs.write(&root.join(MARKER_NAME), marker.as_bytes())
        .unwrap();
    for index in 0..3 {
        vfs.create_dir_all(&root.join(NAMESPACE_TREE_DIR).join(format!("p{index}")))
            .unwrap();
    }

    let store = Store::open_unsourced(vfs, root).expect("the inclusive maximum is valid");
    assert_eq!(store.blob_replica_count(), MAX_REPLICA_COUNT);
}

#[test]
fn shire_id_rejects_the_empty_string() {
    assert!(matches!(ShireId::new(""), Err(StoreError::EmptyShireId)));
    assert_eq!(ShireId::new("x").unwrap().as_str(), "x");
}
