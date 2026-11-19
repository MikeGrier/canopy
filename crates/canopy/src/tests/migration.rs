// Copyright (c) Michael Grier.

//! The genesis format advance: `init` recording the implementation of the root it makes, and the
//! migration of a root written under the previous version, crash-consistent and resumable (canopy's
//! DESIGN-NOTES.md, "A root is migrated to the coordination layer's format").

use super::*;
use crate::store::{LEGACY_MARKER_NAME, LOCK_NAME};
use crate::{GENESIS_FORMAT_VERSION, MIGRATABLE_FORMAT_VERSION, MIGRATION_RECORD_NAME, Migration};
use acadia::coord::file::FileFactory;
use acadia::coord::{CoordinationFactory, CreationNonce};
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{OnceLock, Weak};

mod agents;

/// The coordination layer's record of which implementation owns a root. It is acadia's on-disk
/// format, named here because a migrated or created root is expected to carry it.
const RESERVATION: &str = ".coord-reservation";

fn options() -> StoreOptions {
    StoreOptions::without_nonce_source(test_agent())
}

/// A root as genesis version `version` left it -- the marker under its old name and key, and no
/// implementation record -- holding one document, with no handle open on it. Faults in `policy`
/// are armed once `gate` opens.
fn legacy_root(version: u32, policy: FaultPolicy) -> (Arc<FaultyFs<MemFs>>, Arc<dyn Vfs>, PathBuf) {
    let root = PathBuf::from("migrating");
    let faulty = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    store
        .write_document_replicated_as(name("tickets"), &doc("record"), &body(b"v1"), &nonce("w1"))
        .unwrap();
    drop(store);
    to_legacy(&vfs, &root, version);
    (faulty, vfs, root)
}

/// Rewrites the root of a store made by this build as genesis version `version` wrote it: the marker
/// under its old name and key, and no implementation record.
fn to_legacy(vfs: &Arc<dyn Vfs>, root: &Path, version: u32) {
    let mut marker: serde_json::Value =
        serde_json::from_slice(&vfs.read(&root.join(MARKER_NAME)).unwrap()).unwrap();
    let identity = marker.as_object_mut().unwrap().remove("store_id").unwrap();
    marker.as_object_mut().unwrap().remove("metadata");
    marker["shire_id"] = identity;
    marker["format_version"] = serde_json::Value::from(version);
    let legacy = root.join(LEGACY_MARKER_NAME);
    vfs.write(&legacy, &serde_json::to_vec_pretty(&marker).unwrap())
        .unwrap();
    vfs.fsync(&legacy).unwrap();
    for gone in [MARKER_NAME, RESERVATION, ".coord-creation.lock"] {
        match vfs.remove_file(&root.join(gone)) {
            Ok(()) => {}
            Err(e) if e.kind() == acadia::VfsErrorKind::NotFound => {}
            Err(e) => panic!("{e}"),
        }
    }
    vfs.fsync(root).unwrap();
}

fn legacy_root_v6() -> (Arc<FaultyFs<MemFs>>, Arc<dyn Vfs>, PathBuf) {
    legacy_root(MIGRATABLE_FORMAT_VERSION, FaultPolicy::new())
}

fn json(vfs: &Arc<dyn Vfs>, path: &Path) -> serde_json::Value {
    serde_json::from_slice(&vfs.read(path).unwrap()).unwrap()
}

fn exists(vfs: &Arc<dyn Vfs>, root: &Path, name: &str) -> bool {
    vfs.exists(&root.join(name)).unwrap()
}

/// Asserts the root carries the new names and none of the old.
fn assert_migrated(vfs: &Arc<dyn Vfs>, root: &Path) {
    let marker = json(vfs, &root.join(MARKER_NAME));
    assert_eq!(marker["format_version"], GENESIS_FORMAT_VERSION);
    assert_eq!(marker["store_id"], "test-store");
    assert!(marker.get("shire_id").is_none(), "{marker}");
    assert!(
        !exists(vfs, root, LEGACY_MARKER_NAME),
        "the old marker is gone"
    );
    assert!(
        !exists(vfs, root, MIGRATION_RECORD_NAME),
        "the migration record is gone"
    );
    let reservation = vfs.read(&root.join(RESERVATION)).unwrap();
    let implementation = FileFactory::IMPLEMENTATION;
    assert!(
        reservation
            .windows(implementation.as_str().len())
            .any(|window| window == implementation.as_str().as_bytes()),
        "the implementation record names {implementation}"
    );
}

// -- Init ----------------------------------------------------------------------------------------

#[test]
fn init_records_the_implementation_of_the_root_it_makes() {
    let (vfs, root) = fresh_root();
    drop(Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).unwrap());
    assert_migrated(&vfs, &root);
    assert_eq!(
        json(&vfs, &root.join(MARKER_NAME))["created_at_unix_ms"],
        12_345
    );
}

#[test]
fn init_records_the_implementation_before_it_makes_anything_else() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::CreateDirAll,
        PathMatcher::file_name("p0"),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let root = PathBuf::from("store");
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    vfs.create_dir_all(&root).unwrap();
    gate.open();
    Store::init(Arc::clone(&vfs), root.clone(), test_config(), options())
        .expect_err("the first partition cannot be made");
    assert!(exists(&vfs, &root, RESERVATION), "recorded already");
    assert!(!exists(&vfs, &root, MARKER_NAME), "and no marker yet");
}

#[test]
fn an_interrupted_init_is_finished_by_an_identical_init_and_refused_to_any_other() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::CreateDirAll,
        PathMatcher::file_name("p0"),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let root = PathBuf::from("store");
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    vfs.create_dir_all(&root).unwrap();
    gate.open();
    Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).expect_err("cut short");

    let other = GenesisConfig::new(StoreId::new("another").unwrap(), 1);
    assert!(matches!(
        Store::init(Arc::clone(&vfs), root.clone(), other, options()),
        Err(StoreError::RootReserved { .. })
    ));
    assert!(!exists(&vfs, &root, MARKER_NAME));

    drop(Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).unwrap());
    assert_migrated(&vfs, &root);
}

#[test]
fn init_refuses_a_root_whose_implementation_is_recorded_for_another_creation() {
    let (vfs, root) = fresh_root();
    FileFactory::new()
        .open(Arc::clone(&vfs), &root)
        .unwrap()
        .reserve(b"another creation", CreationNonce::from_bytes([7; 16]))
        .unwrap();
    assert!(matches!(
        Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()),
        Err(StoreError::RootReserved { .. })
    ));
    assert!(!exists(&vfs, &root, MARKER_NAME));
    assert!(!exists(&vfs, &root, "files"), "nothing else was made");
}

#[test]
fn init_refuses_a_root_that_holds_the_previous_marker() {
    let (_faulty, vfs, root) = legacy_root_v6();
    assert!(matches!(
        Store::init(vfs, root, test_config(), options()),
        Err(StoreError::AlreadyInitialized { .. })
    ));
}

#[test]
fn the_options_name_the_implementation_they_coordinate_through() {
    let options = options().with_coordination(Arc::new(FileFactory::new()));
    assert!(format!("{options:?}").contains(FileFactory::IMPLEMENTATION.as_str()));
}

// -- Migration -----------------------------------------------------------------------------------

#[test]
fn a_root_at_the_previous_version_must_be_migrated_before_it_opens() {
    let (_faulty, vfs, root) = legacy_root_v6();
    match Store::open_unsourced(Arc::clone(&vfs), root.clone()) {
        Err(StoreError::MigrationRequired {
            root: reported,
            found,
            expected,
        }) => {
            assert_eq!(reported, root);
            assert_eq!(found, MIGRATABLE_FORMAT_VERSION);
            assert_eq!(expected, GENESIS_FORMAT_VERSION);
        }
        other => panic!("expected MigrationRequired, got {other:?}"),
    }
    assert!(exists(&vfs, &root, LEGACY_MARKER_NAME));
    assert!(!exists(&vfs, &root, MARKER_NAME));
    assert!(!exists(&vfs, &root, MIGRATION_RECORD_NAME));
}

#[test]
fn a_migrated_root_opens_with_its_documents_and_carries_the_new_names_only() {
    let (_faulty, vfs, root) = legacy_root_v6();
    assert_eq!(
        Store::migrate(Arc::clone(&vfs), root.clone(), &options()).unwrap(),
        Migration::Migrated
    );
    assert_migrated(&vfs, &root);
    let marker = json(&vfs, &root.join(MARKER_NAME));
    assert_eq!(marker["created_at_unix_ms"], 12_345);
    assert_eq!(marker["blob_replica_count"], 3);
    assert_eq!(marker["pointer_replica_count"], 3);
    assert_eq!(marker["shard_width"], 2);
    let store = Store::open_unsourced(vfs, root).unwrap();
    store
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert_eq!(store.store_id(), "test-store");
    assert_eq!(
        store.read_body(name("tickets"), &doc("record")).unwrap(),
        Some(body(b"v1"))
    );
}

#[test]
fn a_migration_survives_a_power_loss() {
    let (faulty, vfs, root) = legacy_root_v6();
    Store::migrate(Arc::clone(&vfs), root.clone(), &options()).unwrap();
    let vfs: Arc<dyn Vfs> = Arc::new(faulty.simulate_restart());
    assert_migrated(&vfs, &root);
    let store = Store::open_unsourced(vfs, root).unwrap();
    assert!(store.recovery_required().unwrap());
    store
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
}

#[test]
fn migrating_a_migrated_root_changes_nothing() {
    let (_faulty, vfs, root) = legacy_root_v6();
    Store::migrate(Arc::clone(&vfs), root.clone(), &options()).unwrap();
    let before = vfs.read(&root.join(MARKER_NAME)).unwrap();
    for _ in 0..2 {
        assert_eq!(
            Store::migrate(Arc::clone(&vfs), root.clone(), &options()).unwrap(),
            Migration::AlreadyCurrent
        );
        assert_eq!(vfs.read(&root.join(MARKER_NAME)).unwrap(), before);
    }
}

#[test]
fn a_root_made_by_this_build_is_already_current() {
    let (vfs, root) = fresh_root();
    drop(Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).unwrap());
    assert_eq!(
        Store::migrate(vfs, root, &options()).unwrap(),
        Migration::AlreadyCurrent
    );
}

#[test]
fn a_root_at_any_other_version_is_not_migrated() {
    for version in [1, 4, 5, GENESIS_FORMAT_VERSION + 1] {
        let (_faulty, vfs, root) = legacy_root(version, FaultPolicy::new());
        let before = vfs.read(&root.join(LEGACY_MARKER_NAME)).unwrap();
        match Store::migrate(Arc::clone(&vfs), root.clone(), &options()) {
            Err(StoreError::UnsupportedFormatVersion { found, expected }) => {
                assert_eq!(found, version);
                assert_eq!(expected, GENESIS_FORMAT_VERSION);
            }
            other => panic!("expected UnsupportedFormatVersion for {version}, got {other:?}"),
        }
        assert_eq!(vfs.read(&root.join(LEGACY_MARKER_NAME)).unwrap(), before);
        assert!(!exists(&vfs, &root, MIGRATION_RECORD_NAME), "{version}");
    }
}

#[test]
fn a_marker_under_the_new_name_at_another_version_is_unsupported() {
    let (vfs, root) = fresh_root();
    drop(Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).unwrap());
    let mut marker = json(&vfs, &root.join(MARKER_NAME));
    marker["format_version"] = serde_json::Value::from(GENESIS_FORMAT_VERSION + 1);
    vfs.write(
        &root.join(MARKER_NAME),
        &serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    for outcome in [
        Store::open_unsourced(Arc::clone(&vfs), root.clone()).map(drop),
        Store::migrate(Arc::clone(&vfs), root.clone(), &options()).map(drop),
    ] {
        assert!(matches!(
            outcome,
            Err(StoreError::UnsupportedFormatVersion { .. })
        ));
    }
}

#[test]
fn a_directory_that_is_not_a_store_is_not_migrated() {
    let (vfs, root) = fresh_root();
    assert!(matches!(
        Store::migrate(vfs, root, &options()),
        Err(StoreError::NotAStore { .. })
    ));
}

#[test]
fn a_held_root_is_not_migrated() {
    let (_faulty, vfs, root) = legacy_root_v6();
    let _held = vfs.lock_exclusive(&root.join(LOCK_NAME)).unwrap();
    assert!(matches!(
        Store::migrate(Arc::clone(&vfs), root.clone(), &options()),
        Err(StoreError::RootInUse { .. })
    ));
    assert!(exists(&vfs, &root, LEGACY_MARKER_NAME));
    assert!(!exists(&vfs, &root, MIGRATION_RECORD_NAME));
}

#[test]
fn an_older_binary_finds_no_marker_in_a_migrated_root_and_refuses_it() {
    let (_faulty, vfs, root) = legacy_root_v6();
    Store::migrate(Arc::clone(&vfs), root.clone(), &options()).unwrap();
    // An older binary looks only for the marker it knows, under its old name.
    assert!(!exists(&vfs, &root, LEGACY_MARKER_NAME));
}

/// A step of a migration that fails once, and when.
struct Interruption {
    step: &'static str,
    op: FaultOp,
    matcher: PathMatcher,
    when: When,
    /// Whether the new marker is in the root when the step has failed: the migration commits when
    /// it is published, and no step before it may leave it there.
    marker_published: bool,
}

/// The ways a migration is cut short: the publication of the record, of the implementation record
/// and of the new marker, the removal of the old marker and of the record, and the sync of the root
/// that follows each of those five.
fn interruptions() -> Vec<Interruption> {
    let at = |step, op, matcher, marker_published| Interruption {
        step,
        op,
        matcher,
        when: When::Once,
        marker_published,
    };
    let sync = |step, n, marker_published| Interruption {
        step,
        op: FaultOp::Fsync,
        matcher: PathMatcher::exact("migrating"),
        when: When::OnCall(n),
        marker_published,
    };
    vec![
        at(
            "the migration record",
            FaultOp::Rename,
            PathMatcher::file_name(format!("{MIGRATION_RECORD_NAME}.tmp")),
            false,
        ),
        at(
            "the implementation record",
            FaultOp::PublishNew,
            PathMatcher::file_name(RESERVATION),
            false,
        ),
        at(
            "the new marker",
            FaultOp::Rename,
            PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
            false,
        ),
        at(
            "the old marker's removal",
            FaultOp::RemoveFile,
            PathMatcher::file_name(LEGACY_MARKER_NAME),
            true,
        ),
        at(
            "the record's removal",
            FaultOp::RemoveFile,
            PathMatcher::file_name(MIGRATION_RECORD_NAME),
            true,
        ),
        sync("the sync after the migration record", 1, false),
        sync("the sync after the implementation record", 2, false),
        sync("the sync after the new marker", 3, true),
        sync("the sync after the old marker's removal", 4, true),
        sync("the sync after the record's removal", 5, true),
    ]
}

#[test]
fn a_migration_interrupted_at_any_step_is_not_opened_and_is_finished_by_the_next_run() {
    for cut in interruptions() {
        let step = cut.step;
        for power_loss in [false, true] {
            let gate = FaultGate::closed();
            let policy = FaultPolicy::new().inject_gated(
                cut.op,
                cut.matcher.clone(),
                Fault::io_error(),
                cut.when,
                gate.clone(),
            );
            let (faulty, mut vfs, root) = legacy_root(MIGRATABLE_FORMAT_VERSION, policy);
            gate.open();
            Store::migrate(Arc::clone(&vfs), root.clone(), &options())
                .expect_err(&format!("{step} fails"));
            gate.close();
            if !power_loss {
                assert_eq!(
                    exists(&vfs, &root, MARKER_NAME),
                    cut.marker_published,
                    "{step}: the new marker is published last"
                );
            }
            if power_loss {
                vfs = Arc::new(faulty.simulate_restart());
            }

            match Store::open_unsourced(Arc::clone(&vfs), root.clone()) {
                Err(StoreError::MigrationRequired { .. }) => {}
                Ok(store) => {
                    drop(store);
                    assert!(
                        !exists(&vfs, &root, MIGRATION_RECORD_NAME)
                            && !exists(&vfs, &root, LEGACY_MARKER_NAME),
                        "{step}: a root opens only once its migration is complete"
                    );
                }
                Err(other) => panic!("{step}: {other:?}"),
            }

            let outcome = Store::migrate(Arc::clone(&vfs), root.clone(), &options())
                .unwrap_or_else(|e| panic!("{step}, power loss {power_loss}: {e}"));
            assert!(
                matches!(outcome, Migration::Migrated | Migration::AlreadyCurrent),
                "{step}"
            );
            assert_migrated(&vfs, &root);
            let store = Store::open_unsourced(vfs, root).unwrap();
            store
                .recover()
                .unwrap()
                .agent_stamp
                .expect("agent version stamp");
            assert_eq!(
                store.read_body(name("tickets"), &doc("record")).unwrap(),
                Some(body(b"v1")),
                "{step}"
            );
        }
    }
}

#[test]
fn a_migration_cut_short_before_its_marker_leaves_the_old_marker_for_an_older_binary() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Rename,
        PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let (_faulty, vfs, root) = legacy_root(MIGRATABLE_FORMAT_VERSION, policy);
    gate.open();
    Store::migrate(Arc::clone(&vfs), root.clone(), &options()).expect_err("cut short");
    assert!(exists(&vfs, &root, LEGACY_MARKER_NAME), "uncommitted");
    assert!(!exists(&vfs, &root, MARKER_NAME));
    assert!(exists(&vfs, &root, MIGRATION_RECORD_NAME));
    assert!(exists(&vfs, &root, RESERVATION));
}

#[test]
fn a_migration_is_resumed_only_for_the_implementation_it_was_begun_for() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Rename,
        PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let (_faulty, vfs, root) = legacy_root(MIGRATABLE_FORMAT_VERSION, policy);
    gate.open();
    Store::migrate(Arc::clone(&vfs), root.clone(), &options()).expect_err("cut short");

    let record_path = root.join(MIGRATION_RECORD_NAME);
    let mut record = json(&vfs, &record_path);
    record["implementation"] = serde_json::Value::from("someone-elses-1");
    vfs.write(&record_path, &serde_json::to_vec_pretty(&record).unwrap())
        .unwrap();
    match Store::migrate(Arc::clone(&vfs), root.clone(), &options()) {
        Err(StoreError::MigrationInputsDiffer {
            recorded,
            requested,
            ..
        }) => {
            assert_eq!(recorded, "someone-elses-1");
            assert_eq!(requested, FileFactory::IMPLEMENTATION.as_str());
        }
        other => panic!("expected MigrationInputsDiffer, got {other:?}"),
    }
    assert!(!exists(&vfs, &root, MARKER_NAME), "nothing was changed");
    assert!(exists(&vfs, &root, LEGACY_MARKER_NAME));
}

#[test]
fn a_root_another_creation_reserved_is_not_migrated() {
    let (_faulty, vfs, root) = legacy_root_v6();
    FileFactory::new()
        .open(Arc::clone(&vfs), &root)
        .unwrap()
        .reserve(b"another creation", CreationNonce::from_bytes([7; 16]))
        .unwrap();
    for _ in 0..2 {
        assert!(matches!(
            Store::migrate(Arc::clone(&vfs), root.clone(), &options()),
            Err(StoreError::RootReserved { .. })
        ));
    }
    assert!(!exists(&vfs, &root, MARKER_NAME), "never published");
    assert!(matches!(
        Store::open_unsourced(vfs, root),
        Err(StoreError::MigrationRequired { .. })
    ));
}

#[test]
fn a_migration_record_this_build_did_not_write_is_refused() {
    let (_faulty, vfs, root) = legacy_root_v6();
    vfs.write(&root.join(MIGRATION_RECORD_NAME), b"not a record")
        .unwrap();
    for outcome in [
        Store::open_unsourced(Arc::clone(&vfs), root.clone()).map(drop),
        Store::migrate(Arc::clone(&vfs), root.clone(), &options()).map(drop),
    ] {
        assert!(matches!(outcome, Err(StoreError::MalformedMarker { .. })));
    }
}

/// A filesystem that runs `during` while the root's lock is being taken, once armed, as another
/// process would between a first look at the root and the lock.
fn during_the_lock(
    during: impl Fn(&Arc<dyn Vfs>) + Send + Sync + 'static,
) -> (Arc<FaultyFs<MemFs>>, Arc<AtomicUsize>) {
    during_the_operation(
        FaultOp::LockExclusive,
        PathMatcher::file_name(LOCK_NAME),
        during,
    )
}

fn during_the_operation(
    operation: FaultOp,
    path: PathMatcher,
    during: impl Fn(&Arc<dyn Vfs>) + Send + Sync + 'static,
) -> (Arc<FaultyFs<MemFs>>, Arc<AtomicUsize>) {
    let armed = Arc::new(AtomicUsize::new(0));
    let handle: Arc<OnceLock<Weak<FaultyFs<MemFs>>>> = Arc::default();
    let (fire, reach) = (Arc::clone(&armed), Arc::clone(&handle));
    let policy = FaultPolicy::new().inject_matching(
        operation,
        path,
        Fault::observe_with(move |_, _| {
            if fire.fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            }) == Ok(1)
                && let Some(fs) = reach.get().and_then(Weak::upgrade)
            {
                let vfs: Arc<dyn Vfs> = fs;
                during(&vfs);
            }
        }),
        When::Always,
    );
    let faulty = Arc::new(FaultyFs::new(MemFs::new(), policy));
    handle.set(Arc::downgrade(&faulty)).unwrap();
    (faulty, armed)
}

#[test]
fn a_migration_reads_the_root_again_under_the_lock() {
    let root = PathBuf::from("raced");
    let at = root.clone();
    let (faulty, armed) = during_the_lock(move |vfs| {
        assert_eq!(
            Store::migrate(Arc::clone(vfs), at.clone(), &options()).unwrap(),
            Migration::Migrated,
            "another process migrates the root while this one waits for the lock"
        );
    });
    let vfs: Arc<dyn Vfs> = faulty;
    vfs.create_dir_all(&root).unwrap();
    drop(Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap());
    to_legacy(&vfs, &root, MIGRATABLE_FORMAT_VERSION);
    armed.store(1, Ordering::Release);
    assert_eq!(
        Store::migrate(Arc::clone(&vfs), root.clone(), &options()).unwrap(),
        Migration::AlreadyCurrent,
        "the migration found under the lock is the other's"
    );
    assert_migrated(&vfs, &root);
}

#[test]
fn an_open_reads_the_marker_again_under_the_attachment() {
    let root = PathBuf::from("raced");
    let at = root.clone();
    let (faulty, armed) = during_the_operation(
        FaultOp::Read,
        PathMatcher::file_name(MARKER_NAME),
        move |vfs| {
            let marker = at.join(MARKER_NAME);
            let mut genesis = json(vfs, &marker);
            genesis["format_version"] = serde_json::Value::from(GENESIS_FORMAT_VERSION + 1);
            vfs.write(&marker, &serde_json::to_vec_pretty(&genesis).unwrap())
                .unwrap();
        },
    );
    let vfs: Arc<dyn Vfs> = faulty;
    vfs.create_dir_all(&root).unwrap();
    drop(Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap());
    armed.store(2, Ordering::Release);
    assert!(matches!(
        Store::open_unsourced(vfs, root),
        Err(StoreError::UnsupportedFormatVersion { .. })
    ));
}

#[test]
fn two_migrations_at_once_migrate_the_root_once() {
    use std::sync::Barrier;
    let (_faulty, vfs, root) = legacy_root_v6();
    let start = Arc::new(Barrier::new(2));
    let runs: Vec<_> = (0..2)
        .map(|_| {
            let (vfs, root, start) = (Arc::clone(&vfs), root.clone(), Arc::clone(&start));
            std::thread::spawn(move || {
                start.wait();
                Store::migrate(vfs, root, &options())
            })
        })
        .collect();
    let outcomes: Vec<_> = runs.into_iter().map(|run| run.join().unwrap()).collect();
    for outcome in &outcomes {
        assert!(
            matches!(
                outcome,
                Ok(Migration::Migrated | Migration::AlreadyCurrent)
                    | Err(StoreError::RootInUse { .. })
            ),
            "{outcome:?}"
        );
    }
    assert!(
        outcomes
            .iter()
            .any(|outcome| matches!(outcome, Ok(Migration::Migrated))),
        "{outcomes:?}"
    );
    assert_migrated(&vfs, &root);
}
