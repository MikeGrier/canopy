// Copyright (c) Michael Grier.

use super::*;
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, When};

fn options(schema: u32, prefix: &str) -> StoreOptions {
    let prefix = prefix.to_owned();
    let mut next = 0;
    StoreOptions::with_nonce_source(
        Agent::new("body-tests", format!("v{schema}"), "body tests", schema),
        move || {
            next += 1;
            Nonce::new(&format!("{prefix}{next}")).unwrap()
        },
    )
}

fn fixture(policy: FaultPolicy) -> (Arc<dyn Vfs>, Store) {
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(MemFs::new(), policy));
    let root = PathBuf::from("body-store");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        Arc::clone(&vfs),
        root,
        GenesisConfig::new(StoreId::new("body-tests").unwrap(), 0),
        options(1, "source"),
    )
    .unwrap();
    (vfs, store)
}

fn start(store: &Store, units: u64) -> SchemaUpgrade {
    let upgrade = store
        .drain_upgrade()
        .unwrap()
        .begin_migration(
            UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap(),
            units,
        )
        .unwrap();
    assert!(upgrade.poll().unwrap());
    upgrade
}

fn write_unit(unit: &MigrationUnit<'_>) -> Result<(), StoreError> {
    let document = DocumentName::new(&format!("unit{}", unit.progress().next_unit)).unwrap();
    let body = DocumentBody::new(format!("value{}", unit.progress().next_unit).into_bytes());
    match unit.read_document(NodePath::parse("docs").unwrap(), &document)? {
        crate::Read::Found(existing) if existing == body => Ok(()),
        _ => unit
            .apply_transaction(&SwapSet::single(Swap::new(
                NodePath::parse("docs").unwrap(),
                document,
                body,
            )))
            .map(|_| ()),
    }
}

fn recover(store: &Store) {
    let mut run = store.recovery().unwrap();
    for _ in 0..100 {
        if run.progress().finished() {
            break;
        }
        if run.progress().phase() == RecoveryPhase::UpgradeBody {
            run.step_upgrade(write_unit).unwrap().agent_stamp.unwrap();
        } else {
            run.step().unwrap().agent_stamp.unwrap();
        }
    }
    assert!(run.progress().finished());
    assert!(!store.recovery_required().unwrap());
}

#[test]
fn unit_cursors_include_empty_and_large_bodies_but_never_past_the_end() {
    let transition = Transition {
        request: UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap(),
        safe: true,
    };
    for units in [0, 1, 2, 3, 4, 7, 8, 12, 64, 128, 1024, u64::MAX] {
        let mut intent = Intent::new(transition.clone(), units);
        assert!(intent.valid());
        intent.next_unit = units;
        assert!(intent.valid());
        if let Some(past_end) = units.checked_add(1) {
            intent.next_unit = past_end;
            assert!(!intent.valid());
        }
    }
}

#[test]
fn malformed_versions_and_nonadvancing_schema_requests_are_refused() {
    let transition = Transition {
        request: UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap(),
        safe: true,
    };
    let mut intent = Intent::new(transition, 1);
    intent.format_version = intent_tags::FORMAT_VERSION + 1;
    assert!(!intent.valid());
    intent.format_version = intent_tags::FORMAT_VERSION;
    intent.transition.request.target = intent.transition.request.source;
    assert!(!intent.valid());
}

#[test]
fn caller_units_preserve_intent_and_exclusion_until_the_final_schema_advance() {
    for units in 0..12 {
        let (vfs, store) = fixture(FaultPolicy::new());
        let upgrade = start(&store, units);
        let diagnostic = DiagnosticView::open(Arc::clone(&vfs), store.root.clone()).unwrap();
        assert!(vfs.exists(&store.root.join(UPGRADE_INTENT_NAME)).unwrap());
        for next in 0..units {
            assert!(upgrade.complete().is_err());
            assert!(!diagnostic.recovery_need().unwrap().fenced());
            let outcome = upgrade.step(write_unit).unwrap();
            outcome.agent_stamp.unwrap();
            assert_eq!(outcome.value.next_unit, next + 1);
            assert_eq!(diagnostic.properties().unwrap().agent_schema.current, 1);
        }
        upgrade.complete().unwrap().agent_stamp.unwrap();
        assert_eq!(upgrade.progress().unwrap().next_unit, units);
        assert_eq!(diagnostic.properties().unwrap().agent_schema.current, 2);
        assert!(!vfs.exists(&store.root.join(UPGRADE_INTENT_NAME)).unwrap());
        assert!(!diagnostic.recovery_need().unwrap().required());
    }
}

#[test]
fn interrupted_bodies_resume_from_every_durable_unit_cursor() {
    for stop in 0..=4 {
        let (vfs, store) = fixture(FaultPolicy::new());
        let upgrade = start(&store, 4);
        for _ in 0..stop {
            upgrade.step(write_unit).unwrap().agent_stamp.unwrap();
        }
        upgrade.cancel().unwrap();
        assert_eq!(Intent::read(&store).unwrap().unwrap().next_unit, stop);
        assert!(matches!(
            Store::open(Arc::clone(&vfs), store.root.clone(), options(1, "old")),
            Err(StoreError::SchemaMismatch { .. })
        ));
        let (resumer, found) =
            Store::open(Arc::clone(&vfs), store.root.clone(), options(2, "resumer")).unwrap();
        assert!(found.required());
        recover(&resumer);
        for unit in 0..4 {
            assert!(matches!(
                resumer
                    .read_document(
                        NodePath::parse("docs").unwrap(),
                        &DocumentName::new(&format!("unit{unit}")).unwrap()
                    )
                    .unwrap(),
                crate::Read::Found(_)
            ));
        }
        assert!(!vfs.exists(&store.root.join(UPGRADE_INTENT_NAME)).unwrap());
    }
}

#[test]
fn a_failed_unit_fences_the_body_and_keeps_its_cursor() {
    let (vfs, store) = fixture(FaultPolicy::new());
    let upgrade = start(&store, 2);
    upgrade.step(write_unit).unwrap().agent_stamp.unwrap();
    assert!(upgrade.step(|_| Err(StoreError::NoNonceSource)).is_err());
    assert_eq!(Intent::read(&store).unwrap().unwrap().next_unit, 1);
    let (resumer, found) = Store::open(vfs, store.root.clone(), options(2, "resume")).unwrap();
    assert!(found.fenced());
    recover(&resumer);
}

#[test]
fn generic_recovery_refuses_missing_units_without_clearing_the_intent() {
    let (vfs, store) = fixture(FaultPolicy::new());
    let upgrade = start(&store, 1);
    upgrade.cancel().unwrap();
    let (resumer, _) = Store::open(vfs, store.root.clone(), options(2, "resume")).unwrap();
    assert!(matches!(
        resumer.recover(),
        Err(StoreError::UpgradeBodyRequired { .. })
    ));
    assert!(Intent::read(&resumer).unwrap().is_some());
    assert!(resumer.recovery_required().unwrap());
    recover(&resumer);
}

#[test]
fn a_failure_after_transaction_prepare_is_settled_before_the_unit_is_replayed() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Rename,
        PathMatcher::has_component("namespace"),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let (vfs, store) = fixture(policy);
    let upgrade = start(&store, 1);
    gate.open();
    assert!(upgrade.step(write_unit).is_err());
    gate.close();
    assert_eq!(Intent::read(&store).unwrap().unwrap().next_unit, 0);
    let (resumer, _) = Store::open(vfs, store.root.clone(), options(2, "resume")).unwrap();
    let mut run = resumer.recovery().unwrap();
    while run.progress().phase() != RecoveryPhase::UpgradeBody {
        run.step().unwrap().agent_stamp.unwrap();
    }
    run.step_upgrade(|unit| {
        assert!(matches!(
            unit.read_document(
                NodePath::parse("docs").unwrap(),
                &DocumentName::new("unit0").unwrap()
            )?,
            crate::Read::Found(_)
        ));
        write_unit(unit)
    })
    .unwrap()
    .agent_stamp
    .unwrap();
    while !run.progress().finished() {
        run.step().unwrap().agent_stamp.unwrap();
    }
    assert!(!resumer.recovery_required().unwrap());
}

#[test]
fn undefined_canopy_bodies_are_refused_before_detaching_or_publishing_intent() {
    let (vfs, store) = fixture(FaultPolicy::new());
    let drain = store.drain_upgrade().unwrap();
    assert!(matches!(
        drain.begin_migration(
            UpgradeRequest::new(SchemaLayer::Canopy, 2, 3, 2).unwrap(),
            1
        ),
        Err(StoreError::InvalidUpgrade { .. })
    ));
    drain.cancel().unwrap();
    assert!(!vfs.exists(&store.root.join(UPGRADE_INTENT_NAME)).unwrap());
    assert_eq!(store.properties().unwrap().canopy_schema.current, 2);
}

#[test]
fn an_agent_format_root_requires_offline_migration_and_preserves_its_metadata() {
    let (vfs, store) = fixture(FaultPolicy::new());
    let root = store.root.clone();
    let mut genesis = Store::read_genesis(&vfs, &root).unwrap();
    genesis.format_version = AGENT_FORMAT_VERSION;
    genesis.metadata.agent_description = "preserved description".into();
    let metadata = genesis.metadata.clone();
    drop(store);
    Store::publish_durably(
        &vfs,
        &root,
        MARKER_NAME,
        &serde_json::to_vec_pretty(&genesis).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        Store::open(Arc::clone(&vfs), root.clone(), options(1, "open")),
        Err(StoreError::MigrationRequired {
            found: AGENT_FORMAT_VERSION,
            ..
        })
    ));
    Store::migrate(Arc::clone(&vfs), root.clone(), &options(1, "migration")).unwrap();
    assert_eq!(Store::read_genesis(&vfs, &root).unwrap().metadata, metadata);
    assert_eq!(
        Store::read_genesis(&vfs, &root).unwrap().format_version,
        GENESIS_FORMAT_VERSION
    );
}

#[test]
fn a_cursor_publication_failure_replays_the_same_idempotent_unit() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Write,
        PathMatcher::file_name(format!("{UPGRADE_INTENT_NAME}.tmp")),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let (vfs, store) = fixture(policy);
    let upgrade = start(&store, 1);
    gate.open();
    assert!(upgrade.step(write_unit).is_err());
    gate.close();
    assert_eq!(Intent::read(&store).unwrap().unwrap().next_unit, 0);
    let (resumer, _) = Store::open(vfs, store.root.clone(), options(2, "resume")).unwrap();
    recover(&resumer);
    assert_eq!(resumer.list_documents().unwrap().len(), 1);
}

#[test]
fn a_failed_initial_intent_write_runs_no_unit_and_can_recover_the_source() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Write,
        PathMatcher::file_name(format!("{UPGRADE_INTENT_NAME}.tmp")),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let (vfs, store) = fixture(policy);
    let upgrade = store
        .drain_upgrade()
        .unwrap()
        .begin_migration(UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap(), 1)
        .unwrap();
    gate.open();
    assert!(upgrade.poll().is_err());
    gate.close();
    assert!(Intent::read(&store).unwrap().is_none());
    let (resumer, _) = Store::open(vfs, store.root.clone(), options(1, "resume")).unwrap();
    resumer.recover().unwrap().agent_stamp.unwrap();
    assert!(resumer.list_documents().unwrap().is_empty());
}

#[test]
fn an_intent_that_survives_schema_publication_is_cleared_without_replaying_units() {
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::RemoveFile,
        PathMatcher::file_name(UPGRADE_INTENT_NAME),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let (vfs, store) = fixture(policy);
    let upgrade = start(&store, 1);
    upgrade.step(write_unit).unwrap().agent_stamp.unwrap();
    gate.open();
    assert!(upgrade.complete().is_err());
    gate.close();
    assert_eq!(
        Store::read_genesis(&vfs, &store.root)
            .unwrap()
            .metadata
            .agent_schema
            .current,
        2
    );
    let (resumer, _) = Store::open(vfs, store.root.clone(), options(2, "resume")).unwrap();
    let mut run = resumer.recovery().unwrap();
    while !run.progress().finished() {
        assert_ne!(run.progress().phase(), RecoveryPhase::UpgradeBody);
        run.step().unwrap().agent_stamp.unwrap();
    }
    assert!(Intent::read(&resumer).unwrap().is_none());
}

#[test]
fn a_panicking_unit_releases_exclusion_and_leaves_recovery_intent() {
    let (vfs, store) = fixture(FaultPolicy::new());
    let upgrade = start(&store, 1);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = upgrade.step(|_| panic!("unit failure"));
        }))
        .is_err()
    );
    let (resumer, found) = Store::open(vfs, store.root.clone(), options(2, "resume")).unwrap();
    assert!(found.fenced());
    recover(&resumer);
}

#[test]
fn a_unit_cannot_recursively_cancel_or_complete_its_own_upgrade() {
    let (vfs, store) = fixture(FaultPolicy::new());
    let upgrade = start(&store, 1);
    upgrade
        .step(|unit| {
            assert!(matches!(
                upgrade.cancel(),
                Err(StoreError::UpgradeFromOperation { .. })
            ));
            assert!(matches!(
                upgrade.complete(),
                Err(StoreError::UpgradeFromOperation { .. })
            ));
            write_unit(unit)
        })
        .unwrap()
        .agent_stamp
        .unwrap();
    upgrade.complete().unwrap().agent_stamp.unwrap();
    assert_eq!(
        DiagnosticView::open(vfs, store.root.clone())
            .unwrap()
            .properties()
            .unwrap()
            .agent_schema
            .current,
        2
    );
}

#[test]
fn another_recovery_reloads_the_durable_cursor_between_units() {
    let (vfs, store) = fixture(FaultPolicy::new());
    let upgrade = start(&store, 3);
    upgrade.cancel().unwrap();
    let (first, _) =
        Store::open(Arc::clone(&vfs), store.root.clone(), options(2, "first")).unwrap();
    let (second, _) = Store::open(vfs, store.root.clone(), options(2, "second")).unwrap();
    let mut left = first.recovery().unwrap();
    let mut right = second.recovery().unwrap();
    for _ in 0..100 {
        for run in [&mut left, &mut right] {
            if run.progress().finished() {
                continue;
            }
            if run.progress().phase() == RecoveryPhase::UpgradeBody {
                let known = run.progress().known();
                let pending = Intent::read(&first).unwrap().is_some();
                run.step_upgrade(write_unit).unwrap().agent_stamp.unwrap();
                if pending {
                    assert!(
                        run.progress().known() <= known,
                        "remeasuring a cursor must not count its old units twice"
                    );
                }
            } else {
                run.step().unwrap().agent_stamp.unwrap();
            }
        }
        if left.progress().finished() && right.progress().finished() {
            break;
        }
    }
    assert!(left.progress().finished() && right.progress().finished());
    assert_eq!(first.list_documents().unwrap().len(), 3);
}

#[test]
fn an_upgrader_that_finds_recovery_required_publishes_no_intent_or_announcement() {
    let (vfs, store) = fixture(FaultPolicy::new());
    {
        let _writer = store.writable().unwrap();
        let _modification = store.begin_modification().unwrap();
        store.writer.mark_unresolved();
    }
    let marker = vfs.read(&store.root.join(MARKER_NAME)).unwrap();
    let upgrade = store
        .drain_upgrade()
        .unwrap()
        .begin_migration(UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap(), 1)
        .unwrap();
    assert!(matches!(
        upgrade.poll(),
        Err(StoreError::RecoveryRequired { .. })
    ));
    assert_eq!(vfs.read(&store.root.join(MARKER_NAME)).unwrap(), marker);
    assert!(Intent::read(&store).unwrap().is_none());
    store
        .reattach(options(1, "recover"), acadia::coord::Access::Write)
        .unwrap();
    store.recover().unwrap().agent_stamp.unwrap();
}

#[test]
fn a_unit_cannot_start_competing_recovery_through_a_captured_store() {
    let (_, store) = fixture(FaultPolicy::new());
    let upgrade = start(&store, 1);
    upgrade
        .step(|unit| {
            assert!(matches!(
                store.recovery(),
                Err(StoreError::UpgradeFromOperation { .. })
            ));
            assert!(matches!(
                store.resolve(),
                Err(StoreError::UpgradeFromOperation { .. })
            ));
            write_unit(unit)
        })
        .unwrap()
        .agent_stamp
        .unwrap();
    upgrade.complete().unwrap().agent_stamp.unwrap();
}
