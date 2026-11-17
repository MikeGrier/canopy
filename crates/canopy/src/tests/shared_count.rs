// Copyright (c) Michael Grier.

//! The modification count is the coordination state's (`D335`): a handle opened only to read
//! samples the count a writer raises, and a write in progress, a transaction that ended unresolved
//! and state no writer has vouched for all read to it as contention.

use super::*;
use crate::StoreOptions;
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

fn open_reader(vfs: &Arc<dyn Vfs>, root: &std::path::Path) -> Store {
    open_reader_with_timeout(vfs, root, crate::DEFAULT_SNAPSHOT_TIMEOUT)
}

fn open_reader_with_timeout(
    vfs: &Arc<dyn Vfs>,
    root: &std::path::Path,
    timeout: Duration,
) -> Store {
    Store::open_read_only(
        Arc::clone(vfs),
        root.to_path_buf(),
        StoreOptions::without_nonce_source(test_agent()).with_snapshot_timeout(timeout),
    )
    .expect("open the store to read it")
}

fn write(store: &Store, node: &str, document: &str, bytes: &[u8], claim: &str) {
    store
        .write_document_replicated_as(name(node), &doc(document), &body(bytes), &nonce(claim))
        .expect("write");
}

/// A writer, and a reader of the same root that samples its count at every namespace rename the
/// writer makes while the returned gate is open.
fn watched(root: &str) -> (Store, Store, FaultGate, Arc<Mutex<Vec<u64>>>) {
    let samples: Arc<Mutex<Vec<u64>>> = Arc::default();
    let reader: Arc<OnceLock<Store>> = Arc::default();
    let gate = FaultGate::closed();
    let (recorded, watcher) = (Arc::clone(&samples), Arc::clone(&reader));
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
                        .push(reader.modifications().unwrap());
                }
            }),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from(root);
    vfs.create_dir_all(&root).unwrap();
    let writer = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    let reading = open_reader(&vfs, &root);
    let _ = reader.set(reading.clone());
    (writer, reading, gate, samples)
}

#[test]
fn a_reader_opens_while_a_writer_holds_the_root() {
    let (vfs, writer) = fresh_store();
    let reader = open_reader(&vfs, writer.root());

    assert_eq!(reader.store_id(), writer.store_id());
    let second = Store::open_unsourced(Arc::clone(&vfs), writer.root().to_path_buf()).unwrap();
    assert_eq!(second.store_id(), writer.store_id());
}

#[test]
fn several_readers_open_beside_one_writer() {
    let (vfs, writer) = fresh_store();
    let readers: Vec<Store> = (0..4).map(|_| open_reader(&vfs, writer.root())).collect();
    write(&writer, "alpha", "one", b"shared", "w1");
    for reader in &readers {
        assert_eq!(
            reader.read_body(name("alpha"), &doc("one")).unwrap(),
            Some(body(b"shared"))
        );
    }
}

#[test]
fn a_reader_reads_every_document_a_writer_wrote() {
    let (vfs, writer) = fresh_store();
    let reader = open_reader(&vfs, writer.root());
    for i in 0..12 {
        write(
            &writer,
            &format!("node{i}"),
            "d.json",
            format!("v{i}").as_bytes(),
            &format!("w{i}"),
        );
    }
    for i in 0..12 {
        assert_eq!(
            reader
                .read_body(name(&format!("node{i}")), &doc("d.json"))
                .unwrap(),
            Some(body(format!("v{i}").as_bytes()))
        );
    }
    assert_eq!(
        reader.read_body(name("absent"), &doc("d.json")).unwrap(),
        None
    );
}

#[test]
fn a_reader_reads_a_later_version_than_the_one_it_read_before() {
    let (vfs, writer) = fresh_store();
    let reader = open_reader(&vfs, writer.root());
    write(&writer, "alpha", "one", b"first", "w1");
    assert_eq!(
        reader.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(b"first"))
    );
    write(&writer, "alpha", "one", b"second", "w2");
    assert_eq!(
        reader.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(b"second"))
    );
}

#[test]
fn a_reader_samples_the_count_the_writer_raises() {
    let (vfs, writer) = fresh_store();
    let reader = open_reader(&vfs, writer.root());
    let start = reader.modifications().unwrap();
    assert_eq!(start, writer.modifications().unwrap());
    for i in 0..10 {
        write(&writer, "alpha", "one", b"v", &format!("s{i}"));
        let sampled = reader.modifications().unwrap();
        assert_eq!(sampled, start + 2 * (i + 1));
        assert_eq!(sampled, writer.modifications().unwrap());
        assert!(Store::undisturbed(sampled, reader.modifications().unwrap()));
    }
}

#[test]
fn a_reader_samples_odd_throughout_one_write() {
    let (writer, reader, gate, samples) = watched("odd-throughout");
    let before = reader.modifications().unwrap();
    assert!(Store::undisturbed(before, before));

    gate.open();
    write(&writer, "alpha", "one", b"during", "w1");
    gate.close();

    let samples = samples.lock().unwrap().clone();
    assert_eq!(samples.len(), writer.pointer_replica_count());
    for mid in &samples {
        assert_eq!(*mid, before + 1, "the whole write is one odd span");
        assert!(!Store::undisturbed(*mid, *mid));
    }
    assert_eq!(reader.modifications().unwrap(), before + 2);
}

#[test]
fn a_snapshot_a_reader_takes_once_the_write_settles_succeeds() {
    let (vfs, writer) = fresh_store();
    let reader = open_reader(&vfs, writer.root());
    write(&writer, "alpha", "one", b"settled", "w1");
    let seen = reader
        .snapshot(|view| view.read_document(name("alpha"), &doc("one")).unwrap())
        .expect("a settled store yields a snapshot");
    assert!(matches!(seen, crate::Read::Found(_)));
}

#[test]
fn a_snapshot_taken_inside_a_write_is_contended() {
    let reader_cell: Arc<OnceLock<Store>> = Arc::default();
    let verdict: Arc<Mutex<Option<bool>>> = Arc::default();
    let gate = FaultGate::closed();
    let (watcher, outcome) = (Arc::clone(&reader_cell), Arc::clone(&verdict));
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(NAMESPACE_TREE_DIR),
            Fault::observe_with(move |_, _| {
                if let Some(reader) = watcher.get() {
                    let contended = matches!(
                        reader.snapshot(|_| ()),
                        Err(StoreError::SnapshotContended { .. })
                    );
                    outcome.lock().unwrap().get_or_insert(contended);
                }
            }),
            When::Once,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("snapshot-inside");
    vfs.create_dir_all(&root).unwrap();
    let writer = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    let _ = reader_cell.set(open_reader_with_timeout(
        &vfs,
        &root,
        Duration::from_millis(25),
    ));

    gate.open();
    write(&writer, "alpha", "one", b"x", "w1");
    gate.close();

    assert_eq!(*verdict.lock().unwrap(), Some(true));
    assert!(reader_cell.get().unwrap().snapshot(|_| ()).is_ok());
}

#[test]
fn every_write_verb_is_refused_to_a_reader_and_changes_nothing() {
    let (vfs, writer) = fresh_store();
    write(&writer, "alpha", "one", b"kept", "w1");
    let mut minted = 0;
    let reader = Store::open_read_only(
        Arc::clone(&vfs),
        writer.root().to_path_buf(),
        StoreOptions::with_nonce_source(test_agent(), move || {
            minted += 1;
            nonce(&format!("mint{minted}"))
        }),
    )
    .unwrap();
    let count = reader.modifications().unwrap();
    let swap = || Swap::new(name("alpha"), doc("one"), body(b"changed"));
    let pointer = writer.read_pointer(&name("alpha"), &doc("one")).unwrap();
    let reference = pointer.pointer().target();
    let refused: Vec<(&str, Result<(), StoreError>)> = vec![
        (
            "write_document_replicated",
            reader
                .write_document_replicated(name("alpha"), &doc("one"), &body(b"changed"))
                .map(|_| ()),
        ),
        (
            "write_document_replicated_as",
            reader.write_document_replicated_as(
                name("alpha"),
                &doc("one"),
                &body(b"changed"),
                &nonce("as"),
            ),
        ),
        (
            "update_document",
            reader.update_document(swap()).map(|_| ()),
        ),
        (
            "apply_transaction",
            reader
                .apply_transaction(&SwapSet::single(swap()))
                .map(|_| ()),
        ),
        (
            "apply_transaction_as",
            reader
                .apply_transaction_as(&SwapSet::single(swap()), &nonce("txn"))
                .map(|_| ()),
        ),
        (
            "transform",
            transform_document(&reader, name("alpha"), &doc("one"), |_| body(b"changed"))
                .map(|_| ()),
        ),
        ("quiesce", reader.quiesce().map(|outcome| outcome.value)),
        ("scrub_content", reader.scrub_content().map(|_| ())),
        (
            "repair_reported_damage",
            reader.repair_reported_damage().map(|_| ()),
        ),
        (
            "repair_blob",
            reader
                .repair_blob(&reference, b"kept", &[], &[])
                .map(|_| ()),
        ),
        (
            "repair_pointer",
            reader
                .repair_pointer(&name("alpha"), &doc("one"), &pointer)
                .map(|_| ()),
        ),
    ];
    for (verb, result) in refused {
        assert!(
            matches!(result, Err(StoreError::ReadOnly { .. })),
            "{verb} must be refused to a reader: {result:?}"
        );
    }

    assert_eq!(reader.modifications().unwrap(), count);
    assert_eq!(
        writer.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(b"kept"))
    );
    assert!(!reader.recovery_required().unwrap());
}

#[test]
fn a_reader_does_not_recover_what_an_unresolved_transaction_left() {
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
    let root = PathBuf::from("unresolved-reader");
    vfs.create_dir_all(&root).unwrap();
    let writer = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    let swaps = SwapSet::new([Swap::new(name("alpha"), doc("pin"), body(b"committed"))]).unwrap();
    gate.open();
    assert!(writer.apply_transaction_as(&swaps, &nonce("u1")).is_err());
    gate.close();
    let log_before = writer.commit_log().read().unwrap().len();
    assert!(log_before > 0, "the transaction left its log behind");

    let reader = open_reader_with_timeout(&vfs, &root, Duration::from_millis(25));
    let odd = reader.modifications().unwrap();
    assert!(
        !Store::undisturbed(odd, odd),
        "an unresolved transaction reads odd: {odd}"
    );
    assert_eq!(reader.commit_log().read().unwrap().len(), log_before);
    assert!(matches!(
        reader.snapshot(|_| ()),
        Err(StoreError::SnapshotContended { .. })
    ));

    // The next writer's open leaves it odd; resolving is what settles the count the reader samples.
    drop(writer);
    let recovered = Store::open_unsourced(Arc::clone(&vfs), root.clone()).unwrap();
    let opened = reader.modifications().unwrap();
    assert!(!Store::undisturbed(opened, opened), "opened: {opened}");
    recovered
        .resolve()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    let settled = reader.modifications().unwrap();
    assert!(Store::undisturbed(settled, settled), "recovered: {settled}");
    assert!(settled > odd);
    assert!(reader.snapshot(|_| ()).is_ok());
    write(&recovered, "alpha", "one", b"after recovery", "w9");
}

#[test]
fn a_root_whose_coordination_state_is_lost_reads_odd_until_a_writer_recovers() {
    let (vfs, writer) = fresh_store();
    write(&writer, "alpha", "one", b"kept", "w1");
    let root = writer.root().to_path_buf();
    drop(writer);
    vfs.remove_dir_all(&root.join(".coord")).unwrap();

    let reader = open_reader(&vfs, &root);
    let lost = reader.modifications().unwrap();
    assert!(
        !Store::undisturbed(lost, lost),
        "unvouched state reads odd: {lost}"
    );

    let writer = Store::open_unsourced(Arc::clone(&vfs), root).unwrap();
    assert!(
        writer.recovery_required().unwrap(),
        "an open does not vouch for state nothing has"
    );
    let still = reader.modifications().unwrap();
    assert!(
        !Store::undisturbed(still, still),
        "still unvouched: {still}"
    );
    writer
        .recover()
        .unwrap()
        .agent_stamp
        .expect("agent version stamp");
    let vouched = reader.modifications().unwrap();
    assert!(Store::undisturbed(vouched, vouched), "vouched: {vouched}");
    assert_eq!(
        writer.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(b"kept"))
    );
}

#[test]
fn the_count_keeps_its_value_across_opens() {
    let (vfs, writer) = fresh_store();
    write(&writer, "alpha", "one", b"v", "w1");
    let before = writer.modifications().unwrap();
    let reopened = reopen(&vfs, writer).unwrap();
    let after = reopened.modifications().unwrap();
    assert_eq!(after, before);
    write(&reopened, "alpha", "one", b"v2", "w2");
    assert_eq!(reopened.modifications().unwrap(), before + 2);
}

#[test]
fn a_clone_of_a_writer_raises_the_same_count() {
    let (vfs, writer) = fresh_store();
    let reader = open_reader(&vfs, writer.root());
    let clone = writer.clone();
    write(&clone, "alpha", "one", b"v", "w1");
    assert_eq!(
        writer.modifications().unwrap(),
        clone.modifications().unwrap()
    );
    assert_eq!(
        reader.modifications().unwrap(),
        writer.modifications().unwrap()
    );
}

#[test]
fn a_reader_is_refused_a_directory_that_is_not_a_store_and_leaves_nothing_in_it() {
    let (vfs, root) = fresh_root();
    let refused = Store::open_read_only(
        Arc::clone(&vfs),
        root.clone(),
        StoreOptions::without_nonce_source(test_agent()),
    );
    assert!(matches!(refused, Err(StoreError::NotAStore { .. })));
    assert!(vfs.read_dir(&root).unwrap().is_empty());
}

#[test]
fn a_root_no_creation_made_is_refused_to_a_writer_and_to_a_reader() {
    let (vfs, root) = fresh_root();
    let marker = format!(
        "{{\"format_version\":{},\"blob_replica_count\":3,\"pointer_replica_count\":3,\
         \"shard_width\":2,\"store_id\":\"x\",\"created_at_unix_ms\":0}}",
        crate::GENESIS_FORMAT_VERSION
    );
    vfs.write(
        &root.join(MARKER_NAME),
        &marker_with_agent_metadata(marker.as_bytes()),
    )
    .unwrap();
    for index in 0..3 {
        vfs.create_dir_all(
            &root
                .join(crate::store::BLOB_TREE_DIR)
                .join(format!("p{index}")),
        )
        .unwrap();
        vfs.create_dir_all(&namespace_root_of(&root, index))
            .unwrap();
    }

    assert!(matches!(
        Store::open_unsourced(Arc::clone(&vfs), root.clone()),
        Err(StoreError::Coordination(
            acadia::coord::CoordError::NotCreated
        ))
    ));
    assert!(matches!(
        Store::open_read_only(vfs, root, StoreOptions::without_nonce_source(test_agent())),
        Err(StoreError::Coordination(
            acadia::coord::CoordError::NotCreated
        ))
    ));
}

fn namespace_root_of(root: &std::path::Path, index: usize) -> PathBuf {
    root.join(NAMESPACE_TREE_DIR).join(format!("p{index}"))
}

#[test]
fn a_reader_is_refused_a_root_that_has_not_been_migrated() {
    let (vfs, root) = fresh_root();
    let legacy = format!(
        "{{\"format_version\":{},\"blob_replica_count\":3,\"pointer_replica_count\":3,\
         \"shard_width\":2,\"shire_id\":\"x\",\"created_at_unix_ms\":0}}",
        crate::MIGRATABLE_FORMAT_VERSION
    );
    vfs.write(&root.join(".shire"), legacy.as_bytes()).unwrap();
    assert!(matches!(
        Store::open_read_only(vfs, root, StoreOptions::without_nonce_source(test_agent())),
        Err(StoreError::MigrationRequired { .. })
    ));
}

/// A writer whose opening raise cannot be published makes no effect and leaves the count as it was.
#[test]
fn a_write_whose_opening_raise_fails_changes_nothing() {
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::file_name_prefix("count.staging"),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("opening-fails");
    vfs.create_dir_all(&root).unwrap();
    let writer = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    let reader = open_reader(&vfs, &root);
    let before = reader.modifications().unwrap();

    gate.open();
    let refused = writer.write_document_replicated_as(
        name("alpha"),
        &doc("one"),
        &body(b"never"),
        &nonce("w1"),
    );
    gate.close();

    assert!(
        matches!(refused, Err(StoreError::Coordination(_))),
        "{refused:?}"
    );
    assert_eq!(reader.modifications().unwrap(), before);
    assert_eq!(reader.read_body(name("alpha"), &doc("one")).unwrap(), None);
    // The failure was one operation's: the next write goes through.
    write(&writer, "alpha", "one", b"after", "w2");
    assert_eq!(
        reader.read_body(name("alpha"), &doc("one")).unwrap(),
        Some(body(b"after"))
    );
}

/// A closing raise that cannot be published leaves the count odd and the handle needing recovery,
/// as an unresolved transaction does.
#[test]
fn a_write_whose_closing_raise_fails_leaves_the_count_odd_until_recovery() {
    let gate = FaultGate::closed();
    let armed = FaultGate::closed();
    let opener = armed.clone();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new()
            .inject_gated(
                FaultOp::Rename,
                PathMatcher::has_component(NAMESPACE_TREE_DIR),
                Fault::observe_with(move |_, _| opener.open()),
                When::Once,
                gate.clone(),
            )
            .inject_gated(
                FaultOp::Rename,
                PathMatcher::file_name_prefix("count.staging"),
                Fault::io_error(),
                When::Always,
                armed.clone(),
            ),
    ));
    let root = PathBuf::from("closing-fails");
    vfs.create_dir_all(&root).unwrap();
    let writer = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    let reader = open_reader(&vfs, &root);
    let before = reader.modifications().unwrap();

    // The first namespace rename opens the gate, so the opening raise is published and the
    // closing one is not.
    gate.open();
    let outcome = writer.write_document_replicated_as(
        name("alpha"),
        &doc("one"),
        &body(b"landed"),
        &nonce("w1"),
    );
    assert!(
        outcome.is_ok(),
        "the write's own result is reported: {outcome:?}"
    );
    gate.close();
    armed.close();

    assert!(writer.recovery_required().unwrap());
    let odd = reader.modifications().unwrap();
    assert_eq!(odd, before + 1);
    assert!(matches!(
        writer.write_document_replicated_as(name("beta"), &doc("two"), &body(b"no"), &nonce("w2")),
        Err(StoreError::RecoveryRequired { .. })
    ));

    drop(writer);
    let recovered = open_resolved(Arc::clone(&vfs), root).unwrap();
    let settled = reader.modifications().unwrap();
    assert!(Store::undisturbed(settled, settled), "recovered: {settled}");
    write(&recovered, "beta", "two", b"yes", "w3");
    assert_eq!(
        reader.read_body(name("beta"), &doc("two")).unwrap(),
        Some(body(b"yes"))
    );
}
