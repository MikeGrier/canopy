// Copyright (c) Michael Grier.

use super::*;
use crate::{Agent, CANOPY_SCHEMA_VERSION, COORDINATED_FORMAT_VERSION};

fn agent_options(agent: Agent) -> StoreOptions {
    StoreOptions::without_nonce_source(agent)
}

fn assert_metadata(vfs: &Arc<dyn Vfs>, root: &Path, agent: &Agent) {
    let marker = json(vfs, &root.join(MARKER_NAME));
    assert_eq!(marker["format_version"], GENESIS_FORMAT_VERSION);
    let metadata = &marker["metadata"];
    assert_eq!(metadata["agent_identifier"], agent.identifier());
    assert_eq!(metadata["agent_version"], agent.version());
    assert_eq!(metadata["agent_description"], agent.description());
    assert_eq!(metadata["canopy_schema"]["current"], CANOPY_SCHEMA_VERSION);
    assert_eq!(
        metadata["canopy_schema"]["oldest_reader"],
        CANOPY_SCHEMA_VERSION
    );
    assert_eq!(metadata["agent_schema"]["current"], agent.schema_version());
    assert_eq!(
        metadata["agent_schema"]["oldest_reader"],
        agent.schema_version()
    );
}

fn old_root(version: u32, policy: FaultPolicy) -> (Arc<FaultyFs<MemFs>>, Arc<dyn Vfs>, PathBuf) {
    let (faulty, vfs, root) = legacy_root(MIGRATABLE_FORMAT_VERSION, policy);
    if version == COORDINATED_FORMAT_VERSION {
        let mut marker = json(&vfs, &root.join(LEGACY_MARKER_NAME));
        let identity = marker.as_object_mut().unwrap().remove("shire_id").unwrap();
        marker["store_id"] = identity;
        marker["format_version"] = version.into();
        let bytes = serde_json::to_vec_pretty(&marker).unwrap();
        FileFactory::new()
            .open(Arc::clone(&vfs), &root)
            .unwrap()
            .reserve(&bytes, CreationNonce::from_bytes([1; 16]))
            .unwrap();
        vfs.write(&root.join(MARKER_NAME), &bytes).unwrap();
        vfs.fsync(&root.join(MARKER_NAME)).unwrap();
        vfs.remove_file(&root.join(LEGACY_MARKER_NAME)).unwrap();
        vfs.fsync(&root).unwrap();
    }
    (faulty, vfs, root)
}

#[test]
fn creation_and_both_open_modes_preserve_agent_metadata_without_stamping() {
    for schema in [0, 1, 2, 3, 4, 7, 8, 16, 100, u32::MAX] {
        let (vfs, root) = fresh_root();
        let agent = Agent::new(
            format!("agent-{schema}"),
            "release-Z",
            "description\n\"quoted\"",
            schema,
        );
        let options = agent_options(agent.clone());
        let store = Store::init(Arc::clone(&vfs), root.clone(), test_config(), options).unwrap();
        assert_metadata(&vfs, &root, &agent);
        let before = vfs.read(&root.join(MARKER_NAME)).unwrap();
        let later = Agent::new(
            agent.identifier(),
            "release-A",
            "different description",
            schema,
        );
        let reader =
            Store::open_read_only(Arc::clone(&vfs), root.clone(), agent_options(later.clone()))
                .unwrap();
        assert_eq!(reader.store_id(), store.store_id());
        drop(reader);
        drop(store);
        let (writer, _) =
            Store::open(Arc::clone(&vfs), root.clone(), agent_options(later)).unwrap();
        drop(writer);
        assert_eq!(vfs.read(&root.join(MARKER_NAME)).unwrap(), before);
    }
}

#[test]
fn a_different_agent_is_refused_by_both_open_modes_and_current_migration() {
    let (vfs, root) = fresh_root();
    drop(Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).unwrap());
    let before = vfs.read(&root.join(MARKER_NAME)).unwrap();
    for identifier in ["another-agent", "CANOPY-TESTS", "canopy-tests ", ""] {
        let other = || agent_options(Agent::new(identifier, "test", "", 1));
        for result in [
            Store::open(Arc::clone(&vfs), root.clone(), other()).map(drop),
            Store::open_read_only(Arc::clone(&vfs), root.clone(), other()).map(drop),
            Store::migrate(Arc::clone(&vfs), root.clone(), &other()).map(drop),
        ] {
            assert!(
                matches!(result, Err(StoreError::AgentMismatch { recorded, requested })
                if recorded == test_agent().identifier() && requested == identifier)
            );
        }
        assert_eq!(vfs.read(&root.join(MARKER_NAME)).unwrap(), before);
    }
    assert!(Store::open(vfs, root, options()).is_ok());
}

/// Creations that differ from the test agent's with `test_config` in what the store is.
fn other_identities() -> Vec<(Agent, GenesisConfig)> {
    vec![
        (
            Agent::new("other", "test", "Canopy test fixture", 1),
            test_config(),
        ),
        (
            test_agent(),
            GenesisConfig::new(StoreId::new("other-store").unwrap(), 12_345),
        ),
        (
            test_agent(),
            test_config()
                .with_blob_replica_count(crate::DEFAULT_BLOB_REPLICA_COUNT + 2)
                .unwrap(),
        ),
        (
            test_agent(),
            test_config()
                .with_pointer_replica_count(crate::DEFAULT_POINTER_REPLICA_COUNT + 2)
                .unwrap(),
        ),
        (
            test_agent(),
            test_config()
                .with_shard_width(crate::DEFAULT_SHARD_WIDTH + 1)
                .unwrap(),
        ),
    ]
}

/// Creations that differ from the test agent's with `test_config` only in provenance.
fn other_provenances() -> Vec<(Agent, GenesisConfig)> {
    vec![
        (
            Agent::new("canopy-tests", "other", "Canopy test fixture", 1),
            test_config(),
        ),
        (
            Agent::new("canopy-tests", "test", "other", 1),
            test_config(),
        ),
        (
            Agent::new("canopy-tests", "test", "Canopy test fixture", 2),
            test_config(),
        ),
        (
            test_agent(),
            GenesisConfig::new(test_config().store_id().clone(), 98_765),
        ),
    ]
}

/// A root whose creation by the test agent with `test_config` stopped after its reservation.
fn interrupted_creation() -> (Arc<dyn Vfs>, PathBuf) {
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::CreateDirAll,
            PathMatcher::file_name("p0"),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("agent-creation");
    vfs.create_dir_all(&root).unwrap();
    gate.open();
    assert!(Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).is_err());
    assert!(exists(&vfs, &root, RESERVATION));
    assert!(!exists(&vfs, &root, MARKER_NAME));
    (vfs, root)
}

#[test]
fn every_identity_creation_input_is_bound_by_the_reservation() {
    let (vfs, root) = interrupted_creation();
    let reserved = vfs.read(&root.join(RESERVATION)).unwrap();
    for (agent, config) in other_identities() {
        assert!(matches!(
            Store::init(Arc::clone(&vfs), root.clone(), config, agent_options(agent)),
            Err(StoreError::RootReserved { .. })
        ));
        assert_eq!(vfs.read(&root.join(RESERVATION)).unwrap(), reserved);
        assert!(!exists(&vfs, &root, MARKER_NAME));
    }
    drop(Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).unwrap());
    assert_metadata(&vfs, &root, &test_agent());
}

#[test]
fn no_provenance_creation_input_is_bound_by_the_reservation() {
    for (agent, config) in other_provenances() {
        let (vfs, root) = interrupted_creation();
        let reserved = vfs.read(&root.join(RESERVATION)).unwrap();
        let created_at = config.created_at_unix_ms();
        let store = Store::init(
            Arc::clone(&vfs),
            root.clone(),
            config,
            agent_options(agent.clone()),
        )
        .unwrap();
        assert_metadata(&vfs, &root, &agent);
        assert_eq!(store.created_at_unix_ms(), created_at);
        assert_eq!(vfs.read(&root.join(RESERVATION)).unwrap(), reserved);
    }
}

#[test]
fn both_prior_formats_migrate_with_explicit_agent_metadata_and_preserve_documents() {
    for version in [MIGRATABLE_FORMAT_VERSION, COORDINATED_FORMAT_VERSION] {
        for schema in [0, 1, 2, 7, u32::MAX] {
            let (_, vfs, root) = old_root(version, FaultPolicy::new());
            let reservation = vfs.read(&root.join(RESERVATION)).ok();
            let agent = Agent::new(
                "migrating-agent",
                format!("version-{schema}"),
                "migrated",
                schema,
            );
            let options = agent_options(agent.clone());
            assert_eq!(
                Store::migrate(Arc::clone(&vfs), root.clone(), &options).unwrap(),
                Migration::Migrated
            );
            assert_metadata(&vfs, &root, &agent);
            if let Some(reservation) = reservation {
                assert_eq!(vfs.read(&root.join(RESERVATION)).unwrap(), reservation);
            }
            let (store, _) =
                Store::open(Arc::clone(&vfs), root.clone(), agent_options(agent.clone())).unwrap();
            store
                .recover()
                .unwrap()
                .agent_stamp
                .expect("agent version stamp");
            assert_eq!(
                store.read_body(name("tickets"), &doc("record")).unwrap(),
                Some(body(b"v1"))
            );
            drop(store);
            assert_eq!(
                Store::migrate(vfs, root, &options).unwrap(),
                Migration::AlreadyCurrent
            );
        }
    }
}

#[test]
fn old_markers_are_refused_before_their_missing_metadata_is_decoded() {
    for version in [MIGRATABLE_FORMAT_VERSION, COORDINATED_FORMAT_VERSION] {
        let (_, vfs, root) = old_root(version, FaultPolicy::new());
        for result in [
            Store::open(Arc::clone(&vfs), root.clone(), options()).map(drop),
            Store::open_read_only(Arc::clone(&vfs), root.clone(), options()).map(drop),
        ] {
            assert!(
                matches!(result, Err(StoreError::MigrationRequired { found, expected, .. })
                if found == version && expected == GENESIS_FORMAT_VERSION)
            );
        }
    }
}

#[test]
fn a_v7_version_gate_refuses_a_v8_marker() {
    #[derive(serde::Deserialize)]
    struct VersionProbe {
        format_version: u32,
    }
    let (vfs, root) = fresh_root();
    drop(Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).unwrap());
    let probe: VersionProbe =
        serde_json::from_slice(&vfs.read(&root.join(MARKER_NAME)).unwrap()).unwrap();
    assert_eq!(probe.format_version, GENESIS_FORMAT_VERSION);
    assert_ne!(probe.format_version, COORDINATED_FORMAT_VERSION);
    assert!(!exists(&vfs, &root, LEGACY_MARKER_NAME));
}

/// Agents that differ from the test agent in one input each, every one of which a migration binds.
fn agents_differing_in_one_input() -> [Agent; 4] {
    [
        Agent::new("other", "test", "Canopy test fixture", 1),
        Agent::new("canopy-tests", "other", "Canopy test fixture", 1),
        Agent::new("canopy-tests", "test", "other", 1),
        Agent::new("canopy-tests", "test", "Canopy test fixture", 2),
    ]
}

#[test]
fn interrupted_migrations_reject_every_changed_agent_input() {
    for version in [MIGRATABLE_FORMAT_VERSION, COORDINATED_FORMAT_VERSION] {
        let gate = FaultGate::closed();
        let policy = FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::file_name(format!("{MARKER_NAME}.tmp")),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        );
        let (_, vfs, root) = old_root(version, policy);
        gate.open();
        assert!(Store::migrate(Arc::clone(&vfs), root.clone(), &options()).is_err());
        let record = vfs.read(&root.join(MIGRATION_RECORD_NAME)).unwrap();
        for other in agents_differing_in_one_input() {
            assert!(matches!(
                Store::migrate(Arc::clone(&vfs), root.clone(), &agent_options(other)),
                Err(StoreError::MigrationInputsDiffer { .. })
            ));
            assert_eq!(vfs.read(&root.join(MIGRATION_RECORD_NAME)).unwrap(), record);
        }
        Store::migrate(Arc::clone(&vfs), root.clone(), &options()).unwrap();
        assert_metadata(&vfs, &root, &test_agent());
    }
}

#[test]
fn v7_migration_resumes_after_each_publication_and_cleanup_failure() {
    let mut cuts = vec![];
    for file in [MIGRATION_RECORD_NAME, MARKER_NAME] {
        for op in [FaultOp::Write, FaultOp::Fsync, FaultOp::Rename] {
            cuts.push((
                op,
                PathMatcher::file_name(format!("{file}.tmp")),
                When::Once,
            ));
        }
    }
    cuts.push((
        FaultOp::RemoveFile,
        PathMatcher::file_name(MIGRATION_RECORD_NAME),
        When::Once,
    ));
    for call in 1..=4 {
        cuts.push((
            FaultOp::Fsync,
            PathMatcher::exact("migrating"),
            When::OnCall(call),
        ));
    }
    for (op, matcher, when) in cuts {
        for crash in [false, true] {
            let gate = FaultGate::closed();
            let policy = FaultPolicy::new().inject_gated(
                op,
                matcher.clone(),
                Fault::io_error(),
                when,
                gate.clone(),
            );
            let (faulty, mut vfs, root) = old_root(COORDINATED_FORMAT_VERSION, policy);
            let reservation = vfs.read(&root.join(RESERVATION)).unwrap();
            gate.open();
            Store::migrate(Arc::clone(&vfs), root.clone(), &options())
                .expect_err("injected interruption");
            gate.close();
            if crash {
                vfs = Arc::new(faulty.simulate_restart());
            }
            Store::migrate(Arc::clone(&vfs), root.clone(), &options()).unwrap();
            assert_metadata(&vfs, &root, &test_agent());
            assert_eq!(vfs.read(&root.join(RESERVATION)).unwrap(), reservation);
            assert!(!exists(&vfs, &root, MIGRATION_RECORD_NAME));
            let (store, _) = Store::open(vfs, root, options()).unwrap();
            store
                .recover()
                .unwrap()
                .agent_stamp
                .expect("agent version stamp");
            assert_eq!(
                store.read_body(name("tickets"), &doc("record")).unwrap(),
                Some(body(b"v1"))
            );
        }
    }
}

#[test]
fn missing_metadata_and_inverted_reader_floors_are_rejected() {
    for field in ["metadata", "canopy_schema", "agent_schema"] {
        let (vfs, root) = fresh_root();
        drop(Store::init(Arc::clone(&vfs), root.clone(), test_config(), options()).unwrap());
        let mut marker = json(&vfs, &root.join(MARKER_NAME));
        if field == "metadata" {
            marker.as_object_mut().unwrap().remove(field);
        } else {
            marker["metadata"][field]["oldest_reader"] = (u32::MAX).into();
        }
        vfs.write(
            &root.join(MARKER_NAME),
            &serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        assert!(Store::open(Arc::clone(&vfs), root.clone(), options()).is_err());
        assert!(Store::open_read_only(vfs, root, options()).is_err());
    }
}
