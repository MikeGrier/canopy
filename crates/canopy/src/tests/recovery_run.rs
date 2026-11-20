// Copyright (c) Michael Grier.

//! Recovery is a run the caller drives: opening changes no store data, discovery reports abandoned
//! work and never a live writer's, and a recovery is a sequence of bounded, idempotent steps that
//! any stop or death leaves resumable (canopy's DESIGN-NOTES.md, "Recovery is a run the caller
//! drives").

use super::*;
use crate::{RecoveryNeed, RecoveryPhase, RecoveryProgress, StoreOptions};
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
use std::sync::{Mutex, OnceLock};

fn open_writer(vfs: &Arc<dyn Vfs>, root: &std::path::Path) -> Result<Store, StoreError> {
    found_on_opening(vfs, root).map(|(store, _)| store)
}

fn found_on_opening(
    vfs: &Arc<dyn Vfs>,
    root: &std::path::Path,
) -> Result<(Store, RecoveryNeed), StoreError> {
    Store::open(
        Arc::clone(vfs),
        root.to_path_buf(),
        StoreOptions::without_nonce_source(test_agent()),
    )
}

fn open_reader(vfs: &Arc<dyn Vfs>, root: &std::path::Path) -> Store {
    Store::open_read_only(
        Arc::clone(vfs),
        root.to_path_buf(),
        StoreOptions::without_nonce_source(test_agent()),
    )
    .expect("open to read")
}

/// Logs a transaction as a crash would leave it: begun and staged, and prepared if `prepare`.
fn logged(store: &Store, label: &str, node: &str, text: &[u8], prepare: bool) -> Nonce {
    let swaps = SwapSet::single(Swap::new(name(node), doc("record.json"), body(text)));
    let txn = nonce(label);
    store.commit_log().append(&begin(&txn)).unwrap();
    store.stage_transaction_for_test(&txn, &swaps).unwrap();
    if prepare {
        store
            .commit_log()
            .append(&crate::commit_log::LogRecord::Prepare {
                txn: txn.clone(),
                prepared: prepared(&swaps),
            })
            .unwrap();
    }
    txn
}

/// A store whose log holds two prepared transactions and one that was only begun.
fn store_with_three_logged() -> (Arc<dyn Vfs>, Store) {
    let (vfs, store) = fresh_store();
    logged(&store, "t1", "one", b"first", true);
    logged(&store, "t2", "two", b"second", true);
    logged(&store, "t3", "three", b"third", false);
    (vfs, store)
}

fn assert_recovered(store: &Store, vfs: &Arc<dyn Vfs>) {
    assert_eq!(
        store.read_body(name("one"), &doc("record.json")).unwrap(),
        Some(body(b"first"))
    );
    assert_eq!(
        store.read_body(name("two"), &doc("record.json")).unwrap(),
        Some(body(b"second"))
    );
    assert_eq!(
        store.read_body(name("three"), &doc("record.json")).unwrap(),
        None,
        "a transaction that was only begun rolls back"
    );
    for label in ["t1", "t2", "t3"] {
        for index in 0..store.pointer_replica_count() {
            assert!(!staging_exists(vfs, store, index, &nonce(label)));
        }
    }
}

/// A store over a filesystem whose first namespace swap fails once the returned gate opens, so
/// that a transaction fails after its commit point and leaves recovery to finish it.
fn store_with_an_unresolved_transaction(label: &str) -> (Arc<dyn Vfs>, Store) {
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            namespace_tree_staging(),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from(crate::tests::MEM_ROOT).join(label);
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root, test_config()).unwrap();
    let swaps = SwapSet::new([Swap::new(name("alpha"), doc("pin"), body(b"committed"))]).unwrap();
    gate.open();
    assert!(store.apply_transaction_as(&swaps, &nonce("u1")).is_err());
    gate.close();
    assert!(store.recovery_required().unwrap());
    (vfs, store)
}

#[test]
fn a_fence_refuses_another_attached_writer_until_either_handle_recovers() {
    let (vfs, first) = store_with_an_unresolved_transaction("shared-fence");
    let second = open_writer(&vfs, first.root()).unwrap();
    assert!(second.recovery_required().unwrap());
    assert!(matches!(
        second.update_document_as(
            Swap::new(name("alpha"), doc("later"), body(b"later")),
            &nonce("later")
        ),
        Err(StoreError::RecoveryRequired { .. })
    ));
    second.recover().unwrap().agent_stamp.unwrap();
    assert!(!first.recovery_required().unwrap());
    first
        .update_document_as(
            Swap::new(name("alpha"), doc("later"), body(b"later")),
            &nonce("later"),
        )
        .unwrap();
}

#[test]
fn two_recovery_runs_interleave_without_reusing_stale_phase_records() {
    let (vfs, first) = store_with_three_logged();
    let second = open_writer(&vfs, first.root()).unwrap();
    let mut left = first.recovery().unwrap();
    let mut right = second.recovery().unwrap();
    for _ in 0..100 {
        if !left.progress().finished() {
            left.step().unwrap().agent_stamp.unwrap();
        }
        if !right.progress().finished() {
            right.step().unwrap().agent_stamp.unwrap();
        }
        if left.progress().finished() && right.progress().finished() {
            break;
        }
    }
    assert!(left.progress().finished() && right.progress().finished());
    assert_recovered(&first, &vfs);
    assert_recovered(&second, &vfs);
}

#[test]
fn a_paused_recovery_cannot_clear_a_later_transactions_log() {
    let (vfs, first) = store_with_an_unresolved_transaction("stale-recovery");
    let second = open_writer(&vfs, first.root()).unwrap();
    let mut paused = first.recovery().unwrap();
    while paused.progress().phase() != RecoveryPhase::Completion {
        paused.step().unwrap().agent_stamp.unwrap();
    }
    second.recover().unwrap().agent_stamp.unwrap();
    logged(&second, "later", "later", b"new committed", true);
    {
        let _writer = second.exclusive();
        let _modification = second.begin_modification().unwrap();
    }
    for _ in 0..30 {
        paused.step().unwrap().agent_stamp.unwrap();
        if paused.progress().finished() {
            break;
        }
    }
    assert!(paused.progress().finished());
    assert_eq!(
        first.read_body(name("later"), &doc("record.json")).unwrap(),
        Some(body(b"new committed"))
    );
}

#[test]
fn recovery_completion_releases_a_claim_left_by_an_intervening_document_writer() {
    assert!(
        !claim_left_after_intervening_document_writer(RecoveryPhase::Completion),
        "recovery reported completion while the intervening writer's claim remained"
    );
}

#[test]
fn recovery_publication_releases_a_claim_left_by_an_intervening_document_writer() {
    assert!(!claim_left_after_intervening_document_writer(
        RecoveryPhase::Publication
    ));
}

#[test]
fn recovery_claims_releases_a_claim_left_by_an_intervening_document_writer() {
    assert!(!claim_left_after_intervening_document_writer(
        RecoveryPhase::Claims
    ));
}

fn claim_left_after_intervening_document_writer(paused_at: RecoveryPhase) -> bool {
    let mem = Arc::new(MemFs::new());
    let vfs: Arc<dyn Vfs> = mem.clone();
    let root = PathBuf::from(crate::tests::MEM_ROOT).join("late-production-claim");
    vfs.create_dir_all(&root).unwrap();
    let first = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    let mut run = first.recovery().unwrap();
    for _ in 0..32 {
        if run.progress().phase() == paused_at {
            break;
        }
        run.step().unwrap().agent_stamp.unwrap();
    }
    assert_eq!(run.progress().phase(), paused_at);
    assert!(!run.progress().finished());

    let claim = root
        .join(crate::store::CLAIMS_DIR)
        .join(nonce("interrupted-document").path_component().as_ref());
    let observed_claim = claim.clone();
    let observed_vfs = Arc::clone(&vfs);
    let claims_directory = root.join(crate::store::CLAIMS_DIR);
    let claim_sync = PathMatcher::predicate(move |path| {
        path == claims_directory && observed_vfs.metadata(&observed_claim).is_ok()
    });
    let gate = FaultGate::closed();
    let writer_vfs: Arc<FaultyFs<MemFs>> =
        Arc::new_cyclic(|weak: &std::sync::Weak<FaultyFs<MemFs>>| {
            let dying_process = weak.clone();
            FaultyFs::new(
                mem.another_process(),
                FaultPolicy::new().inject_gated(
                    FaultOp::Fsync,
                    claim_sync,
                    Fault::observe_with(move |_, _| {
                        dying_process.upgrade().unwrap().inner().abandon();
                    }),
                    When::Once,
                    gate.clone(),
                ),
            )
        });
    let (writer, _) = Store::open(
        writer_vfs,
        root.clone(),
        StoreOptions::with_nonce_source(test_agent(), || nonce("interrupted-document")),
    )
    .unwrap();
    let before = first.modifications().unwrap();
    gate.open();
    assert!(
        writer
            .write_document_replicated(name("late"), &doc("record.json"), &body(b"unwritten"))
            .is_err()
    );
    assert!(
        vfs.metadata(&claim).is_ok(),
        "the real write left its claim"
    );
    assert_eq!(first.modifications().unwrap(), before);
    assert_eq!(
        first.read_body(name("late"), &doc("record.json")).unwrap(),
        None
    );
    for _ in 0..32 {
        if run.progress().finished() {
            break;
        }
        run.step().unwrap().agent_stamp.unwrap();
    }
    assert!(run.progress().finished());
    let claim_left = vfs.metadata(&claim).is_ok();
    first.recover().unwrap().agent_stamp.unwrap();
    assert!(
        vfs.metadata(&claim).is_err(),
        "a fresh recovery releases the same stale claim"
    );
    claim_left
}

/// Every file beneath `root` with its contents, other than the coordination's own and the lock.
fn tree(vfs: &Arc<dyn Vfs>, root: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(vfs: &Arc<dyn Vfs>, dir: &std::path::Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in vfs.read_dir(dir).unwrap() {
            let name = &entry.file_name;
            if name.starts_with(".coord") || name == ".lock" {
                continue;
            }
            let path = dir.join(&entry.file_name);
            if vfs.metadata(&path).unwrap().is_dir() {
                out.push((path.clone(), Vec::new()));
                walk(vfs, &path, out);
            } else {
                out.push((path.clone(), vfs.read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(vfs, root, &mut out);
    out.sort();
    out
}

#[test]
fn opening_a_root_a_failed_transaction_left_changes_no_store_data_and_requires_recovery() {
    let (vfs, writer) = store_with_an_unresolved_transaction("attach-dirty");
    let root = writer.root().to_path_buf();
    drop(writer);
    let before = tree(&vfs, &root);

    let attached = open_writer(&vfs, &root).unwrap();

    assert_eq!(tree(&vfs, &root), before, "opening moved store data");
    assert!(attached.recovery_required().unwrap());
    let need = attached.recovery_need().unwrap();
    assert!(need.required() && need.fenced(), "{need:?}");
}

#[test]
fn a_handle_that_needs_recovery_refuses_writes_and_still_reads_until_recovery_completes() {
    let (vfs, writer) = store_with_an_unresolved_transaction("attach-refuses");
    let root = writer.root().to_path_buf();
    drop(writer);
    let attached = open_writer(&vfs, &root).unwrap();

    assert!(matches!(
        attached.write_document_replicated_as(
            name("beta"),
            &doc("two"),
            &body(b"no"),
            &nonce("w1")
        ),
        Err(StoreError::RecoveryRequired { .. })
    ));
    assert!(attached.read_body(name("beta"), &doc("two")).is_ok());

    attached
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");

    assert!(!attached.recovery_required().unwrap());
    assert_eq!(
        attached.read_body(name("alpha"), &doc("pin")).unwrap(),
        Some(body(b"committed")),
        "a prepared transaction rolls forward"
    );
    attached
        .write_document_replicated_as(name("beta"), &doc("two"), &body(b"yes"), &nonce("w2"))
        .unwrap();
}

#[test]
fn a_handle_that_needs_recovery_refuses_repairs_scrub_and_reset_without_changing_the_store() {
    let (vfs, writer) = store_with_an_unresolved_transaction("attach-refuses-repair");
    let root = writer.root().to_path_buf();
    drop(writer);
    let attached = open_writer(&vfs, &root).unwrap();

    let (_source_vfs, source) = fresh_store();
    source
        .write_document_replicated_as(
            name("alpha"),
            &doc("pin"),
            &body(b"committed"),
            &nonce("source"),
        )
        .unwrap();
    let pointer = source.read_pointer(&name("alpha"), &doc("pin")).unwrap();
    let reference = pointer.pointer().target();
    let before = tree(&vfs, &root);

    let refused = [
        (
            "repair_blob",
            attached
                .repair_blob(&reference, b"committed", &[], &[])
                .map(|_| ()),
        ),
        (
            "repair_pointer",
            attached
                .repair_pointer(&name("alpha"), &doc("pin"), &pointer)
                .map(|_| ()),
        ),
        ("scrub_content", attached.scrub_content().map(|_| ())),
        (
            "repair_reported_damage",
            attached.repair_reported_damage().map(|_| ()),
        ),
        ("quiesce", attached.quiesce().map(|outcome| outcome.value)),
    ];
    for (verb, result) in refused {
        assert!(
            matches!(result, Err(StoreError::RecoveryRequired { .. })),
            "{verb} must be refused until recovery: {result:?}"
        );
    }

    assert_eq!(
        tree(&vfs, &root),
        before,
        "a refused verb changed store data"
    );
    assert!(!attached.commit_log_is_empty().unwrap());
}

#[test]
fn opening_a_root_that_needs_nothing_requires_no_recovery() {
    let (vfs, store) = fresh_store();
    let root = store.root().to_path_buf();
    drop(store);
    let attached = open_writer(&vfs, &root).unwrap();
    assert!(!attached.recovery_required().unwrap());
    assert!(!attached.recovery_need().unwrap().required());
    attached
        .write_document_replicated_as(name("alpha"), &doc("one"), &body(b"v"), &nonce("w1"))
        .unwrap();
}

#[test]
fn a_transaction_that_failed_unresolved_is_recovered_without_reopening() {
    let (_vfs, store) = store_with_an_unresolved_transaction("recover-in-place");
    let odd = store.modifications().unwrap();
    assert!(!Store::undisturbed(odd, odd));

    store
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");

    let settled = store.modifications().unwrap();
    assert!(Store::undisturbed(settled, settled), "{settled}");
    assert!(!store.recovery_required().unwrap());
    assert!(!store.recovery_need().unwrap().required());
    assert_eq!(
        store.read_body(name("alpha"), &doc("pin")).unwrap(),
        Some(body(b"committed"))
    );
    store
        .write_document_replicated_as(name("beta"), &doc("two"), &body(b"later"), &nonce("w1"))
        .unwrap();
}

#[test]
fn discovery_by_a_reader_and_by_a_writer_agree_and_both_see_recovery_finish() {
    let (vfs, writer) = store_with_an_unresolved_transaction("discover-both");
    let root = writer.root().to_path_buf();
    drop(writer);
    let reader = open_reader(&vfs, &root);
    let need = reader.recovery_need().unwrap();
    assert!(need.fenced() && need.required(), "{need:?}");
    assert!(!need.holder_died(), "the writer released cleanly");

    let writer = open_writer(&vfs, &root).unwrap();
    assert!(writer.recovery_need().unwrap().fenced());
    assert!(reader.recovery_need().unwrap().fenced());
    writer
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(!writer.recovery_need().unwrap().required());
    assert!(!reader.recovery_need().unwrap().required());
}

#[test]
fn a_live_writers_transaction_is_not_reported_as_abandoned() {
    let reports: Arc<Mutex<Vec<bool>>> = Arc::default();
    let reader_cell: Arc<OnceLock<Store>> = Arc::default();
    let gate = FaultGate::closed();
    let (recorded, watcher) = (Arc::clone(&reports), Arc::clone(&reader_cell));
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(NAMESPACE_TREE_DIR),
            Fault::observe_with(move |_, _| {
                if let Some(reader) = watcher.get() {
                    recorded
                        .lock()
                        .unwrap()
                        .push(reader.recovery_need().unwrap().required());
                }
            }),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from(crate::tests::MEM_ROOT).join("live-writer");
    vfs.create_dir_all(&root).unwrap();
    let writer = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    let _ = reader_cell.set(open_reader(&vfs, &root));

    gate.open();
    writer
        .write_document_replicated_as(name("alpha"), &doc("one"), &body(b"x"), &nonce("w1"))
        .unwrap();
    gate.close();

    let reports = reports.lock().unwrap().clone();
    assert_eq!(reports.len(), writer.pointer_replica_count());
    assert!(
        reports.iter().all(|required| !required),
        "a write in progress was reported as abandoned: {reports:?}"
    );
}

#[test]
fn state_nothing_has_vouched_for_is_reported_to_a_reader_and_recovery_vouches_for_it() {
    let (vfs, writer) = fresh_store();
    let root = writer.root().to_path_buf();
    drop(writer);
    vfs.remove_dir_all(&root.join(".coord")).unwrap();

    let reader = open_reader(&vfs, &root);
    let need = reader.recovery_need().unwrap();
    assert!(need.state_untrusted() && need.required(), "{need:?}");
    assert!(!need.fenced());

    let writer = open_writer(&vfs, &root).unwrap();
    assert!(writer.recovery_required().unwrap());
    writer
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(!reader.recovery_need().unwrap().required());
    let count = reader.modifications().unwrap();
    assert!(Store::undisturbed(count, count));
}

#[test]
fn the_steps_report_each_phase_in_order_and_the_totals_the_run_knows() {
    let (vfs, store) = store_with_three_logged();
    let mut run = store.recovery().unwrap();
    let first: RecoveryProgress = run.progress();
    assert_eq!(
        (
            first.phase(),
            first.done(),
            first.known(),
            first.total_grew(),
            first.finished()
        ),
        (RecoveryPhase::Publication, 0, 3, false, false),
        "nothing recorded, so the first phase with work is the publication's"
    );

    let mut seen = Vec::new();
    while !run.progress().finished() {
        let p = run.step().unwrap();
        seen.push((p.phase(), p.done(), p.known(), p.total_grew(), p.finished()));
    }
    use RecoveryPhase::{Claims, Collection, Completion, Transactions};
    assert_eq!(
        seen,
        vec![
            (Transactions, 1, 6, true, false),
            (Transactions, 2, 6, false, false),
            (Transactions, 3, 6, false, false),
            (Collection, 4, 9, true, false),
            (Collection, 5, 9, false, false),
            (Collection, 6, 9, false, false),
            (Claims, 7, 9, false, false),
            (Completion, 8, 9, false, false),
            (Completion, 9, 9, false, true),
        ]
    );
    assert_recovered(&store, &vfs);
}

#[test]
fn a_step_on_a_finished_run_changes_nothing() {
    let (_vfs, store) = fresh_store();
    let mut run = store.recovery().unwrap();
    while !run.progress().finished() {
        run.step()
            .unwrap()
            .agent_stamp
            .expect("agent version stamp");
    }
    let done = run.progress();
    let log = store.commit_log().read().unwrap().len();
    for _ in 0..3 {
        assert_eq!(run.step().unwrap().value, done);
    }
    assert_eq!(store.commit_log().read().unwrap().len(), log);
}

#[test]
fn a_run_stopped_at_any_step_is_finished_by_a_later_run_on_the_same_handle() {
    for stop in 0..=9 {
        let (vfs, store) = store_with_three_logged();
        let mut run = store.recovery().unwrap();
        for _ in 0..stop {
            if !run.progress().finished() {
                run.step()
                    .unwrap()
                    .agent_stamp
                    .expect("agent version stamp");
            }
        }
        drop(run);
        store
            .recover()
            .unwrap()
            .agent_stamp
            .expect("agent version stamp");
        assert_recovered(&store, &vfs);
    }
}

#[test]
fn a_run_stopped_at_any_step_is_finished_by_resolving_after_the_next_open() {
    for stop in 0..=9 {
        let (vfs, store) = store_with_three_logged();
        let root = store.root().to_path_buf();
        let mut run = store.recovery().unwrap();
        for _ in 0..stop {
            if !run.progress().finished() {
                run.step()
                    .unwrap()
                    .agent_stamp
                    .expect("agent version stamp");
            }
        }
        drop(run);
        drop(store);
        let reopened = open_resolved(Arc::clone(&vfs), root).unwrap();
        assert!(
            !reopened.recovery_required().unwrap(),
            "stopped after {stop}"
        );
        assert_recovered(&reopened, &vfs);
    }
}

#[test]
fn a_run_stopped_before_it_completes_leaves_the_fence_and_the_next_open_is_told() {
    let (vfs, store) = store_with_an_unresolved_transaction("stopped-fenced");
    let root = store.root().to_path_buf();
    let mut run = store.recovery().unwrap();
    // Everything up to the last step, which is the one that clears the fence.
    while run.progress().phase() != RecoveryPhase::Completion {
        run.step()
            .unwrap()
            .agent_stamp
            .expect("agent version stamp");
    }
    drop(run);
    drop(store);

    let attached = open_writer(&vfs, &root).unwrap();
    assert!(
        attached.recovery_required().unwrap(),
        "the fence outlives a recovery that stopped"
    );
    attached
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(!attached.recovery_need().unwrap().required());
}

#[test]
fn a_step_that_fails_is_retried_by_the_next() {
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            namespace_tree_staging(),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from(crate::tests::MEM_ROOT).join("retry");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root, test_config()).unwrap();
    logged(&store, "t1", "one", b"first", true);
    logged(&store, "t2", "two", b"second", true);
    logged(&store, "t3", "three", b"third", false);

    gate.open();
    let mut run = store.recovery().unwrap();
    let mut failures = 0;
    while !run.progress().finished() {
        match run.step() {
            Ok(_) => {}
            Err(_) => failures += 1,
        }
        assert!(failures <= 1, "one injected failure, one retry");
    }
    gate.close();
    assert_eq!(failures, 1);
    assert_recovered(&store, &vfs);
}

#[test]
fn directory_sync_debt_is_a_phase_of_its_own_counted_by_directory() {
    let (vfs, store) = fresh_store();
    let root = store.root().to_path_buf();
    let record = root.join(crate::store::DIR_SYNC_DEBT_NAME);
    vfs.create_dir_all(&root.join("files").join("p0").join("ab"))
        .unwrap();
    vfs.write(
        &record,
        b"files/p0/ab\nfiles/p0/gone\n../outside\nunterminated",
    )
    .unwrap();

    let mut run = store.recovery().unwrap();
    let first = run.progress();
    assert_eq!(first.phase(), RecoveryPhase::DirectorySyncDebt);
    assert_eq!(
        first.known(),
        3 + 2 + 1,
        "the fixed units, two valid directories and the record's emptying"
    );
    while !run.progress().finished() {
        run.step()
            .unwrap()
            .agent_stamp
            .expect("agent version stamp");
    }
    assert!(
        vfs.read(&record).unwrap().is_empty(),
        "the record is emptied once every directory is synced"
    );
}

#[test]
fn a_pending_publication_is_finished_by_its_own_phase() {
    use super::publication::{faulty_store, v2_fails_at_replica_2, write_v2_short_of_replica_2};
    let (_faulty, _vfs, store) = faulty_store("pending-publication", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);
    assert!(!store.read_pointer(&node, &d).unwrap().is_unanimous());

    let mut run = store.recovery().unwrap();
    assert_eq!(run.progress().phase(), RecoveryPhase::Publication);
    run.step()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(store.read_pointer(&node, &d).unwrap().is_unanimous());
    while !run.progress().finished() {
        run.step()
            .unwrap()
            .agent_stamp
            .expect("agent version stamp");
    }
}

#[test]
fn claims_of_operations_that_died_are_released_by_the_claims_phase() {
    let (vfs, store) = fresh_store();
    let claim = store.root().join(crate::store::CLAIMS_DIR).join("dead-op");
    vfs.create_dir_all(&claim).unwrap();
    let mut run = store.recovery().unwrap();
    while run.progress().phase() != RecoveryPhase::Claims {
        assert!(!run.progress().finished(), "the run passed the claims");
        run.step()
            .unwrap()
            .agent_stamp
            .expect("agent version stamp");
    }
    assert!(vfs.metadata(&claim).is_ok(), "not yet released");
    run.step()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(vfs.metadata(&claim).is_err(), "released by its phase");
}

#[test]
fn a_reader_cannot_recover_and_a_reader_can_ask() {
    let (vfs, store) = fresh_store();
    let reader = open_reader(&vfs, store.root());
    assert!(matches!(
        reader.recovery(),
        Err(StoreError::ReadOnly { .. })
    ));
    assert!(matches!(reader.recover(), Err(StoreError::ReadOnly { .. })));
    assert!(!reader.recovery_need().unwrap().required());
}

#[test]
fn recovering_a_store_that_needs_none_leaves_its_count_and_documents_as_they_were() {
    let (_vfs, store) = fresh_store();
    store
        .write_document_replicated_as(name("alpha"), &doc("one"), &body(b"kept"), &nonce("w1"))
        .unwrap();
    let before = store.modifications().unwrap();
    store
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert_eq!(store.modifications().unwrap(), before);
    assert_eq!(
        store.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(b"kept"))
    );
}

#[test]
fn clones_of_a_handle_share_the_recovery_that_finishes() {
    let (_vfs, store) = store_with_an_unresolved_transaction("clones-recover");
    let clone = store.clone();
    assert!(clone.recovery_required().unwrap());
    clone
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(!store.recovery_required().unwrap());
    store
        .write_document_replicated_as(name("beta"), &doc("two"), &body(b"v"), &nonce("w1"))
        .unwrap();
}

#[test]
fn an_open_reports_what_a_failed_transaction_left_and_resolving_makes_it_writable() {
    let (vfs, store) = store_with_an_unresolved_transaction("open-reports");
    let root = store.root().to_path_buf();
    drop(store);
    let before = tree(&vfs, &root);

    let (opened, found) = found_on_opening(&vfs, &root).unwrap();

    assert_eq!(tree(&vfs, &root), before, "opening moved store data");
    assert!(found.fenced() && found.required(), "{found:?}");
    assert!(
        !found.state_untrusted() && !found.holder_died(),
        "{found:?}"
    );
    assert_eq!(found.unresolved_work(), Some(true));
    assert!(opened.recovery_required().unwrap());
    assert!(matches!(
        opened.write_document_replicated_as(name("beta"), &doc("two"), &body(b"no"), &nonce("w0")),
        Err(StoreError::RecoveryRequired { .. })
    ));

    opened
        .resolve()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");

    assert!(
        !opened.recovery_required().unwrap(),
        "resolving was all it needed"
    );
    assert_eq!(
        opened.recovery_need().unwrap().unresolved_work(),
        Some(false)
    );
    assert!(!opened.recovery_need().unwrap().required());
    assert_eq!(
        opened.read_body(name("alpha"), &doc("pin")).unwrap(),
        Some(body(b"committed")),
        "the prepared transaction rolled forward"
    );
    opened
        .write_document_replicated_as(name("beta"), &doc("two"), &body(b"v"), &nonce("w1"))
        .unwrap();
}

#[test]
fn resolving_rolls_a_begun_transaction_back_and_a_prepared_one_forward() {
    let (vfs, store) = store_with_three_logged();
    let opened = reopen(&vfs, store).unwrap();
    assert_recovered(&opened, &vfs);
}

#[test]
fn an_open_does_not_recover_state_nothing_has_vouched_for() {
    let (vfs, writer) = fresh_store();
    write_one(&writer);
    let root = writer.root().to_path_buf();
    drop(writer);
    vfs.remove_dir_all(&root.join(".coord")).unwrap();

    let opened = Store::open_unsourced(Arc::clone(&vfs), root).unwrap();

    assert!(opened.recovery_required().unwrap());
    assert!(opened.recovery_need().unwrap().state_untrusted());
    assert_eq!(
        opened.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(b"kept")),
        "reads are allowed while recovery is required"
    );
    assert!(matches!(
        opened.write_document_replicated_as(name("beta"), &doc("two"), &body(b"no"), &nonce("w2")),
        Err(StoreError::RecoveryRequired { .. })
    ));

    opened
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");

    assert!(!opened.recovery_required().unwrap());
    assert!(!opened.recovery_need().unwrap().required());
    opened
        .write_document_replicated_as(name("beta"), &doc("two"), &body(b"yes"), &nonce("w3"))
        .unwrap();
}

#[test]
fn an_open_after_a_host_restart_needs_recovery_and_loses_nothing_that_was_durable() {
    let mem = Arc::new(MemFs::new());
    let vfs: Arc<dyn Vfs> = mem.clone();
    let root = PathBuf::from(crate::tests::MEM_ROOT).join("restarted");
    vfs.create_dir_all(&root).unwrap();
    let writer = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    write_one(&writer);
    drop(writer);
    let vfs: Arc<dyn Vfs> = Arc::new(mem.simulate_restart());

    let opened = Store::open_unsourced(Arc::clone(&vfs), root).unwrap();

    assert!(
        opened.recovery_required().unwrap(),
        "state is not trusted across a restart"
    );
    assert!(opened.recovery_need().unwrap().state_untrusted());
    opened
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(!opened.recovery_required().unwrap());
    assert_eq!(
        opened.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(b"kept"))
    );
}

fn write_one(store: &Store) {
    store
        .write_document_replicated_as(name("alpha"), &doc("one"), &body(b"kept"), &nonce("w1"))
        .unwrap();
}

#[test]
fn a_clean_root_opens_with_nothing_found() {
    let (vfs, store) = fresh_store();
    write_one(&store);
    let root = store.root().to_path_buf();
    drop(store);

    let (opened, found) = found_on_opening(&vfs, &root).unwrap();

    assert!(!found.required(), "{found:?}");
    assert_eq!(found.unresolved_work(), Some(false));
    assert!(!opened.recovery_required().unwrap());
    write_one(&opened);
}

/// Leaves one kind of work on the root's work list of a store nothing else is wrong with, and
/// reports whether it is still there.
type Leftover = (
    &'static str,
    fn(&Arc<dyn Vfs>, &Store),
    fn(&Arc<dyn Vfs>, &Store) -> bool,
);

fn leftovers() -> [Leftover; 3] {
    fn claim(store: &Store) -> PathBuf {
        store.root().join(crate::store::CLAIMS_DIR).join("dead-op")
    }
    fn record(store: &Store) -> PathBuf {
        store.root().join(crate::store::DIR_SYNC_DEBT_NAME)
    }
    [
        (
            "a dead operation's claim",
            |vfs, store| vfs.create_dir_all(&claim(store)).unwrap(),
            |vfs, store| vfs.metadata(&claim(store)).is_ok(),
        ),
        (
            "a directory whose name was not synced",
            |vfs, store| {
                vfs.create_dir_all(&store.root().join("files").join("p0").join("ab"))
                    .unwrap();
                vfs.write(&record(store), b"files/p0/ab\n").unwrap();
            },
            |vfs, store| !vfs.read(&record(store)).unwrap().is_empty(),
        ),
        (
            "a transaction that was only begun",
            |_, store| {
                logged(store, "t9", "nine", b"never", false);
            },
            |_, store| !store.commit_log().read().unwrap().is_empty(),
        ),
    ]
}

#[test]
fn each_kind_of_work_left_behind_is_reported_and_requires_no_recovery() {
    for (what, leave, present) in leftovers() {
        let (vfs, store) = fresh_store();
        leave(&vfs, &store);
        let root = store.root().to_path_buf();
        drop(store);

        let (opened, found) = found_on_opening(&vfs, &root).unwrap();

        assert_eq!(found.unresolved_work(), Some(true), "{what}");
        assert!(!found.required(), "{what}: {found:?}");
        assert!(present(&vfs, &opened), "{what}: opening moved it");
        assert!(!opened.recovery_required().unwrap(), "{what}");
    }
}

#[test]
fn resolving_finishes_each_kind_of_work_left_behind() {
    for (what, leave, present) in leftovers() {
        let (vfs, store) = fresh_store();
        leave(&vfs, &store);
        let root = store.root().to_path_buf();
        drop(store);
        let opened = open_writer(&vfs, &root).unwrap();

        opened
            .resolve()
            .unwrap()
            .agent_stamp
            .expect("agent version stamp");

        assert!(!present(&vfs, &opened), "{what}: still there");
        assert_eq!(
            opened.recovery_need().unwrap().unresolved_work(),
            Some(false),
            "{what}"
        );
    }
}

#[test]
fn a_pending_publication_is_reported_on_opening_and_resolved() {
    use super::publication::{faulty_store, v2_fails_at_replica_2, write_v2_short_of_replica_2};
    let (_faulty, vfs, store) = faulty_store("reported-publication", v2_fails_at_replica_2());
    let (node, d) = (name("tickets"), doc("record"));
    write_v2_short_of_replica_2(&store, &node, &d);
    let root = store.root().to_path_buf();
    drop(store);

    let (opened, found) = found_on_opening(&vfs, &root).unwrap();

    assert_eq!(found.unresolved_work(), Some(true));
    assert!(!opened.read_pointer(&node, &d).unwrap().is_unanimous());
    opened
        .resolve()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(opened.read_pointer(&node, &d).unwrap().is_unanimous());
    assert_eq!(
        opened.recovery_need().unwrap().unresolved_work(),
        Some(false)
    );
}

#[test]
fn work_left_behind_cannot_be_told_from_a_live_operations_by_a_reader_or_from_inside_an_operation()
{
    let (vfs, store) = fresh_store();
    let (_, leave, _) = leftovers()[0];
    leave(&vfs, &store);

    let reader = open_reader(&vfs, store.root());
    assert_eq!(
        reader.recovery_need().unwrap().unresolved_work(),
        Some(true)
    );
    assert!(matches!(reader.resolve(), Err(StoreError::ReadOnly { .. })));

    let inside = store.exclusive();
    assert_eq!(store.recovery_need().unwrap().unresolved_work(), None);
    drop(inside);
    assert_eq!(store.recovery_need().unwrap().unresolved_work(), Some(true));
}

#[test]
fn resolving_a_store_with_nothing_left_changes_nothing_and_may_be_repeated() {
    let (vfs, store) = fresh_store();
    write_one(&store);
    let root = store.root().to_path_buf();
    let before = tree(&vfs, &root);
    let count = store.modifications().unwrap();

    store
        .resolve()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    store
        .resolve()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");

    assert_eq!(tree(&vfs, &root), before);
    assert_eq!(store.modifications().unwrap(), count);
}

#[test]
fn resolving_state_nothing_has_vouched_for_finishes_the_work_and_leaves_recovery_required() {
    let (vfs, store) = fresh_store();
    let (_, leave, present) = leftovers()[0];
    leave(&vfs, &store);
    let root = store.root().to_path_buf();
    drop(store);
    vfs.remove_dir_all(&root.join(".coord")).unwrap();

    let (opened, found) = found_on_opening(&vfs, &root).unwrap();
    assert!(found.state_untrusted() && found.required(), "{found:?}");
    assert_eq!(found.unresolved_work(), Some(true));

    opened
        .resolve()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");

    assert!(!present(&vfs, &opened));
    assert!(
        opened.recovery_required().unwrap(),
        "resolving vouches for nothing"
    );
    opened
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    assert!(!opened.recovery_required().unwrap());
}
