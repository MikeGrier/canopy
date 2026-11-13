// Copyright (c) Michael Grier.

//! The publication record that guards an unlogged write against `STRANDED-VOTE`, and the
//! genesis migration that admits a root to it.

use super::*;
use crate::store::LOCK_NAME;
use crate::{
    GENESIS_FORMAT_VERSION, MIGRATABLE_FORMAT_VERSION, Migration, PUBLICATION_RECORD_NAME, Swap,
};
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{OnceLock, Weak};

/// A store over a fault-injecting in-memory filesystem, with three pointer replicas so a write
/// can land on some of them and not the rest.
fn faulty_store(label: &str, policy: FaultPolicy) -> (Arc<FaultyFs<MemFs>>, Arc<dyn Vfs>, Store) {
    let root = PathBuf::from(label);
    let faulty = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    let config = test_config().with_pointer_replica_count(3).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root, config).unwrap();
    (faulty, vfs, store)
}

/// The rename that moves pointer replica `partition` into place for the write under `nonce`.
fn replica_rename(partition: &'static str, nonce: &'static str) -> PathMatcher {
    let nonce = nonce.to_ascii_uppercase();
    PathMatcher::predicate(move |path: &Path| {
        let parts: Vec<String> = path
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_ascii_uppercase())
            .collect();
        let has = |wanted: &str| parts.iter().any(|part| part == wanted);
        has(&NAMESPACE_TREE_DIR.to_ascii_uppercase())
            && has(&partition.to_ascii_uppercase())
            && parts.iter().any(|part| part.contains(&nonce))
    })
}

fn record_path(store: &Store) -> PathBuf {
    store.root().join(PUBLICATION_RECORD_NAME)
}

fn record_text(vfs: &Arc<dyn Vfs>, store: &Store) -> String {
    String::from_utf8(vfs.read(&record_path(store)).unwrap()).unwrap()
}

/// Asserts every pointer replica of (`node`, `document`) names `expected`.
fn assert_settled_on(store: &Store, node: &NodePath, document: &DocumentName, expected: &[u8]) {
    assert_eq!(
        store.read_body(node, document).unwrap(),
        Some(body(expected))
    );
    assert!(
        store.read_pointer(node, document).unwrap().is_unanimous(),
        "a replica still disagrees"
    );
}

/// The write of `v2` under `w2` renames replicas 0 and 1 and fails at replica 2, leaving `v2`
/// the majority and its record pending.
fn write_v2_short_of_replica_2(store: &Store, node: &NodePath, document: &DocumentName) {
    store
        .update_document_as(Swap::new(node, document.clone(), body(b"v1")), &nonce("w1"))
        .unwrap();
    store
        .write_document_replicated_as(node, document, &body(b"v2"), &nonce("w2"))
        .expect_err("replica 2's rename fails");
    assert_eq!(store.read_body(node, document).unwrap(), Some(body(b"v2")));
    assert!(!store.read_pointer(node, document).unwrap().is_unanimous());
}

fn v2_fails_at_replica_2() -> FaultPolicy {
    FaultPolicy::new().inject_matching(
        FaultOp::Rename,
        replica_rename("p2", "w2"),
        Fault::io_error(),
        When::Once,
    )
}

// -- STRANDED-VOTE ----------------------------------------------------------------------------

#[test]
fn two_failed_writes_in_a_row_leave_the_document_a_majority() {
    // Unguarded, `v3` landing on replica 0 alone leaves v3, v2, v1 across the three replicas:
    // no majority, so a document every reader saw as `v2` stops being readable. The record
    // makes `v3`'s write finish `v2` first.
    let policy = v2_fails_at_replica_2().inject_matching(
        FaultOp::Rename,
        replica_rename("p1", "w3"),
        Fault::io_error(),
        When::Once,
    );
    let (_faulty, _vfs, store) = faulty_store("stranded", policy);
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);

    store
        .write_document_replicated_as(&node, &d, &body(b"v3"), &nonce("w3"))
        .expect_err("replica 1's rename fails");

    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));
    store
        .update_document_as(Swap::new(&node, doc("other"), body(b"x")), &nonce("t1"))
        .unwrap();
    assert_settled_on(&store, &node, &d, b"v3");
}

#[test]
fn a_failed_write_is_finished_by_the_next_open() {
    let (_faulty, vfs, store) = faulty_store("warm-reopen", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);

    let reopened = reopen(&vfs, store).unwrap();
    assert_settled_on(&reopened, &node, &d, b"v2");
    assert_eq!(record_text(&vfs, &reopened), "");
}

#[test]
fn a_failed_write_is_finished_after_a_power_loss() {
    // The record was durable before the first rename, so it survives whatever the renames'
    // parent syncs did or did not make durable.
    let (faulty, vfs, store) = faulty_store("power-loss", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    let root = store.root().to_path_buf();
    write_v2_short_of_replica_2(&store, &node, &d);
    drop(store);
    faulty.inner().simulate_crash();

    let reopened = Store::open_unsourced(vfs, root).unwrap();
    assert_settled_on(&reopened, &node, &d, b"v2");
}

#[test]
fn a_write_that_failed_over_a_lost_replica_reads_as_an_error_until_the_next_settle() {
    // T26: replica 2 has lost its entry, and a write renames replica 0 and fails at replica 1.
    // One new, one old and one missing is no majority, and nothing is modifying it, so a read
    // reports it rather than calling it contention; the next write settles the record.
    let policy = FaultPolicy::new().inject_matching(
        FaultOp::Rename,
        replica_rename("p1", "w2"),
        Fault::io_error(),
        When::Once,
    );
    let (_faulty, vfs, store) = faulty_store("lost-replica", policy);
    let (node, d) = (name("tickets"), doc("record"));
    store
        .write_document_replicated_as(&node, &d, &body(b"v1"), &nonce("w1"))
        .unwrap();
    vfs.remove_file(&namespace_root(&store, 2).join("TICKETS").join("RECORD"))
        .unwrap();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("replica 1's rename fails");

    assert!(matches!(
        store.read_document(&node, &d),
        Err(StoreError::PointerUnavailable {
            missing: 1,
            rejected: 0,
            disagreeing: 2,
            ..
        })
    ));
    store
        .write_document_replicated_as(&node, &doc("other"), &body(b"x"), &nonce("w3"))
        .unwrap();
    assert_settled_on(&store, &node, &d, b"v2");
}

#[test]
fn a_completed_write_leaves_the_record_empty() {
    let (_faulty, vfs, store) = faulty_store("completed", FaultPolicy::new());
    let (node, d) = (name("tickets"), doc("record"));
    store
        .write_document_replicated_as(&node, &d, &body(b"v1"), &nonce("w1"))
        .unwrap();
    assert_eq!(record_text(&vfs, &store), "");
    assert_settled_on(&store, &node, &d, b"v1");
}

#[test]
fn the_record_name_is_synced_once_per_open() {
    let root_syncs = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&root_syncs);
    let policy = FaultPolicy::new().inject_matching(
        FaultOp::Fsync,
        PathMatcher::exact("once-per-open"),
        Fault::observe_with(move |_, _| {
            counted.fetch_add(1, Ordering::AcqRel);
        }),
        When::Always,
    );
    let (_faulty, _vfs, store) = faulty_store("once-per-open", policy);
    let (node, d) = (name("tickets"), doc("record"));
    store
        .write_document_replicated_as(&node, &d, &body(b"v1"), &nonce("w1"))
        .unwrap();
    let after_first = root_syncs.load(Ordering::Acquire);
    for (round, tag) in [(2u8, "w2"), (3, "w3"), (4, "w4")] {
        store
            .write_document_replicated_as(&node, &d, &body(&[round]), &nonce(tag))
            .unwrap();
    }
    assert_eq!(root_syncs.load(Ordering::Acquire), after_first);
}

// -- The record's own durability (T24, T25) -----------------------------------------------------

/// A store whose writes so far went through the log, so no record file exists yet, and whose
/// record fsync fails on the `call`th attempt once `gate` opens.
fn record_fsync_fails(
    label: &str,
    call: u32,
) -> (Arc<FaultyFs<MemFs>>, Arc<dyn Vfs>, Store, FaultGate) {
    let gate = FaultGate::closed();
    let record = PathBuf::from(label).join(PUBLICATION_RECORD_NAME);
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Fsync,
        PathMatcher::exact(record),
        Fault::io_error(),
        When::OnCall(call),
        gate.clone(),
    );
    let (faulty, vfs, store) = faulty_store(label, policy);
    store
        .update_document_as(
            Swap::new(name("tickets"), doc("record"), body(b"v1")),
            &nonce("w1"),
        )
        .unwrap();
    (faulty, vfs, store, gate)
}

#[test]
fn a_record_that_did_not_reach_the_disk_moves_no_replica() {
    let (faulty, vfs, store, gate) = record_fsync_fails("unrecorded", 1);
    let (node, d) = (name("tickets"), doc("record"));
    let root = store.root().to_path_buf();
    gate.open();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("the record's fsync fails");
    assert_settled_on(&store, &node, &d, b"v1");
    drop(store);
    faulty.inner().simulate_crash();

    let reopened = Store::open_unsourced(vfs, root).unwrap();
    assert_settled_on(&reopened, &node, &d, b"v1");
}

#[test]
fn a_record_left_unsynced_is_made_durable_and_finished_by_the_next_operation() {
    // T24: the line reached the file but not the disk. The next settle syncs it before acting
    // on it, so it can never roll replicas forward on a record a crash could still take away.
    let (_faulty, vfs, store, gate) = record_fsync_fails("unsynced", 1);
    let (node, d) = (name("tickets"), doc("record"));
    gate.open();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("the record's fsync fails");

    store
        .update_document_as(Swap::new(&node, doc("other"), body(b"x")), &nonce("t1"))
        .unwrap();
    assert_settled_on(&store, &node, &d, b"v2");
    assert_eq!(record_text(&vfs, &store), "");
}

#[test]
fn a_settle_interrupted_by_a_power_loss_is_finished_by_the_reopen() {
    // T24 continued: the settle's roll-forward moves replica 0 and fails at replica 1. Had it
    // acted on the record without syncing it first, the crash would take the record and leave
    // replica 0 ahead of the others with nothing to finish it.
    let gate = FaultGate::closed();
    let record = PathBuf::from("interrupted-settle").join(PUBLICATION_RECORD_NAME);
    let policy = FaultPolicy::new()
        .inject_gated(
            FaultOp::Fsync,
            PathMatcher::exact(record),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        )
        .inject_matching(
            FaultOp::Rename,
            replica_rename("p1", "w2"),
            Fault::io_error(),
            When::Once,
        );
    let (faulty, vfs, store) = faulty_store("interrupted-settle", policy);
    let (node, d) = (name("tickets"), doc("record"));
    let root = store.root().to_path_buf();
    store
        .update_document_as(Swap::new(&node, d.clone(), body(b"v1")), &nonce("w1"))
        .unwrap();
    gate.open();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("the record's fsync fails");
    store
        .update_document_as(Swap::new(&node, doc("other"), body(b"x")), &nonce("t1"))
        .expect_err("the roll-forward fails at replica 1");
    drop(store);
    faulty.inner().simulate_crash();

    let reopened = Store::open_unsourced(vfs, root).unwrap();
    assert_settled_on(&reopened, &node, &d, b"v2");
}

#[test]
fn a_clear_that_did_not_reach_the_disk_cannot_roll_a_later_write_back() {
    // T25: the clear's truncation is live but not durable. A later transaction moves the
    // document on; unless its settle syncs the empty file first, a crash restores the record
    // and the reopen rolls the document back to the version the record names.
    let (faulty, vfs, store, gate) = record_fsync_fails("unsynced-clear", 2);
    let (node, d) = (name("tickets"), doc("record"));
    let root = store.root().to_path_buf();
    gate.open();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("the clear's fsync fails");
    assert_settled_on(&store, &node, &d, b"v2");

    store
        .update_document_as(Swap::new(&node, d.clone(), body(b"v3")), &nonce("t3"))
        .unwrap();
    drop(store);
    faulty.inner().simulate_crash();

    let reopened = Store::open_unsourced(vfs, root).unwrap();
    assert_settled_on(&reopened, &node, &d, b"v3");
}

#[test]
fn a_settle_that_fails_fails_its_operation_and_leaves_the_record_pending() {
    let gate = FaultGate::closed();
    let policy = v2_fails_at_replica_2().inject_gated(
        FaultOp::Fsync,
        PathMatcher::exact(PathBuf::from("failed-settle").join(PUBLICATION_RECORD_NAME)),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let (_faulty, vfs, store) = faulty_store("failed-settle", policy);
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);

    gate.open();
    store
        .write_document_replicated_as(&node, &d, &body(b"v3"), &nonce("w3"))
        .expect_err("the settle's fsync fails");
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));
    assert!(record_text(&vfs, &store).contains(nonce("w2").as_str()));

    store
        .update_document_as(Swap::new(&node, doc("other"), body(b"x")), &nonce("t1"))
        .unwrap();
    assert_settled_on(&store, &node, &d, b"v2");
}

// -- What a settle reads ------------------------------------------------------------------------

#[test]
fn an_unterminated_line_is_an_append_that_never_completed() {
    let (vfs, store) = fresh_store();
    vfs.append(&record_path(&store), br#"{"node":"tick"#)
        .unwrap();
    let (node, d) = (name("tickets"), doc("record"));
    store
        .update_document_as(Swap::new(&node, d.clone(), body(b"v1")), &nonce("t1"))
        .unwrap();
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v1")));
    assert_eq!(record_text(&vfs, &store), "");
}

#[test]
fn a_complete_line_is_finished_whatever_follows_it() {
    let (_faulty, vfs, store) = faulty_store("torn-tail", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);
    vfs.append(&record_path(&store), br#"{"node":"#).unwrap();

    let reopened = reopen(&vfs, store).unwrap();
    assert_settled_on(&reopened, &node, &d, b"v2");
    assert_eq!(record_text(&vfs, &reopened), "");
}

#[test]
fn a_line_this_store_did_not_write_is_refused() {
    for line in [
        &b"not a record\n"[..],
        br#"{"node":"tickets","document":"record","digest":"zz","nonce":"w9"}
"#,
        br#"{"node":"tickets","document":"","digest":"00","nonce":"w9"}
"#,
    ] {
        let (vfs, store) = fresh_store();
        vfs.append(&record_path(&store), line).unwrap();
        let refused = store
            .write_document_replicated_as(
                name("tickets"),
                &doc("record"),
                &body(b"v1"),
                &nonce("w1"),
            )
            .expect_err("the record cannot be settled");
        assert!(
            matches!(refused, StoreError::MalformedPublicationRecord { .. }),
            "got {refused:?}"
        );
        let root = store.root().to_path_buf();
        drop(store);
        let refused = Store::open_unsourced(Arc::clone(&vfs), root).expect_err("nor opened");
        assert!(matches!(
            refused,
            StoreError::MalformedPublicationRecord { .. }
        ));
    }
}

// -- Maintenance settles first -----------------------------------------------------------------

#[test]
fn a_pointer_repair_settles_first() {
    // A repair never rewrites a stale replica (`D320`), so replica 2 coming level shows the
    // settle, not the repair, moved it.
    let (_faulty, _vfs, store) = faulty_store("repair-settles", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);
    let report = store.read_pointer(&node, &d).unwrap();
    assert_eq!(report.stale(), [2]);

    store
        .repair_pointer_as(&node, &d, &report, &nonce("r1"))
        .unwrap();
    assert_settled_on(&store, &node, &d, b"v2");
}

#[test]
fn a_scrub_settles_first() {
    let (_faulty, _vfs, store) = faulty_store("scrub-settles", v2_fails_at_replica_2());
    store.install(StoreOptions::with_nonce_source(counting_nonce_source()));
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);

    let (report, _) = store.scrub_content().unwrap();
    assert!(report.stale().is_empty(), "{:?}", report.stale());
    assert_settled_on(&store, &node, &d, b"v2");
}

#[test]
fn a_rejected_transaction_still_settles() {
    let (_faulty, _vfs, store) = faulty_store("check-settles", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);

    let outcome = store
        .update_document_as(
            Swap::conditional_absent(&node, d.clone(), body(b"x")),
            &nonce("t1"),
        )
        .unwrap();
    assert!(outcome.is_rejected());
    assert_settled_on(&store, &node, &d, b"v2");
}

// -- Migration -----------------------------------------------------------------------------------

/// A store at `version`, holding one document, with no handle open on it.
fn root_at_version(version: u32) -> (Arc<FaultyFs<MemFs>>, Arc<dyn Vfs>, PathBuf) {
    let (faulty, vfs, store) = faulty_store("migrating", FaultPolicy::new());
    store
        .write_document_replicated_as(name("tickets"), &doc("record"), &body(b"v1"), &nonce("w1"))
        .unwrap();
    let root = store.root().to_path_buf();
    drop(store);
    set_marker_version(&vfs, &root, version);
    (faulty, vfs, root)
}

fn set_marker_version(vfs: &Arc<dyn Vfs>, root: &Path, version: u32) {
    let marker = root.join(MARKER_NAME);
    let mut genesis: serde_json::Value =
        serde_json::from_slice(&vfs.read(&marker).unwrap()).unwrap();
    genesis["format_version"] = serde_json::Value::from(version);
    vfs.write(&marker, &serde_json::to_vec_pretty(&genesis).unwrap())
        .unwrap();
    vfs.fsync(&marker).unwrap();
}

fn marker_version(vfs: &Arc<dyn Vfs>, root: &Path) -> u64 {
    let genesis: serde_json::Value =
        serde_json::from_slice(&vfs.read(&root.join(MARKER_NAME)).unwrap()).unwrap();
    genesis["format_version"].as_u64().unwrap()
}

#[test]
fn a_root_at_the_migratable_version_must_be_migrated_before_it_opens() {
    let (_faulty, vfs, root) = root_at_version(MIGRATABLE_FORMAT_VERSION);
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
    assert_eq!(
        marker_version(&vfs, &root),
        u64::from(MIGRATABLE_FORMAT_VERSION)
    );
}

#[test]
fn a_migrated_root_opens_with_its_documents() {
    let (_faulty, vfs, root) = root_at_version(MIGRATABLE_FORMAT_VERSION);
    assert_eq!(
        Store::migrate(Arc::clone(&vfs), root.clone()).unwrap(),
        Migration::Migrated
    );
    assert_eq!(
        marker_version(&vfs, &root),
        u64::from(GENESIS_FORMAT_VERSION)
    );
    let store = Store::open_unsourced(vfs, root).unwrap();
    assert_settled_on(&store, &name("tickets"), &doc("record"), b"v1");
}

#[test]
fn a_migration_survives_a_power_loss() {
    let (faulty, vfs, root) = root_at_version(MIGRATABLE_FORMAT_VERSION);
    Store::migrate(Arc::clone(&vfs), root.clone()).unwrap();
    faulty.inner().simulate_crash();
    assert_eq!(
        marker_version(&vfs, &root),
        u64::from(GENESIS_FORMAT_VERSION)
    );
    Store::open_unsourced(vfs, root).unwrap();
}

#[test]
fn migrating_a_current_root_changes_nothing() {
    let (_faulty, vfs, root) = root_at_version(GENESIS_FORMAT_VERSION);
    let before = vfs.read(&root.join(MARKER_NAME)).unwrap();
    assert_eq!(
        Store::migrate(Arc::clone(&vfs), root.clone()).unwrap(),
        Migration::AlreadyCurrent
    );
    assert_eq!(vfs.read(&root.join(MARKER_NAME)).unwrap(), before);
    assert_eq!(
        Store::migrate(vfs, root).unwrap(),
        Migration::AlreadyCurrent,
        "and again"
    );
}

#[test]
fn a_root_at_any_other_version_is_not_migrated() {
    for version in [1, 4, GENESIS_FORMAT_VERSION + 1] {
        let (_faulty, vfs, root) = root_at_version(version);
        match Store::migrate(Arc::clone(&vfs), root.clone()) {
            Err(StoreError::UnsupportedFormatVersion { found, expected }) => {
                assert_eq!(found, version);
                assert_eq!(expected, GENESIS_FORMAT_VERSION);
            }
            other => panic!("expected UnsupportedFormatVersion for {version}, got {other:?}"),
        }
        assert_eq!(marker_version(&vfs, &root), u64::from(version));
    }
}

#[test]
fn a_directory_that_is_not_a_store_is_not_migrated() {
    let (vfs, root) = fresh_root();
    assert!(matches!(
        Store::migrate(vfs, root),
        Err(StoreError::NotAShire { .. })
    ));
}

#[test]
fn a_held_root_is_not_migrated() {
    let (_faulty, vfs, root) = root_at_version(MIGRATABLE_FORMAT_VERSION);
    let _held = vfs.lock_exclusive(&root.join(LOCK_NAME)).unwrap();
    assert!(matches!(
        Store::migrate(Arc::clone(&vfs), root.clone()),
        Err(StoreError::RootInUse { .. })
    ));
    assert_eq!(
        marker_version(&vfs, &root),
        u64::from(MIGRATABLE_FORMAT_VERSION)
    );
}

#[test]
fn an_interrupted_migration_leaves_the_old_version_and_can_be_run_again() {
    let gate = FaultGate::closed();
    let tmp = PathBuf::from("migrating").join(format!("{MARKER_NAME}.tmp"));
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Rename,
        PathMatcher::exact(tmp),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let root = PathBuf::from("migrating");
    let faulty = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    drop(Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap());
    set_marker_version(&vfs, &root, MIGRATABLE_FORMAT_VERSION);

    gate.open();
    Store::migrate(Arc::clone(&vfs), root.clone()).expect_err("the marker's rename fails");
    faulty.inner().simulate_crash();
    assert_eq!(
        marker_version(&vfs, &root),
        u64::from(MIGRATABLE_FORMAT_VERSION)
    );

    assert_eq!(
        Store::migrate(Arc::clone(&vfs), root.clone()).unwrap(),
        Migration::Migrated
    );
    Store::open_unsourced(vfs, root).unwrap();
}

/// A filesystem that rewrites the marker to `version` while the root's lock is being taken,
/// as a concurrent migration would between a reader's first look and its lock.
fn marker_rewritten_under_the_lock(version: u32) -> (Arc<dyn Vfs>, PathBuf, Arc<AtomicUsize>) {
    let root = PathBuf::from("raced");
    let armed = Arc::new(AtomicUsize::new(0));
    let handle: Arc<OnceLock<Weak<FaultyFs<MemFs>>>> = Arc::default();
    let (fire, reach, at) = (Arc::clone(&armed), Arc::clone(&handle), root.clone());
    let policy = FaultPolicy::new().inject_matching(
        FaultOp::LockExclusive,
        PathMatcher::file_name(LOCK_NAME),
        Fault::observe_with(move |_, _| {
            if fire.swap(0, Ordering::AcqRel) == 1
                && let Some(fs) = reach.get().and_then(Weak::upgrade)
            {
                let vfs: Arc<dyn Vfs> = fs;
                set_marker_version(&vfs, &at, version);
            }
        }),
        When::Always,
    );
    let faulty = Arc::new(FaultyFs::new(MemFs::new(), policy));
    handle.set(Arc::downgrade(&faulty)).unwrap();
    let vfs: Arc<dyn Vfs> = faulty;
    vfs.create_dir_all(&root).unwrap();
    drop(Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap());
    (vfs, root, armed)
}

#[test]
fn a_migration_reads_the_marker_again_under_the_lock() {
    let (vfs, root, armed) = marker_rewritten_under_the_lock(GENESIS_FORMAT_VERSION);
    set_marker_version(&vfs, &root, MIGRATABLE_FORMAT_VERSION);
    armed.store(1, Ordering::Release);
    assert_eq!(
        Store::migrate(vfs, root).unwrap(),
        Migration::AlreadyCurrent,
        "another migration finished between the first read and the lock"
    );
}

#[test]
fn an_open_reads_the_marker_again_under_the_lock() {
    let (vfs, root, armed) = marker_rewritten_under_the_lock(GENESIS_FORMAT_VERSION + 1);
    armed.store(1, Ordering::Release);
    assert!(matches!(
        Store::open_unsourced(vfs, root),
        Err(StoreError::UnsupportedFormatVersion { .. })
    ));
}
