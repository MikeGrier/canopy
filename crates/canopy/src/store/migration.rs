// Copyright (c) Michael Grier.

//! Migrating a root from the previous genesis version: the record of the migration in progress,
//! the steps it makes, and what each interruption leaves (canopy's DESIGN-NOTES.md, "A root is
//! migrated to the coordination layer's format").
//!
//! Every step is idempotent and the order is the protocol: the record, the implementation record,
//! the new marker -- which commits the migration -- then the old marker, then the migration record.
//! A root with a migration record is never opened; the migration is run again, and finishes.

use acadia::coord::{CoordError, CoordinationFactory, CreationNonce};

use super::*;

/// What a migration is of, recorded before it changes anything and recorded as the inputs of the
/// reservation, so an implementation that resumes is checked against it by the coordination layer.
///
/// Changing a field's name is a breaking change to the on-disk format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MigrationInputs {
    pub(super) from_version: u32,
    pub(super) to_version: u32,
    pub(super) agent: Agent,
}

/// The migration record: its inputs, the implementation it records for the root, and the marker it
/// publishes, so that it is finished from the record alone once the old marker is gone.
///
/// Changing a field's name is a breaking change to the on-disk format.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct MigrationRecord {
    pub(super) inputs: MigrationInputs,
    pub(super) implementation: String,
    pub(super) genesis: Genesis,
}

/// The common layout fields of either supported source marker, with its identity key normalized.
#[derive(Debug, Deserialize)]
struct LegacyGenesis {
    blob_replica_count: usize,
    pointer_replica_count: usize,
    shard_width: usize,
    store_id: String,
    created_at_unix_ms: u64,
}

/// What a root's marker and migration record say it is.
pub(super) enum Found {
    /// A marker of this build's version, and no migration in progress.
    Current(Vec<u8>),
    /// A supported older marker, and no migration begun.
    Legacy(Vec<u8>),
    /// A migration was begun and not finished.
    Interrupted(Box<MigrationRecord>),
}

impl MigrationRecord {
    /// Requires the recorded implementation, agent inputs and target metadata to match the retry.
    pub(super) fn check(
        &self,
        root: &Path,
        factory: &dyn CoordinationFactory,
        agent: &Agent,
    ) -> Result<(), StoreError> {
        let requested = factory.implementation();
        if self.implementation != requested.as_str() {
            return Err(StoreError::MigrationInputsDiffer {
                root: root.to_path_buf(),
                recorded: self.implementation.clone(),
                requested: requested.to_string(),
            });
        }
        if self.inputs.agent == *agent
            && self.inputs.to_version == GENESIS_FORMAT_VERSION
            && matches!(
                self.inputs.from_version,
                MIGRATABLE_FORMAT_VERSION | COORDINATED_FORMAT_VERSION | AGENT_FORMAT_VERSION
            )
            && self.genesis.format_version == GENESIS_FORMAT_VERSION
            && (if self.inputs.from_version == AGENT_FORMAT_VERSION {
                self.genesis.metadata.check_identity(agent).is_ok()
                    && self.genesis.metadata.transition.is_none()
            } else {
                self.genesis.metadata == super::agent::AgentMetadata::new(agent)
            })
        {
            return Ok(());
        }
        Err(StoreError::MigrationInputsDiffer {
            root: root.to_path_buf(),
            recorded: format!("{} {:?}", self.implementation, self.inputs),
            requested: format!("{} {:?} to {}", requested, agent, GENESIS_FORMAT_VERSION),
        })
    }
}

impl Store {
    /// What the root is: read from the marker under its current name, then its old one, and the
    /// migration record, which decides before either.
    ///
    /// The version decides refusal before the marker is read as this build's shape (`D325`): a
    /// foreign version is never reported as malformed JSON just because its field set differs.
    pub(super) fn find(vfs: &Arc<dyn Vfs>, root: &Path) -> Result<Found, StoreError> {
        if let Some(record) = Self::read_migration_record(vfs, root)? {
            return Ok(Found::Interrupted(record));
        }
        let (bytes, version, legacy) = match vfs.read(&root.join(MARKER_NAME)) {
            Ok(bytes) => {
                let version = Self::probe_version(&bytes)?;
                (bytes, version, false)
            }
            Err(e) if e.kind() == VfsErrorKind::NotFound => {
                match vfs.read(&root.join(LEGACY_MARKER_NAME)) {
                    Ok(bytes) => {
                        let version = Self::probe_version(&bytes)?;
                        (bytes, version, true)
                    }
                    Err(e) if e.kind() == VfsErrorKind::NotFound => {
                        return Err(StoreError::NotAStore {
                            root: root.to_path_buf(),
                        });
                    }
                    Err(e) => return Err(StoreError::Vfs(e)),
                }
            }
            Err(e) => return Err(StoreError::Vfs(e)),
        };
        match (version, legacy) {
            (GENESIS_FORMAT_VERSION, false) => Ok(Found::Current(bytes)),
            (MIGRATABLE_FORMAT_VERSION, true) => Ok(Found::Legacy(bytes)),
            (COORDINATED_FORMAT_VERSION, false) => Ok(Found::Legacy(bytes)),
            (AGENT_FORMAT_VERSION, false) => Ok(Found::Legacy(bytes)),
            (found, _) => Err(StoreError::UnsupportedFormatVersion {
                found,
                expected: GENESIS_FORMAT_VERSION,
            }),
        }
    }

    pub(super) fn probe_version(bytes: &[u8]) -> Result<u32, StoreError> {
        let probe: GenesisVersion = serde_json::from_slice(bytes)?;
        Ok(probe.format_version)
    }

    fn read_migration_record(
        vfs: &Arc<dyn Vfs>,
        root: &Path,
    ) -> Result<Option<Box<MigrationRecord>>, StoreError> {
        match vfs.read(&root.join(MIGRATION_RECORD_NAME)) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|_| StoreError::MalformedMarker {
                        root: root.to_path_buf(),
                        reason: "the migration record is not one this build wrote",
                    })
            }
            Err(e) if e.kind() == VfsErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Vfs(e)),
        }
    }

    /// The record of migrating the root whose previous-version marker is `legacy`, checked against
    /// what is on disk as an open checks a marker, so a root that would not open is not migrated.
    pub(super) fn begin_migration(
        vfs: &Arc<dyn Vfs>,
        root: &Path,
        legacy: &[u8],
        factory: &dyn CoordinationFactory,
        agent: &Agent,
    ) -> Result<MigrationRecord, StoreError> {
        let from_version = Self::probe_version(legacy)?;
        let mut value: serde_json::Value = serde_json::from_slice(legacy)?;
        if from_version == MIGRATABLE_FORMAT_VERSION {
            let object = value
                .as_object_mut()
                .ok_or_else(|| StoreError::MalformedMarker {
                    root: root.to_path_buf(),
                    reason: "a marker must be an object",
                })?;
            let identity =
                object
                    .remove("shire_id")
                    .ok_or_else(|| StoreError::MalformedMarker {
                        root: root.to_path_buf(),
                        reason: "the legacy marker has no shire_id",
                    })?;
            object.insert("store_id".into(), identity);
        } else {
            factory
                .open(Arc::clone(vfs), root)?
                .confirm_created(MARKER_NAME)?;
        }
        let metadata = if from_version == AGENT_FORMAT_VERSION {
            let metadata: super::agent::AgentMetadata =
                serde_json::from_value(value.get("metadata").cloned().ok_or(
                    StoreError::MalformedMarker {
                        root: root.to_path_buf(),
                        reason: "an agent-format marker has no metadata",
                    },
                )?)?;
            metadata.check_identity(agent)?;
            if metadata.transition.is_some() || vfs.exists(&root.join(UPGRADE_INTENT_NAME))? {
                return Err(StoreError::RecoveryRequired {
                    root: root.to_path_buf(),
                });
            }
            metadata
        } else {
            super::agent::AgentMetadata::new(agent)
        };
        let legacy: LegacyGenesis = serde_json::from_value(value)?;
        let genesis = Genesis {
            format_version: GENESIS_FORMAT_VERSION,
            blob_replica_count: legacy.blob_replica_count,
            pointer_replica_count: legacy.pointer_replica_count,
            shard_width: legacy.shard_width,
            store_id: legacy.store_id,
            created_at_unix_ms: legacy.created_at_unix_ms,
            metadata,
        };
        Self::validate_genesis(vfs, root, &genesis)?;
        Ok(MigrationRecord {
            inputs: MigrationInputs {
                from_version,
                to_version: GENESIS_FORMAT_VERSION,
                agent: agent.clone(),
            },
            implementation: factory.implementation().to_string(),
            genesis,
        })
    }

    /// Makes every step after the record: the implementation record, the new marker, which commits
    /// the migration, then the old marker and the record, which only tidy a root already migrated.
    pub(super) fn finish_migration(
        vfs: &Arc<dyn Vfs>,
        root: &Path,
        factory: &dyn CoordinationFactory,
        record: &MigrationRecord,
    ) -> Result<(), StoreError> {
        if record.inputs.from_version == MIGRATABLE_FORMAT_VERSION {
            let inputs = serde_json::to_vec(record)?;
            Self::reserve_implementation(vfs, root, factory, &inputs)?;
        } else {
            factory
                .open(Arc::clone(vfs), root)?
                .confirm_created(MARKER_NAME)?;
        }
        let marker = serde_json::to_vec_pretty(&record.genesis)?;
        Self::publish_durably(vfs, root, MARKER_NAME, &marker)?;
        Self::remove_durably(vfs, root, LEGACY_MARKER_NAME)?;
        Self::remove_durably(vfs, root, MIGRATION_RECORD_NAME)
    }

    /// Records `factory`'s implementation as the one that coordinates the root, with `inputs` the
    /// creation or migration it records it for. The record is the coordination layer's reservation,
    /// and is durable when this returns.
    ///
    /// The reservation's nonce is derived from `inputs`, which hold the caller-supplied identity and
    /// creation time: canopy has no clock or random source of its own.
    pub(super) fn reserve_implementation(
        vfs: &Arc<dyn Vfs>,
        root: &Path,
        factory: &dyn CoordinationFactory,
        inputs: &[u8],
    ) -> Result<(), StoreError> {
        let digest = ContentDigest::of(inputs);
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&digest.as_bytes()[..16]);
        let coordination = factory.open(Arc::clone(vfs), root)?;
        match coordination.reserve(inputs, CreationNonce::from_bytes(nonce)) {
            Ok(_) => Ok(()),
            Err(CoordError::Reserved) => Err(StoreError::RootReserved {
                root: root.to_path_buf(),
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes `bytes` as the root's `name` atomically: a temp sibling made durable, renamed over
    /// the name, and the rename made durable.
    pub(super) fn publish_durably(
        vfs: &Arc<dyn Vfs>,
        root: &Path,
        name: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        let tmp = root.join(format!("{name}.tmp"));
        vfs.write(&tmp, bytes)?;
        vfs.fsync(&tmp)?;
        vfs.rename(&tmp, &root.join(name))?;
        vfs.fsync(root)?;
        Ok(())
    }

    /// Removes the root's `name` if it is there, and makes the removal durable.
    fn remove_durably(vfs: &Arc<dyn Vfs>, root: &Path, name: &str) -> Result<(), StoreError> {
        match vfs.remove_file(&root.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == VfsErrorKind::NotFound => {}
            Err(e) => return Err(StoreError::Vfs(e)),
        }
        vfs.fsync(root)?;
        Ok(())
    }
}
