// Copyright (c) Michael Grier.

use super::*;
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, When};

fn options(schema: u32) -> StoreOptions {
    StoreOptions::without_nonce_source(Agent::new(
        "schema-tests",
        format!("v{schema}"),
        "schema test",
        schema,
    ))
}

fn fixture(schema: u32) -> (Arc<dyn Vfs>, Store) {
    fixture_with(schema, FaultPolicy::new())
}

fn fixture_with(schema: u32, policy: FaultPolicy) -> (Arc<dyn Vfs>, Store) {
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let root = PathBuf::from("schema-store");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        Arc::clone(&vfs),
        root,
        GenesisConfig::new(StoreId::new("schema-tests").unwrap(), 0),
        options(schema),
    )
    .unwrap();
    (vfs, store)
}

fn reader(vfs: &Arc<dyn Vfs>, store: &Store, schema: u32) -> Result<Store, StoreError> {
    Store::open_read_only(Arc::clone(vfs), store.root.clone(), options(schema))
}

fn conversion(store: &Store, source: u32, target: u32, floor: u32) -> SchemaUpgrade {
    store
        .drain_upgrade()
        .unwrap()
        .begin_conversion(UpgradeRequest::new(SchemaLayer::Agent, source, target, floor).unwrap())
        .unwrap()
}

#[test]
fn a_schema_only_conversion_preserves_the_reader_floor_and_requires_reattachment() {
    let vfs: Arc<dyn Vfs> = Arc::new(acadia::MemFs::new());
    let root = PathBuf::from("schema-upgrade");
    vfs.create_dir_all(&root).unwrap();
    let options =
        || StoreOptions::without_nonce_source(Agent::new("schema-tests", "v1", "schema test", 1));
    let store = Store::init(
        vfs,
        root,
        GenesisConfig::new(StoreId::new("schema-tests").unwrap(), 0),
        options(),
    )
    .unwrap();
    let drain = store.drain_upgrade().unwrap();
    let upgrade = drain
        .begin_conversion(UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap())
        .unwrap();
    assert!(upgrade.poll().unwrap());
    upgrade.complete().unwrap().agent_stamp.unwrap();
    assert!(matches!(
        store.properties(),
        Err(StoreError::AttachmentDetached { .. })
    ));
    store
        .reattach(
            StoreOptions::without_nonce_source(Agent::new("schema-tests", "v2", "schema test", 2)),
            Access::Write,
        )
        .unwrap();
    assert_eq!(
        store.properties().unwrap().agent_schema,
        SchemaVersions {
            current: 2,
            oldest_reader: 1
        }
    );
}

#[test]
fn cancelled_conversion_is_detached_and_rechecks_ordinary_admission() {
    let (vfs, store) = fixture(1);
    let old = reader(&vfs, &store, 1).unwrap();
    let upgrade = conversion(&store, 1, 2, 2);
    assert!(!upgrade.poll().unwrap());
    assert!(matches!(
        store.properties(),
        Err(StoreError::AttachmentDetached { .. })
    ));
    assert!(store.reattach(options(1), Access::Write).is_err());
    upgrade.cancel().unwrap();
    assert!(matches!(
        store.read_document(
            NodePath::parse("docs").unwrap(),
            &DocumentName::new("one").unwrap()
        ),
        Err(StoreError::AttachmentDetached { .. })
    ));
    store.reattach(options(1), Access::Write).unwrap();
    assert_eq!(store.properties().unwrap().agent_schema.current, 1);
    drop(old);
}

#[test]
fn source_and_target_readers_attach_during_a_safe_upgrade() {
    let (vfs, store) = fixture(1);
    let upgrade = conversion(&store, 1, 2, 1);
    assert!(upgrade.poll().unwrap());
    let source = reader(&vfs, &store, 1).unwrap();
    let target = reader(&vfs, &store, 2).unwrap();
    assert!(reader(&vfs, &store, 3).is_err());
    assert_eq!(
        target.properties().unwrap().schema_upgrade.unwrap().target,
        2
    );
    assert!(upgrade.registration_error().is_none());
    let diagnostic = DiagnosticView::open(Arc::clone(&vfs), store.root.clone()).unwrap();
    assert!(
        diagnostic
            .attached_handles()
            .unwrap()
            .live
            .iter()
            .any(|entry| entry.tiers.writer == Some(acadia::coord::TierHold::Exclusive))
    );
    upgrade.complete().unwrap().agent_stamp.unwrap();
    assert_eq!(
        source.properties().unwrap().agent_schema,
        SchemaVersions {
            current: 2,
            oldest_reader: 1
        }
    );
    assert!(target.properties().unwrap().schema_upgrade.is_none());
}

#[test]
fn consecutive_safe_upgrades_keep_the_first_sources_reader_floor() {
    let (vfs, store) = fixture(1);
    let oldest = reader(&vfs, &store, 1).unwrap();
    for target in 2..=13 {
        let upgrade = conversion(&store, target - 1, target, 1);
        assert!(upgrade.poll().unwrap());
        let newest = reader(&vfs, &store, target).unwrap();
        upgrade.complete().unwrap().agent_stamp.unwrap();
        assert_eq!(oldest.properties().unwrap().agent_schema.oldest_reader, 1);
        store.reattach(options(target), Access::Write).unwrap();
        drop(newest);
    }
}

#[test]
fn safety_is_judged_against_the_oldest_reader_not_the_current_schema() {
    let (vfs, store) = fixture(1);
    let oldest = reader(&vfs, &store, 1).unwrap();
    let first = conversion(&store, 1, 2, 1);
    assert!(first.poll().unwrap());
    first.complete().unwrap().agent_stamp.unwrap();
    store.reattach(options(2), Access::Write).unwrap();
    let upgrade = conversion(&store, 2, 3, 2);
    assert!(!upgrade.poll().unwrap());
    drop(oldest);
    assert!(upgrade.poll().unwrap());
    upgrade.complete().unwrap().agent_stamp.unwrap();
    assert!(matches!(
        reader(&vfs, &store, 1),
        Err(StoreError::SchemaMismatch { .. })
    ));
    assert!(matches!(
        reader(&vfs, &store, 2),
        Err(StoreError::SchemaMismatch { .. })
    ));
    assert_eq!(
        reader(&vfs, &store, 3)
            .unwrap()
            .properties()
            .unwrap()
            .agent_schema,
        SchemaVersions {
            current: 3,
            oldest_reader: 3
        }
    );
}

#[test]
fn an_agent_upgrade_does_not_apply_upgrade_admission_to_the_canopy_layer() {
    let source = Agent::new("schema-tests", "v1", "schema test", 1);
    let target = Agent::new("schema-tests", "v2", "schema test", 2);
    let mut metadata = agent::AgentMetadata::new(&source);
    metadata.canopy_schema = SchemaVersions {
        current: 3,
        oldest_reader: 2,
    };
    metadata.transition = Some(Transition {
        request: UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap(),
        safe: true,
    });
    assert!(matches!(
        metadata.check_admission(&target, Access::Read),
        Err(StoreError::SchemaMismatch {
            layer: SchemaLayer::Canopy,
            ..
        })
    ));
    metadata.canopy_schema.oldest_reader = 1;
    metadata.check_admission(&target, Access::Read).unwrap();
}

#[test]
fn a_canopy_upgrade_preserves_the_other_layers_admission_and_floor() {
    let (vfs, store) = fixture(7);
    let request = UpgradeRequest::new(SchemaLayer::Canopy, 1, 2, 1).unwrap();
    let upgrade = store
        .drain_upgrade()
        .unwrap()
        .begin_conversion(request)
        .unwrap();
    assert!(upgrade.poll().unwrap());
    assert!(reader(&vfs, &store, 7).is_ok());
    assert!(matches!(
        reader(&vfs, &store, 8),
        Err(StoreError::SchemaMismatch {
            layer: SchemaLayer::Agent,
            ..
        })
    ));
    upgrade.complete().unwrap().agent_stamp.unwrap();
    store.reattach(options(7), Access::Read).unwrap();
    let properties = store.properties().unwrap();
    assert_eq!(
        properties.canopy_schema,
        SchemaVersions {
            current: 2,
            oldest_reader: 1
        }
    );
    assert_eq!(
        properties.agent_schema,
        SchemaVersions {
            current: 7,
            oldest_reader: 7
        }
    );
}

#[test]
fn a_changed_source_is_refused_under_the_gate_without_advancing_a_schema() {
    let (vfs, store) = fixture(1);
    let upgrade = conversion(&store, 0, 2, 1);
    assert!(matches!(
        upgrade.poll(),
        Err(StoreError::UpgradeChanged { .. })
    ));
    store.reattach(options(1), Access::Write).unwrap();
    assert_eq!(store.properties().unwrap().agent_schema.current, 1);
    assert!(vfs.read(&store.root.join(MARKER_NAME)).is_ok());
}

#[test]
fn a_waiting_conversion_preserves_metadata_changed_by_the_last_writer() {
    let (vfs, store) = fixture(1);
    let (last, _) = Store::open(Arc::clone(&vfs), store.root.clone(), options(1)).unwrap();
    let upgrade = conversion(&store, 1, 2, 1);
    assert!(!upgrade.poll().unwrap());
    last.set_agent_description("preserved during conversion")
        .unwrap()
        .agent_stamp
        .unwrap();
    drop(last);
    assert!(upgrade.poll().unwrap());
    upgrade.complete().unwrap().agent_stamp.unwrap();
    store.reattach(options(2), Access::Write).unwrap();
    assert_eq!(
        store.properties().unwrap().agent_description,
        "preserved during conversion"
    );
}

#[test]
fn a_cancelled_published_announcement_stays_recoverable_for_admitted_target_readers() {
    let (vfs, store) = fixture(1);
    let upgrade = conversion(&store, 1, 2, 1);
    assert!(upgrade.poll().unwrap());
    let target = reader(&vfs, &store, 2).unwrap();
    upgrade.cancel().unwrap();
    assert!(matches!(
        Store::open(Arc::clone(&vfs), store.root.clone(), options(1)),
        Err(StoreError::SchemaMismatch { .. })
    ));
    let (recovery, need) = Store::open(vfs, store.root.clone(), options(2)).unwrap();
    assert!(need.required());
    recovery.recover().unwrap().agent_stamp.unwrap();
    assert_eq!(target.properties().unwrap().agent_schema.current, 2);
    assert!(!target.recovery_need().unwrap().required());
}

#[test]
fn publication_failure_releases_tiers_and_keeps_the_source_detached() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Write,
        PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let (_, store) = fixture_with(1, policy);
    let upgrade = conversion(&store, 1, 2, 1);
    gate.open();
    assert!(upgrade.poll().is_err());
    gate.close();
    assert!(matches!(
        store.properties(),
        Err(StoreError::AttachmentDetached { .. })
    ));
    store.reattach(options(1), Access::Write).unwrap();
    store.recover().unwrap().agent_stamp.unwrap();
    assert_eq!(store.properties().unwrap().agent_schema.current, 1);
}

#[test]
fn invalid_requests_refuse_without_detaching_the_drain() {
    let (_, store) = fixture(1);
    for (source, target, floor) in [(1, 1, 1), (2, 1, 1), (0, 1, 2)] {
        assert!(UpgradeRequest::new(SchemaLayer::Agent, source, target, floor).is_err());
    }
    let drain = store.drain_upgrade().unwrap();
    let request = UpgradeRequest {
        layer: SchemaLayer::Agent,
        source: 1,
        target: 1,
        oldest_reader: 1,
    };
    assert!(drain.begin_conversion(request).is_err());
    drain.cancel().unwrap();
    assert_eq!(store.properties().unwrap().agent_schema.current, 1);
}

#[test]
fn conversion_cannot_start_with_operations_in_flight() {
    let (_, store) = fixture(1);
    let (started, entered) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let worker = store.clone();
    let thread = std::thread::spawn(move || {
        worker
            .snapshot(|_| {
                started.send(()).unwrap();
                released.recv().unwrap();
            })
            .unwrap()
    });
    entered.recv().unwrap();
    let drain = store.drain_upgrade().unwrap();
    assert!(matches!(
        drain.begin_conversion(UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap()),
        Err(StoreError::UpgradeOperationsInFlight { .. })
    ));
    release.send(()).unwrap();
    thread.join().unwrap();
    let upgrade = drain
        .begin_conversion(UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap())
        .unwrap();
    upgrade.cancel().unwrap();
    store.reattach(options(1), Access::Write).unwrap();
}

#[test]
fn a_diagnostic_exclusion_holder_keeps_conversion_pending_without_blocking_cancellation() {
    use acadia::coord::{CoordinationFactory, Waiting};
    let (vfs, store) = fixture(1);
    let participant = acadia::coord::file::FileFactory::new()
        .open(vfs, &store.root)
        .unwrap();
    let hold = participant.acquire(Waiting::Never).unwrap();
    let upgrade = conversion(&store, 1, 2, 1);
    assert!(!upgrade.poll().unwrap());
    upgrade.cancel().unwrap();
    drop(hold);
    store.reattach(options(1), Access::Write).unwrap();
    assert_eq!(store.properties().unwrap().agent_schema.current, 1);
}

#[test]
fn schema_boundaries_include_zero_and_the_largest_target() {
    for source in [0, u32::MAX - 1] {
        let (_, store) = fixture(source);
        let target = source + 1;
        let upgrade = conversion(&store, source, target, source);
        assert!(upgrade.poll().unwrap());
        upgrade.complete().unwrap().agent_stamp.unwrap();
        store.reattach(options(target), Access::Write).unwrap();
        assert_eq!(
            store.properties().unwrap().agent_schema,
            SchemaVersions {
                current: target,
                oldest_reader: source
            }
        );
    }
}

#[test]
fn a_failed_agent_stamp_does_not_repeat_or_fence_a_successful_schema_advance() {
    let armed = FaultGate::closed();
    let failed = FaultGate::closed();
    let enable_failure = failed.clone();
    let policy = FaultPolicy::new()
        .inject_gated(
            FaultOp::Fsync,
            PathMatcher::exact("schema-store"),
            Fault::observe_with(move |_, _| enable_failure.open()),
            When::Once,
            armed.clone(),
        )
        .inject_gated(
            FaultOp::Write,
            PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
            Fault::io_error(),
            When::Once,
            failed,
        );
    let (vfs, store) = fixture_with(1, policy);
    let root = store.root.clone();
    drop(store);
    let (store, _) = Store::open(
        Arc::clone(&vfs),
        root,
        StoreOptions::without_nonce_source(Agent::new(
            "schema-tests",
            "upgrader",
            "schema test",
            1,
        )),
    )
    .unwrap();
    let upgrade = conversion(&store, 1, 2, 1);
    assert!(upgrade.poll().unwrap());
    armed.open();
    let outcome = upgrade.complete().unwrap();
    assert!(outcome.agent_stamp.is_err());
    let diagnostic = DiagnosticView::open(vfs, store.root.clone()).unwrap();
    assert_eq!(diagnostic.properties().unwrap().agent_schema.current, 2);
    assert!(!diagnostic.recovery_need().unwrap().required());
    assert!(upgrade.complete().is_err());
}
