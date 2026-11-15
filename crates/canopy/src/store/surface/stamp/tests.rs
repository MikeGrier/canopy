// Copyright (c) Michael Grier.

use super::*;
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, When};
use std::sync::atomic::{AtomicUsize, Ordering};

fn options(version: &str) -> StoreOptions {
    let mut next = 0;
    StoreOptions::with_nonce_source(
        Agent::new("stamp-tests", version, "original description", 1),
        move || {
            next += 1;
            Nonce::new(&format!("n{next}")).unwrap()
        },
    )
}

fn fixture(policy: FaultPolicy) -> (Arc<FaultyFs<MemFs>>, Arc<dyn Vfs>, Store) {
    let fs = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let vfs: Arc<dyn Vfs> = fs.clone();
    let root = PathBuf::from("stamp-store");
    vfs.create_dir_all(&root).unwrap();
    let config = GenesisConfig::new(StoreId::new("stamping").unwrap(), 123);
    let store = Store::init(Arc::clone(&vfs), root, config, options("A")).unwrap();
    (fs, vfs, store)
}

fn reopen(vfs: &Arc<dyn Vfs>, store: Store, version: &str) -> Store {
    let root = store.root.clone();
    drop(store);
    Store::open(Arc::clone(vfs), root, options(version))
        .unwrap()
        .0
}

fn metadata(vfs: &Arc<dyn Vfs>, store: &Store) -> serde_json::Value {
    serde_json::from_slice(&vfs.read(&store.root.join(MARKER_NAME)).unwrap()).unwrap()
}

fn write(store: &Store, logged: bool, bytes: &[u8]) -> Result<WriteOutcome<()>, StoreError> {
    let node = NodePath::parse("docs").unwrap();
    let document = DocumentName::new("one").unwrap();
    let body = DocumentBody::new(bytes.to_vec());
    if logged {
        store
            .update_document(Swap::new(node, document, body))
            .map(|outcome| {
                assert!(outcome.is_committed());
                outcome.map(|_| ())
            })
    } else {
        store
            .write_document_replicated(node, &document, &body)
            .map(|outcome| outcome.map(|_| ()))
    }
}

#[test]
fn successful_writers_record_a_b_and_a_without_ordering_their_versions() {
    for logged in [false, true] {
        let (_, vfs, mut store) = fixture(FaultPolicy::new());
        let original = metadata(&vfs, &store);
        for version in ["A", "B", "A", "build-20", "build-3"] {
            let before = metadata(&vfs, &store);
            store = reopen(&vfs, store, version);
            assert_eq!(metadata(&vfs, &store), before, "open never stamps");
            write(&store, logged, version.as_bytes())
                .unwrap()
                .agent_stamp
                .unwrap();
            let mut expected = original.clone();
            expected["metadata"]["agent_version"] = version.into();
            assert_eq!(metadata(&vfs, &store), expected);
        }
    }
}

#[test]
fn an_unchanged_version_performs_no_marker_io_after_open() {
    let reads = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let read_count = Arc::clone(&reads);
    let write_count = Arc::clone(&writes);
    let policy = FaultPolicy::new()
        .inject_matching(
            FaultOp::Read,
            PathMatcher::file_name(MARKER_NAME),
            Fault::observe_with(move |_, _| {
                read_count.fetch_add(1, Ordering::Relaxed);
            }),
            When::Always,
        )
        .inject_matching(
            FaultOp::Write,
            PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
            Fault::observe_with(move |_, _| {
                write_count.fetch_add(1, Ordering::Relaxed);
            }),
            When::Always,
        );
    let (_, vfs, store) = fixture(policy);
    write(&store, true, b"first acquisition")
        .unwrap()
        .agent_stamp
        .unwrap();
    let read_before = reads.load(Ordering::Relaxed);
    let write_before = writes.load(Ordering::Relaxed);
    for index in 0..10 {
        write(&store, index % 2 == 0, b"value")
            .unwrap()
            .agent_stamp
            .unwrap();
    }
    assert_eq!(reads.load(Ordering::Relaxed), read_before);
    assert_eq!(writes.load(Ordering::Relaxed), write_before);
    let store = reopen(&vfs, store, "B");
    write(&store, true, b"new").unwrap().agent_stamp.unwrap();
    let read_before = reads.load(Ordering::Relaxed);
    let write_before = writes.load(Ordering::Relaxed);
    write(&store.clone(), false, b"again")
        .unwrap()
        .agent_stamp
        .unwrap();
    assert_eq!(reads.load(Ordering::Relaxed), read_before);
    assert_eq!(writes.load(Ordering::Relaxed), write_before);
}

#[test]
fn attached_writers_reload_the_agent_stamp_after_another_handles_turn() {
    let (_, vfs, first) = fixture(FaultPolicy::new());
    let second = Store::open(Arc::clone(&vfs), first.root.clone(), options("B"))
        .unwrap()
        .0;
    for index in 0..12 {
        let (store, version) = if index % 2 == 0 {
            (&first, "A")
        } else {
            (&second, "B")
        };
        write(store, index % 3 == 0, version.as_bytes())
            .unwrap()
            .agent_stamp
            .unwrap();
        assert_eq!(metadata(&vfs, store)["metadata"]["agent_version"], version);
        assert_eq!(
            metadata(&vfs, store)["metadata"]["agent_description"],
            "original description"
        );
    }
}

#[test]
fn a_scrub_releases_each_documents_claims_before_another_handle_resolves() {
    let gate = FaultGate::closed();
    let turn = Arc::new(AtomicUsize::new(0));
    let other: Arc<Mutex<Option<Store>>> = Arc::default();
    let (turns, target) = (Arc::clone(&turn), Arc::clone(&other));
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Lock,
        PathMatcher::file_name("exclusion.lock"),
        Fault::observe_with(move |_, _| {
            if turns.fetch_add(1, Ordering::Relaxed) == 2 {
                let store = target.lock().unwrap().as_ref().unwrap().clone();
                assert!(
                    store
                        .vfs
                        .read_dir(&store.root.join(CLAIMS_DIR))
                        .unwrap()
                        .is_empty()
                );
                store.resolve().unwrap().agent_stamp.unwrap();
            }
        }),
        When::Always,
        gate.clone(),
    );
    let (_, vfs, store) = fixture(policy);
    for index in 0..12 {
        let node = NodePath::parse("docs").unwrap();
        let document = DocumentName::new(&format!("repair{index}")).unwrap();
        store
            .update_document(Swap::new(
                node.clone(),
                document.clone(),
                DocumentBody::new(b"content".to_vec()),
            ))
            .unwrap()
            .agent_stamp
            .unwrap();
        vfs.remove_file(&store.pointer_entry_path(0, &node, &document))
            .unwrap();
    }
    *other.lock().unwrap() = Some(
        Store::open(Arc::clone(&vfs), store.root.clone(), options("A"))
            .unwrap()
            .0,
    );
    gate.open();
    let result = store.scrub_content().unwrap();
    gate.close();
    result.agent_stamp.unwrap();
    assert_eq!(result.value.0.pointer_repairs().len(), 12);
    assert_eq!(result.value.1.len(), 12);
    assert!(turn.load(Ordering::Relaxed) > 2);
    assert!(
        vfs.read_dir(&store.root.join(CLAIMS_DIR))
            .unwrap()
            .is_empty()
    );
    other.lock().unwrap().take();
}

#[test]
fn failed_document_writes_and_rejected_transactions_do_not_stamp() {
    for logged in [false, true] {
        let gate = FaultGate::closed();
        let policy = FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(BLOB_TREE_DIR),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        );
        let (_, vfs, store) = fixture(policy);
        let store = reopen(&vfs, store, "B");
        gate.open();
        assert!(write(&store, logged, b"failed").is_err());
        assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "A");
    }
    let (_, vfs, store) = fixture(FaultPolicy::new());
    write(&store, true, b"old").unwrap().agent_stamp.unwrap();
    let store = reopen(&vfs, store, "B");
    let outcome = store
        .update_document(Swap::conditional_absent(
            NodePath::parse("docs").unwrap(),
            DocumentName::new("one").unwrap(),
            DocumentBody::new(b"lost".to_vec()),
        ))
        .unwrap();
    assert!(outcome.is_rejected());
    outcome.agent_stamp.unwrap();
    assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "A");
}

#[test]
fn stamp_failures_preserve_success_and_are_retried_without_recovery() {
    for logged in [false, true] {
        for op in [FaultOp::Write, FaultOp::Fsync, FaultOp::Rename] {
            let gate = FaultGate::closed();
            let policy = FaultPolicy::new().inject_gated(
                op,
                PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
                Fault::io_error(),
                When::Once,
                gate.clone(),
            );
            let (_, vfs, store) = fixture(policy);
            let store = reopen(&vfs, store, "B");
            gate.open();
            let outcome = write(&store, logged, b"committed").unwrap();
            assert!(outcome.agent_stamp.is_err());
            assert!(!store.recovery_required().unwrap());
            assert_eq!(
                crate::read_once(
                    &store,
                    &NodePath::parse("docs").unwrap(),
                    &DocumentName::new("one").unwrap()
                )
                .unwrap()
                .unwrap()
                .as_bytes(),
                b"committed"
            );
            assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "A");
            write(&store, logged, b"next").unwrap().agent_stamp.unwrap();
            assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "B");
        }
    }
}

#[test]
fn a_directory_sync_failure_after_the_stamp_rename_keeps_the_cache_retryable() {
    let armed = FaultGate::closed();
    let renamed = FaultGate::closed();
    let signal = renamed.clone();
    let policy = FaultPolicy::new()
        .inject_gated(
            FaultOp::Rename,
            PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
            Fault::observe_with(move |_, _| signal.open()),
            When::Always,
            armed.clone(),
        )
        .inject_gated(
            FaultOp::Fsync,
            PathMatcher::exact("stamp-store"),
            Fault::io_error(),
            When::Once,
            renamed,
        );
    let (fs, vfs, store) = fixture(policy);
    let store = reopen(&vfs, store, "B");
    armed.open();
    let outcome = write(&store, true, b"committed").unwrap();
    assert!(outcome.agent_stamp.is_err());
    assert!(!store.recovery_required().unwrap());
    assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "B");
    assert_eq!(
        store
            .surface
            .stamp_cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .1
            .metadata
            .agent_version,
        "A"
    );
    write(&store, false, b"next").unwrap().agent_stamp.unwrap();
    drop(store);
    fs.inner().simulate_crash();
    let marker: serde_json::Value = serde_json::from_slice(
        &vfs.read(&PathBuf::from("stamp-store").join(MARKER_NAME))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(marker["metadata"]["agent_version"], "B");
}

#[test]
fn direct_and_scrub_repairs_stamp_only_when_replicas_change() {
    for mode in 0..4 {
        let (_, vfs, store) = fixture(FaultPolicy::new());
        write(&store, true, b"repair-me")
            .unwrap()
            .agent_stamp
            .unwrap();
        let store = reopen(&vfs, store, "B");
        let node = NodePath::parse("docs").unwrap();
        let document = DocumentName::new("one").unwrap();
        let read = store.read_pointer(&node, &document).unwrap();
        let reference = read.pointer().target();
        let noop = store
            .repair_blob(&reference, b"repair-me", &[], &[])
            .unwrap();
        assert!(noop.value.0.is_noop());
        noop.agent_stamp.unwrap();
        store.scrub_content().unwrap().agent_stamp.unwrap();
        store.repair_reported_damage().unwrap().agent_stamp.unwrap();
        assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "A");
        if mode == 1 {
            vfs.remove_file(&store.pointer_entry_path(0, &node, &document))
                .unwrap();
            store
                .repair_pointer(&node, &document, &read)
                .unwrap()
                .agent_stamp
                .unwrap();
        } else {
            vfs.remove_file(
                &store
                    .root
                    .join(BLOB_TREE_DIR)
                    .join("p0")
                    .join(reference.relative_path()),
            )
            .unwrap();
            match mode {
                0 => {
                    store
                        .repair_blob(&reference, b"repair-me", &[0], &[])
                        .unwrap()
                        .agent_stamp
                        .unwrap();
                }
                2 => {
                    store.scrub_content().unwrap().agent_stamp.unwrap();
                }
                _ => {
                    store.note_damaged_blob(reference);
                    store.repair_reported_damage().unwrap().agent_stamp.unwrap();
                }
            }
        }
        assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "B");
    }
}

#[test]
fn maintenance_stamps_work_but_not_empty_runs() {
    for mode in 0..4 {
        let (_, vfs, store) = fixture(FaultPolicy::new());
        write(&store, false, b"settled")
            .unwrap()
            .agent_stamp
            .unwrap();
        let store = reopen(&vfs, store, "B");
        store.quiesce().unwrap().agent_stamp.unwrap();
        store.resolve().unwrap().agent_stamp.unwrap();
        store.recover().unwrap().agent_stamp.unwrap();
        assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "A");
        let swaps = SwapSet::single(Swap::new(
            NodePath::parse("docs").unwrap(),
            DocumentName::new("one").unwrap(),
            DocumentBody::new(b"work".to_vec()),
        ));
        store
            .apply_transaction_as(&swaps, &Nonce::new("raw").unwrap())
            .unwrap();
        match mode {
            0 => {
                store.quiesce().unwrap().agent_stamp.unwrap();
            }
            1 => {
                store.resolve().unwrap().agent_stamp.unwrap();
            }
            2 => {
                store.recover().unwrap().agent_stamp.unwrap();
            }
            _ => {
                let mut run = store.recovery().unwrap();
                while !run.progress().finished() {
                    run.step().unwrap().agent_stamp.unwrap();
                }
            }
        }
        assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "B");
    }
}

#[test]
fn a_failed_stamp_keeps_the_successful_operation_result() {
    let outcome = WriteOutcome::new(42, Err(StoreError::NoNonceSource));
    let (value, stamp) = outcome.into_parts();
    assert_eq!(value, 42);
    assert!(matches!(stamp, Err(StoreError::NoNonceSource)));
}

#[test]
fn a_cache_from_another_generation_is_reloaded_before_comparison() {
    let (_, vfs, store) = fixture(FaultPolicy::new());
    let store = reopen(&vfs, store, "B");
    let _hold = store.exclusive();
    let acquired = store.coordination.acquired().unwrap();
    let mut stale = store.genesis.clone();
    stale.metadata.agent_version = "B".into();
    *store.surface.stamp_cache.lock().unwrap() = Some((acquired.found(), stale));
    write(&store, true, b"work").unwrap().agent_stamp.unwrap();
    assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "B");
    assert_eq!(
        store
            .surface
            .stamp_cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .0,
        acquired.advanced_to()
    );
}

#[test]
fn stamping_preserves_other_metadata_changed_under_the_same_exclusion() {
    let (_, vfs, store) = fixture(FaultPolicy::new());
    let store = reopen(&vfs, store, "B");
    let mut current = metadata(&vfs, &store);
    current["metadata"]["agent_description"] = "updated by management".into();
    {
        let _guard = store.exclusive();
        Store::publish_durably(
            &vfs,
            &store.root,
            MARKER_NAME,
            &serde_json::to_vec(&current).unwrap(),
        )
        .unwrap();
    }
    write(&store, false, b"work").unwrap().agent_stamp.unwrap();
    current["metadata"]["agent_version"] = "B".into();
    assert_eq!(metadata(&vfs, &store), current);
}

#[test]
fn maintenance_that_settles_a_pending_publication_is_not_a_noop() {
    for mode in 0..3 {
        let gate = FaultGate::closed();
        let policy = FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(NAMESPACE_TREE_DIR),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        );
        let (_, vfs, store) = fixture(policy);
        write(&store, true, b"before").unwrap().agent_stamp.unwrap();
        let store = reopen(&vfs, store, "B");
        gate.open();
        assert!(write(&store, false, b"after").is_err());
        assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "A");
        match mode {
            0 => {
                store.quiesce().unwrap().agent_stamp.unwrap();
            }
            1 => {
                store.scrub_content().unwrap().agent_stamp.unwrap();
            }
            _ => {
                store.repair_reported_damage().unwrap().agent_stamp.unwrap();
            }
        }
        assert_eq!(metadata(&vfs, &store)["metadata"]["agent_version"], "B");
        assert_eq!(
            crate::read_once(
                &store,
                &NodePath::parse("docs").unwrap(),
                &DocumentName::new("one").unwrap()
            )
            .unwrap()
            .unwrap()
            .as_bytes(),
            b"after"
        );
    }
}
