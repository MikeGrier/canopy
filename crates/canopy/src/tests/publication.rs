// Copyright (c) Michael Grier.

//! The publication record that guards an unlogged write against `STRANDED-VOTE`.

use super::*;
use crate::{PUBLICATION_RECORD_NAME, Swap};
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A store over a fault-injecting in-memory filesystem, with three pointer replicas so a write
/// can land on some of them and not the rest.
pub(super) fn faulty_store(
    label: &str,
    policy: FaultPolicy,
) -> (Arc<FaultyFs<MemFs>>, Arc<dyn Vfs>, Store) {
    let root = PathBuf::from(label);
    let faulty = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    vfs.create_dir_all(&root).unwrap();
    let config = test_config().with_pointer_replica_count(3).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root, config).unwrap();
    (faulty, vfs, store)
}

/// The rename that moves pointer replica `partition` into place for the write under `nonce`.
pub(super) fn replica_rename(partition: &'static str, nonce: &'static str) -> PathMatcher {
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
/// the majority, its record pending and recovery required.
pub(super) fn write_v2_short_of_replica_2(store: &Store, node: &NodePath, document: &DocumentName) {
    store
        .update_document_as(Swap::new(node, document.clone(), body(b"v1")), &nonce("w1"))
        .unwrap();
    store
        .write_document_replicated_as(node, document, &body(b"v2"), &nonce("w2"))
        .expect_err("replica 2's rename fails");
    assert_eq!(store.read_body(node, document).unwrap(), Some(body(b"v2")));
    assert!(!store.read_pointer(node, document).unwrap().is_unanimous());
    assert!(
        store.recovery_required().unwrap(),
        "a write that fails after its record fails unresolved"
    );
}

/// Completes the coordination state's recovery without touching a document, as a recovery that
/// leaves the documents would: whatever the record names stays pending, with no fence. A failed
/// write cannot leave that state, so it is how a test reaches a settle that has work to do.
pub(super) fn leave_the_record_pending(vfs: &Arc<dyn Vfs>, store: &Store) {
    use acadia::coord::file::FileFactory;
    use acadia::coord::{CoordinationFactory, Waiting};
    let participant = FileFactory::new()
        .open(Arc::clone(vfs), store.root())
        .expect("a participant");
    let mut hold = participant.acquire(Waiting::Wait).expect("exclusion");
    hold.complete_recovery().expect("the contract's recovery");
    hold.release().expect("a clean release");
    assert!(!store.recovery_required().unwrap());
}

pub(super) fn v2_fails_at_replica_2() -> FaultPolicy {
    FaultPolicy::new().inject_matching(
        FaultOp::Rename,
        replica_rename("p2", "w2"),
        Fault::io_error(),
        When::Once,
    )
}

// -- STRANDED-VOTE ----------------------------------------------------------------------------

#[test]
fn a_write_after_a_failed_one_waits_for_recovery_to_finish_it() {
    // Unguarded, `v3` landing on replica 0 alone would leave v3, v2, v1 across the three
    // replicas: no majority, so a document every reader saw as `v2` stops being readable. The
    // failed write leaves recovery required, and recovery finishes `v2` before `v3` may begin.
    let (_faulty, _vfs, store) = faulty_store("stranded", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);

    let refused = store
        .write_document_replicated_as(&node, &d, &body(b"v3"), &nonce("w3"))
        .expect_err("recovery is required");
    assert!(
        matches!(refused, StoreError::RecoveryRequired { .. }),
        "{refused:?}"
    );
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));

    store
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert_settled_on(&store, &node, &d, b"v2");
    store
        .write_document_replicated_as(&node, &d, &body(b"v3"), &nonce("w3"))
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
    let (faulty, _vfs, store) = faulty_store("power-loss", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    let root = store.root().to_path_buf();
    write_v2_short_of_replica_2(&store, &node, &d);
    drop(store);
    let vfs: Arc<dyn Vfs> = Arc::new(faulty.simulate_restart());

    let reopened = open_resolved(vfs, root).unwrap();
    assert_settled_on(&reopened, &node, &d, b"v2");
}

#[test]
fn a_write_that_failed_over_a_lost_replica_is_contended_until_recovery_finishes_it() {
    // T26: replica 2 has lost its entry, and a write renames replica 0 and fails at replica 1.
    // One new, one old and one missing is no majority. The write failed unresolved, so its
    // operation is still in progress as a reader sees the count, and recovery settles the record.
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
    vfs.remove_file(&namespace_root(&store, 2).join("tickets").join("record"))
        .unwrap();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("replica 1's rename fails");

    let read = store.read_document(&node, &d);
    assert!(matches!(read, Ok(crate::Read::Contended(_))), "{read:?}");
    store
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
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
    let (faulty, _vfs, store, gate) = record_fsync_fails("unrecorded", 1);
    let (node, d) = (name("tickets"), doc("record"));
    let root = store.root().to_path_buf();
    gate.open();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("the record's fsync fails");
    assert_settled_on(&store, &node, &d, b"v1");
    drop(store);
    let vfs: Arc<dyn Vfs> = Arc::new(faulty.simulate_restart());

    let reopened = open_resolved(vfs, root).unwrap();
    assert_settled_on(&reopened, &node, &d, b"v1");
}

#[test]
fn a_record_left_unsynced_is_made_durable_and_finished_by_recovery() {
    // T24: the line reached the file but not the disk. Recovery's settle syncs it before acting
    // on it, so it can never roll replicas forward on a record a crash could still take away.
    let (_faulty, vfs, store, gate) = record_fsync_fails("unsynced", 1);
    let (node, d) = (name("tickets"), doc("record"));
    gate.open();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("the record's fsync fails");
    assert!(store.recovery_required().unwrap());

    store
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert_settled_on(&store, &node, &d, b"v2");
    assert_eq!(record_text(&vfs, &store), "");
}

#[test]
fn a_settle_interrupted_by_a_power_loss_is_finished_by_the_reopen() {
    // T24 continued: recovery's roll-forward moves replica 0 and fails at replica 1. Had it
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
    let (faulty, _vfs, store) = faulty_store("interrupted-settle", policy);
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
        .recover()
        .expect_err("the roll-forward fails at replica 1");
    drop(store);
    let vfs: Arc<dyn Vfs> = Arc::new(faulty.simulate_restart());

    let reopened = open_resolved(vfs, root).unwrap();
    assert_settled_on(&reopened, &node, &d, b"v2");
}

#[test]
fn a_clear_that_did_not_reach_the_disk_cannot_roll_a_later_write_back() {
    // T25: the clear's truncation is live but not durable, and the write fails unresolved.
    // Recovery and a later transaction move on; unless a settle syncs the empty file, a crash
    // restores the record and the reopen rolls the document back to the version it names.
    let (faulty, _vfs, store, gate) = record_fsync_fails("unsynced-clear", 2);
    let (node, d) = (name("tickets"), doc("record"));
    let root = store.root().to_path_buf();
    gate.open();
    store
        .write_document_replicated_as(&node, &d, &body(b"v2"), &nonce("w2"))
        .expect_err("the clear's fsync fails");
    assert_settled_on(&store, &node, &d, b"v2");

    store
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    store
        .update_document_as(Swap::new(&node, d.clone(), body(b"v3")), &nonce("t3"))
        .unwrap();
    drop(store);
    let vfs: Arc<dyn Vfs> = Arc::new(faulty.simulate_restart());

    let reopened = open_resolved(vfs, root).unwrap();
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
    store.recover().expect_err("the settle's fsync fails");
    assert_eq!(store.read_body(&node, &d).unwrap(), Some(body(b"v2")));
    assert!(record_text(&vfs, &store).contains(nonce("w2").as_str()));
    assert!(store.recovery_required().unwrap());

    store
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
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
        let refused = open_resolved(Arc::clone(&vfs), root).expect_err("nor opened");
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
    let (_faulty, vfs, store) = faulty_store("repair-settles", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);
    leave_the_record_pending(&vfs, &store);
    let report = store.read_pointer(&node, &d).unwrap();
    assert_eq!(report.stale(), [2]);

    store
        .repair_pointer_as(&node, &d, &report, &nonce("r1"))
        .unwrap();
    assert_settled_on(&store, &node, &d, b"v2");
}

#[test]
fn a_scrub_settles_first() {
    let (_faulty, vfs, store) = faulty_store("scrub-settles", v2_fails_at_replica_2());
    store.install(StoreOptions::with_nonce_source(
        test_agent(),
        counting_nonce_source(),
    ));
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);
    leave_the_record_pending(&vfs, &store);

    let (report, _) = store.scrub_content().unwrap().value;
    assert!(report.stale().is_empty(), "{:?}", report.stale());
    assert_settled_on(&store, &node, &d, b"v2");
}

#[test]
fn a_rejected_transaction_still_settles() {
    let (_faulty, vfs, store) = faulty_store("check-settles", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);
    leave_the_record_pending(&vfs, &store);

    let outcome = store
        .update_document_as(
            Swap::conditional_absent(&node, d.clone(), body(b"x")),
            &nonce("t1"),
        )
        .unwrap();
    assert!(outcome.is_rejected());
    assert_settled_on(&store, &node, &d, b"v2");
}
