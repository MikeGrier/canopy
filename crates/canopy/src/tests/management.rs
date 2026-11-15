// Copyright (c) Michael Grier.

use super::*;
use crate::{DiagnosticView, StoreOptions};
use acadia::coord::file::FileFactory;
use acadia::coord::{Access, CoordinationFactory, ReaderSafety, Waiting};
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn properties_and_registry_preserve_agent_strings_and_both_schema_layers() {
    for (index, identifier) in [
        "plain",
        "with space",
        "quote\"",
        "slash\\",
        "line\n",
        "tab\t",
        "",
        "\u{03bb}",
        "trailing ",
        "case-A",
    ]
    .into_iter()
    .enumerate()
    {
        let (vfs, root) = fresh_root();
        let schema = index as u32;
        let options = StoreOptions::without_nonce_source(crate::Agent::new(
            identifier,
            "v\"1",
            "description",
            schema,
        ));
        let config = test_config().with_blob_replica_count(index + 1).unwrap();
        let store = Store::init(Arc::clone(&vfs), root.clone(), config, options).unwrap();
        let properties = store.properties().unwrap();
        assert_eq!(properties.agent_identifier, identifier);
        assert_eq!(properties.agent_version, "v\"1");
        assert_eq!(properties.agent_description, "description");
        assert_eq!(
            properties.coordination_implementation,
            FileFactory::IMPLEMENTATION
        );
        assert_eq!(
            properties.canopy_schema.current,
            crate::CANOPY_SCHEMA_VERSION
        );
        assert_eq!(
            properties.canopy_schema.oldest_reader,
            crate::CANOPY_SCHEMA_VERSION
        );
        assert_eq!(properties.agent_schema.current, schema);
        assert_eq!(properties.agent_schema.oldest_reader, schema);
        assert_eq!(properties.store_id, "test-store");
        assert_eq!(properties.created_at_unix_ms, 12_345);
        assert_eq!(
            properties.genesis_format_version,
            crate::GENESIS_FORMAT_VERSION
        );
        assert_eq!(properties.blob_replica_count, index + 1);
        assert_eq!(
            properties.pointer_replica_count,
            test_config().pointer_replica_count()
        );
        assert_eq!(properties.shard_width, test_config().shard_width());
        let diagnostic = DiagnosticView::open(vfs, root).unwrap();
        let listed = diagnostic.attached_handles().unwrap();
        assert_eq!(listed.live.len(), 1);
        assert_eq!(listed.live[0].agent_identifier, identifier);
        assert_eq!(listed.live[0].agent_version, "v\"1");
        assert_eq!(listed.live[0].agent_schema, schema);
        assert_eq!(listed.live[0].process_id, std::process::id());
        assert!(!listed.live[0].draining);
        assert!(!listed.may_be_incomplete);
        assert_eq!(diagnostic.properties().unwrap(), properties);
    }
}

#[test]
fn diagnosis_opens_and_answers_while_the_admission_gate_is_exclusive() {
    let (vfs, writer) = fresh_store();
    let root = writer.root().to_path_buf();
    drop(writer);
    let participant = FileFactory::new().open(Arc::clone(&vfs), &root).unwrap();
    let held = participant
        .attach(Access::Read, Waiting::Never)
        .unwrap()
        .upgrade(ReaderSafety::Unsafe, Waiting::Never)
        .unwrap();
    let (done, received) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let view = DiagnosticView::open(vfs, root).unwrap();
        let result = (
            view.properties(),
            view.attached_handles(),
            view.recovery_need(),
        );
        done.send(result).unwrap();
    });
    let result = test_timers::mpsc::recv_timeout(&received, Duration::from_secs(2));
    drop(held);
    worker.join().unwrap();
    let (properties, listing, recovery) = result.expect("diagnosis must not wait for admission");
    assert_eq!(properties.unwrap().store_id, "test-store");
    assert!(listing.unwrap().live.is_empty());
    assert!(!recovery.unwrap().required());
}

#[test]
fn diagnostic_handles_take_no_tier_and_do_not_register() {
    let (vfs, writer) = fresh_store();
    let root = writer.root().to_path_buf();
    drop(writer);
    let view = DiagnosticView::open(Arc::clone(&vfs), root.clone()).unwrap();
    assert!(view.attached_handles().unwrap().live.is_empty());
    let participant = FileFactory::new().open(vfs, &root).unwrap();
    let attached = participant.attach(Access::Read, Waiting::Never).unwrap();
    let _held = attached
        .upgrade(ReaderSafety::Unsafe, Waiting::Never)
        .unwrap();
    assert!(view.properties().is_ok());
}

#[test]
fn unknown_payload_versions_and_conflicting_tiers_are_counted_as_unknown() {
    let (vfs, writer) = fresh_store();
    let root = writer.root().to_path_buf();
    let view = DiagnosticView::open(Arc::clone(&vfs), root.clone()).unwrap();
    let participant = FileFactory::new().open(vfs, &root).unwrap();
    let mut attached = participant.attach(Access::Read, Waiting::Never).unwrap();
    let mut valid: serde_json::Value =
        serde_json::from_slice(&participant.list().unwrap().live[0].payload).unwrap();
    valid["writer_tier"] = serde_json::Value::Null;
    for field in [
        "format_version",
        "reader_tier",
        "process_id",
        "agent_schema",
    ] {
        let mut invalid = valid.clone();
        invalid[field] = "invalid".into();
        attached
            .register(&serde_json::to_vec(&invalid).unwrap())
            .unwrap();
        let listed = view.attached_handles().unwrap();
        assert_eq!(listed.live.len(), 1);
        assert_eq!(listed.unknown, 1);
        assert!(listed.may_be_incomplete);
    }
    let mut mismatch = valid.clone();
    mismatch["format_version"] = (valid["format_version"].as_u64().unwrap() + 1).into();
    attached
        .register(&serde_json::to_vec(&mismatch).unwrap())
        .unwrap();
    assert_eq!(view.attached_handles().unwrap().unknown, 1);
    let mut mismatch = valid.clone();
    mismatch["writer_tier"] = valid["reader_tier"].clone();
    attached
        .register(&serde_json::to_vec(&mismatch).unwrap())
        .unwrap();
    assert_eq!(view.attached_handles().unwrap().unknown, 1);
    attached
        .register(&serde_json::to_vec(&valid).unwrap())
        .unwrap();
    assert_eq!(writer.attached_handles().unwrap().live.len(), 2);
    drop(attached);
    assert_eq!(view.attached_handles().unwrap().live.len(), 1);
}

#[test]
fn a_description_retry_syncs_a_rename_that_was_visible_but_not_durable() {
    for reopen in [false, true] {
        let active = FaultGate::closed();
        let renamed = FaultGate::closed();
        let signal = renamed.clone();
        let fs = Arc::new(FaultyFs::new(
            MemFs::new(),
            FaultPolicy::new()
                .inject_gated(
                    FaultOp::Rename,
                    PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
                    Fault::observe_with(move |_, _| signal.open()),
                    When::Once,
                    active.clone(),
                )
                .inject_gated(
                    FaultOp::Fsync,
                    PathMatcher::exact("description-sync"),
                    Fault::io_error(),
                    When::Once,
                    renamed,
                ),
        ));
        let vfs: Arc<dyn Vfs> = fs.clone();
        let root = PathBuf::from("description-sync");
        vfs.create_dir_all(&root).unwrap();
        let mut writer = Store::init(
            Arc::clone(&vfs),
            root.clone(),
            test_config(),
            StoreOptions::without_nonce_source(test_agent()),
        )
        .unwrap();
        active.open();
        assert!(writer.set_agent_description("retry me").is_err());
        assert_eq!(writer.properties().unwrap().agent_description, "retry me");
        if reopen {
            drop(writer);
            writer = Store::open(
                Arc::clone(&vfs),
                root.clone(),
                StoreOptions::without_nonce_source(test_agent()),
            )
            .unwrap()
            .0;
        }
        writer
            .clone()
            .set_agent_description("retry me")
            .unwrap()
            .agent_stamp
            .unwrap();
        drop(writer);
        fs.inner().simulate_crash();
        assert_eq!(
            DiagnosticView::open(vfs, root)
                .unwrap()
                .properties()
                .unwrap()
                .agent_description,
            "retry me"
        );
    }
}

#[test]
fn description_writes_are_durable_and_version_stamps_preserve_the_description() {
    let mem = Arc::new(MemFs::new());
    let vfs: Arc<dyn Vfs> = mem.clone();
    let root = PathBuf::from("description");
    vfs.create_dir_all(&root).unwrap();
    drop(
        Store::init(
            Arc::clone(&vfs),
            root.clone(),
            test_config(),
            StoreOptions::without_nonce_source(test_agent()),
        )
        .unwrap(),
    );
    let changed_agent =
        crate::Agent::new(test_agent().identifier(), "new-version", "not installed", 1);
    let (writer, _) = Store::open(
        Arc::clone(&vfs),
        root.clone(),
        StoreOptions::without_nonce_source(changed_agent),
    )
    .unwrap();
    writer
        .set_agent_description("new\n\"description\"")
        .unwrap()
        .agent_stamp
        .unwrap();
    let properties = writer.properties().unwrap();
    assert_eq!(properties.agent_version, "new-version");
    assert_eq!(properties.agent_description, "new\n\"description\"");
    drop(writer);
    mem.simulate_crash();
    assert_eq!(
        DiagnosticView::open(vfs, root)
            .unwrap()
            .properties()
            .unwrap(),
        properties
    );
}

#[test]
fn an_unchanged_description_does_not_publish_or_stamp() {
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Write,
            PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
            Fault::io_error(),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("no-description-change");
    vfs.create_dir_all(&root).unwrap();
    drop(
        Store::init(
            Arc::clone(&vfs),
            root.clone(),
            test_config(),
            StoreOptions::without_nonce_source(test_agent()),
        )
        .unwrap(),
    );
    let other = crate::Agent::new(test_agent().identifier(), "other-version", "ignored", 1);
    let (writer, _) = Store::open(vfs, root, StoreOptions::without_nonce_source(other)).unwrap();
    gate.open();
    writer
        .set_agent_description(test_agent().description())
        .unwrap()
        .agent_stamp
        .unwrap();
    assert_eq!(
        writer.properties().unwrap().agent_version,
        test_agent().version()
    );
}

#[test]
fn description_failure_returns_an_error_without_hiding_readable_properties() {
    for op in [FaultOp::Write, FaultOp::Fsync, FaultOp::Rename] {
        let gate = FaultGate::closed();
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
            MemFs::new(),
            FaultPolicy::new().inject_gated(
                op,
                PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
                Fault::io_error(),
                When::Once,
                gate.clone(),
            ),
        ));
        let root = PathBuf::from("description-failure");
        vfs.create_dir_all(&root).unwrap();
        let writer = Store::init(
            Arc::clone(&vfs),
            root.clone(),
            test_config(),
            StoreOptions::without_nonce_source(test_agent()),
        )
        .unwrap();
        let view = DiagnosticView::open(vfs, root).unwrap();
        gate.open();
        assert!(writer.set_agent_description("unpublished").is_err());
        assert_eq!(
            view.properties().unwrap().agent_description,
            test_agent().description()
        );
        writer
            .set_agent_description("retry")
            .unwrap()
            .agent_stamp
            .unwrap();
        assert_eq!(view.properties().unwrap().agent_description, "retry");
    }
}

#[test]
fn a_recovery_required_handle_can_diagnose_but_cannot_set_description() {
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
    let root = PathBuf::from("management-recovery");
    vfs.create_dir_all(&root).unwrap();
    let writer = Store::init(
        Arc::clone(&vfs),
        root.clone(),
        test_config(),
        StoreOptions::with_nonce_source(test_agent(), counting_nonce_source()),
    )
    .unwrap();
    gate.open();
    assert!(
        writer
            .update_document(Swap::new(name("docs"), doc("one"), body(b"value")))
            .is_err()
    );
    assert!(writer.recovery_required().unwrap());
    assert!(matches!(
        writer.set_agent_description("refused"),
        Err(StoreError::RecoveryRequired { .. })
    ));
    assert!(writer.properties().is_ok());
    assert_eq!(writer.attached_handles().unwrap().live.len(), 1);
    let view = DiagnosticView::open(vfs, root).unwrap();
    assert!(view.properties().is_ok());
    assert!(view.attached_handles().is_ok());
    assert!(view.recovery_need().unwrap().required());
}

#[test]
fn description_and_properties_are_fresh_through_every_read_surface() {
    let (vfs, writer) = fresh_store();
    let root = writer.root().to_path_buf();
    let reader = Store::open_read_only(
        Arc::clone(&vfs),
        root.clone(),
        StoreOptions::without_nonce_source(test_agent()),
    )
    .unwrap();
    let diagnostic = DiagnosticView::open(Arc::clone(&vfs), root.clone()).unwrap();
    let expected = writer.properties().unwrap();
    assert_eq!(reader.properties().unwrap(), expected);
    assert_eq!(diagnostic.properties().unwrap(), expected);
    let listing = writer.attached_handles().unwrap();
    assert_eq!(listing.live.len(), 2);
    assert_eq!(listing.unknown, 0);
    assert!(!listing.may_be_incomplete);
    assert_eq!(reader.attached_handles().unwrap(), listing);
    assert_eq!(diagnostic.attached_handles().unwrap(), listing);
    writer
        .set_agent_description("new description")
        .unwrap()
        .agent_stamp
        .unwrap();
    let mut expected = expected;
    expected.agent_description = "new description".into();
    assert_eq!(writer.properties().unwrap(), expected);
    assert_eq!(reader.properties().unwrap(), expected);
    assert_eq!(diagnostic.properties().unwrap(), expected);
    assert!(matches!(
        reader.set_agent_description("refused"),
        Err(StoreError::ReadOnly { .. })
    ));
    assert!(!diagnostic.recovery_need().unwrap().required());
    assert_eq!(
        diagnostic.recovery_need().unwrap().unresolved_work(),
        Some(false)
    );
    drop(writer);
    let (reopened, _) =
        Store::open(vfs, root, StoreOptions::without_nonce_source(test_agent())).unwrap();
    assert_eq!(reopened.properties().unwrap(), expected);
}

#[test]
fn management_listings_preserve_registry_incompleteness() {
    let mem = MemFs::new();
    let base: Arc<dyn Vfs> = Arc::new(mem.another_process());
    let root = PathBuf::from("management-full-registry");
    base.create_dir_all(&root).unwrap();
    let writer = Store::init(
        Arc::clone(&base),
        root.clone(),
        test_config(),
        StoreOptions::without_nonce_source(test_agent()),
    )
    .unwrap();
    let faulty: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        mem,
        FaultPolicy::new().inject_matching(
            FaultOp::TryLock,
            PathMatcher::file_name("registry.lock"),
            Fault::error(acadia::VfsErrorKind::Locked),
            When::Always,
        ),
    ));
    let reader = Store::open_read_only(
        faulty,
        root.clone(),
        StoreOptions::without_nonce_source(test_agent()),
    )
    .unwrap();
    let diagnostic = DiagnosticView::open(base, root).unwrap();
    for listing in [
        writer.attached_handles().unwrap(),
        diagnostic.attached_handles().unwrap(),
    ] {
        assert_eq!(listing.live.len(), 1);
        assert!(listing.may_be_incomplete);
    }
    assert!(reader.attached_handles().unwrap().may_be_incomplete);
}

#[test]
fn description_success_is_independent_of_a_failed_agent_stamp() {
    let active = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Write,
            PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
            Fault::io_error(),
            When::OnCall(2),
            active.clone(),
        ),
    ));
    let root = PathBuf::from("description-stamp-failure");
    vfs.create_dir_all(&root).unwrap();
    drop(
        Store::init(
            Arc::clone(&vfs),
            root.clone(),
            test_config(),
            StoreOptions::without_nonce_source(test_agent()),
        )
        .unwrap(),
    );
    let agent = crate::Agent::new(test_agent().identifier(), "new-version", "ignored", 1);
    let (writer, _) = Store::open(vfs, root, StoreOptions::without_nonce_source(agent)).unwrap();
    active.open();
    let receipt = writer
        .set_agent_description("committed description")
        .unwrap();
    assert!(receipt.agent_stamp.is_err());
    assert_eq!(
        writer.properties().unwrap().agent_description,
        "committed description"
    );
    assert_eq!(
        writer.properties().unwrap().agent_version,
        test_agent().version()
    );
    assert!(!writer.recovery_required().unwrap());
}

#[test]
fn diagnosis_requires_a_valid_current_marker_and_recorded_implementation() {
    let (vfs, root) = fresh_root();
    assert!(matches!(
        DiagnosticView::open(Arc::clone(&vfs), root.clone()),
        Err(StoreError::NotAStore { .. })
    ));
    drop(
        Store::init(
            Arc::clone(&vfs),
            root.clone(),
            test_config(),
            StoreOptions::without_nonce_source(test_agent()),
        )
        .unwrap(),
    );
    vfs.remove_file(&root.join(".coord-reservation")).unwrap();
    assert!(matches!(
        DiagnosticView::open(Arc::clone(&vfs), root.clone()),
        Err(StoreError::Coordination(_))
    ));
    vfs.write(&root.join(MARKER_NAME), b"invalid").unwrap();
    assert!(matches!(
        DiagnosticView::open(vfs, root),
        Err(StoreError::Json(_))
    ));
}
