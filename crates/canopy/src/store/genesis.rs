// Copyright (c) Michael Grier.
// Split from store.rs at 1303f9a.

//! The `init` / `open` paths: writing the genesis marker replicas over an empty
//! root, and reading them back into a live store (LAY-1).
//!
//! The genesis types themselves (`StoreId`, `GenesisConfig`, `Genesis`) stay in
//! the parent module: `Store` holds a `Genesis` field, so the cluster is bound to
//! the struct definition.

use super::*;
use crate::store::blob::blob_partition_root;
use crate::store::migration::Found;
use crate::store::namespace::namespace_partition_root;
use acadia::coord::{
    Access, ArtifactMaker, CoordError, CoordResult, Coordination, CoordinationFactory,
    CreationNonce, CreationPlan, ModificationEnd, Purpose, Waiting,
};

use super::coordinated::Coordinated;

impl Store {
    pub(super) fn absolute_root(root: impl Into<PathBuf>) -> Result<PathBuf, StoreError> {
        let root = root.into();
        if root.is_absolute() {
            Ok(root)
        } else {
            Err(StoreError::RootNotAbsolute { root })
        }
    }
    /// Rechecks admission under the tiers before registering or taking writer exclusion.
    fn attach_store(
        vfs: &Arc<dyn Vfs>,
        root: &Path,
        factory: &dyn CoordinationFactory,
        agent: &Agent,
        access: Access,
    ) -> Result<(Genesis, Coordinated), StoreError> {
        let participant = factory.open(Arc::clone(vfs), root)?;
        participant.confirm_created(MARKER_NAME)?;
        let mut attachment = participant.attach(access, Waiting::Wait)?;
        let genesis = Self::read_admission_genesis(vfs, root)?;
        genesis.metadata.check_admission(agent, access)?;
        let payload = super::agent::registration_payload(agent, attachment.tiers(), false)?;
        let _ = attachment.register(&payload);
        let coordinated = match access {
            Access::Read => Coordinated::reader(participant, attachment),
            Access::Write => Coordinated::writer(participant, attachment),
        };
        Ok((genesis, coordinated))
    }

    /// Takes offline migration's legacy root lock, naming contention as
    /// [`StoreError::RootInUse`] rather than letting it fold into a bare
    /// [`StoreError::Vfs`].
    fn lock_root(vfs: &Arc<dyn Vfs>, root: &Path) -> Result<Arc<dyn VfsLock>, StoreError> {
        match vfs.lock_exclusive(&root.join(LOCK_NAME)) {
            Ok(guard) => Ok(Arc::from(guard)),
            Err(e) if e.kind() == VfsErrorKind::Locked => Err(StoreError::RootInUse {
                root: root.to_path_buf(),
            }),
            Err(e) => Err(StoreError::Vfs(e)),
        }
    }

    /// Makes `root` a store with the identity `config` and `options` describe, or opens the store
    /// with that identity it already is (D40/LAY-1).
    ///
    /// **Idempotent.** Any number of calls with one identity -- at once, in any processes, or one
    /// after another -- end with one store, and a call returns it whether it made the store,
    /// finished a creation another call began, or found it made; a store it finds is opened under
    /// [`open`](Store::open)'s rules. A store is never made over. A caller that seeds a store does
    /// so idempotently: a creator can die between finishing the store and seeding it, and the next
    /// call cannot tell. The identity is the store id, the layout and the agent's identifier; the
    /// creation time and the agent's version, description and schema are the provenance of
    /// whichever call makes the marker, and are not compared.
    ///
    /// `root` must be an absolute path to an existing directory, holding nothing but what a creation of this
    /// store makes: a root holding anything else, dotfiles included, is [`StoreError::NotEmpty`],
    /// and one that is another store is [`StoreError::AlreadyInitialized`]. The creation goes
    /// through the coordination implementation `options` names, which records itself and the
    /// identity in a reservation before anything else is made; a root recorded for another
    /// implementation or another identity is [`StoreError::RootReserved`], as is one a build that
    /// compared every creation input began and did not finish. The two partition trees are made
    /// first and the [`.store`](MARKER_NAME) marker **last**, published atomically, so a crash
    /// leaves no marker and a later call finishes the creation; [`open`](Store::open) refuses a
    /// root without one.
    ///
    /// `root` must be on a local filesystem; a networked root is unsupported and not
    /// detected (see [`Store`]).
    ///
    /// `options` supplies the agent, its schema and the nonce source; see [`StoreOptions`].
    pub fn init(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        config: GenesisConfig,
        options: StoreOptions,
    ) -> Result<Self, StoreError> {
        let factory = options.coordination();
        let store = Self::init_through(vfs, root, config, &*factory, options.agent())?;
        store.install(options);
        Ok(store)
    }

    /// [`init`](Store::init) with no nonce source, for tests that name every nonce themselves.
    #[cfg(test)]
    pub(crate) fn init_unsourced(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        config: GenesisConfig,
    ) -> Result<Self, StoreError> {
        Self::init(
            vfs,
            root,
            config,
            StoreOptions::without_nonce_source(crate::tests::test_agent()),
        )
    }

    fn init_through(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        config: GenesisConfig,
        factory: &dyn CoordinationFactory,
        agent: &Agent,
    ) -> Result<Self, StoreError> {
        let root = Self::absolute_root(root)?;

        match vfs.metadata(&root) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(StoreError::RootNotADirectory { root }),
            Err(e) if e.kind() == VfsErrorKind::NotFound => {
                return Err(StoreError::RootNotFound { root });
            }
            Err(e) => return Err(StoreError::Vfs(e)),
        }

        // No creation makes the marker of the format before migration, so its presence is read
        // without exclusion: the root is a store an older binary made.
        if vfs.exists(&root.join(LEGACY_MARKER_NAME))? {
            return Err(StoreError::AlreadyInitialized { root });
        }

        let genesis = Genesis::from_config(config, agent);
        let marker = serde_json::to_vec_pretty(&genesis)?;
        let inputs = genesis.creation_inputs()?;
        let artifacts = [BLOB_TREE_DIR, NAMESPACE_TREE_DIR, MARKER_NAME];
        // Creators with one identity make one creation, so the nonce tells nothing apart.
        let plan = CreationPlan {
            inputs: &inputs,
            nonce: CreationNonce::from_bytes([0; 16]),
            artifacts: &artifacts,
        };
        let vouching = factory.open(Arc::clone(&vfs), &root)?;
        let mut maker = GenesisMaker {
            vfs: &vfs,
            root: &root,
            genesis: &genesis,
            marker: &marker,
            vouching: &*vouching,
        };
        let participant = factory.open(Arc::clone(&vfs), &root)?;
        match participant.create(&plan, &mut maker) {
            Ok(_) => {}
            Err(CoordError::AlreadyCreated) => return Err(StoreError::AlreadyInitialized { root }),
            Err(CoordError::Occupied { name }) => {
                return Err(StoreError::NotEmpty { root, entry: name });
            }
            Err(CoordError::Reserved) => return Err(StoreError::RootReserved { root }),
            Err(CoordError::Vfs(error)) => return Err(StoreError::Vfs(error)),
            Err(error) => return Err(error.into()),
        }
        let (genesis, coordination) =
            Self::attach_store(&vfs, &root, factory, agent, Access::Write)?;
        drop((participant, vouching));

        Ok(Store {
            vfs,
            root,
            genesis,
            blob_reads: Arc::default(),
            pointer_reads: Arc::default(),
            coordination: Arc::new(coordination),
            repair_backlog: Arc::default(),
            writer: Arc::default(),
            log_durability: Arc::default(),
            unsynced_dirs: Arc::default(),
            publication: Arc::default(),
            surface: Arc::default(),
        })
    }

    /// Opens the existing store under `root` (D40/LAY-1).
    ///
    /// Reads and validates the [`.store`](MARKER_NAME) marker: an absent marker
    /// is [`StoreError::NotAStore`], marker bytes that are not the expected JSON
    /// are [`StoreError::Json`], a marker written by an unknown genesis version is
    /// [`StoreError::UnsupportedFormatVersion`], and a marker whose recorded
    /// layout is inconsistent with what is on disk (a bad shard width or replica
    /// count, a missing or non-directory partition) is
    /// [`StoreError::MalformedMarker`] / [`StoreError::MissingPartition`]. Only a
    /// fully valid store yields a handle.
    ///
    /// `root` must be on a local filesystem; a networked root is unsupported and not
    /// detected (see [`Store`]).
    ///
    /// `options` supplies the nonce source the write verbs draw from and the coordination
    /// implementation the root must record; see [`StoreOptions`].
    ///
    /// The store holds shared attachment tiers, but no writer exclusion, between operations.
    /// Opening changes only volatile attachment/registry state, not store data or recovery state.
    /// It returns, beside the handle, what discovery observed -- a fence, state that cannot
    /// be vouched for, a holder that died -- and whether writers left work to resolve. What to do
    /// about it is the caller's: [`resolve`](Store::resolve) finishes what writers left, and when
    /// the state was vouched for that is all a fence needs; [`recover`](Store::recover), or a
    /// step-at-a-time [`recovery`](Store::recovery), is the general case. Until one of them has
    /// lifted it, a handle that [needs recovery](Store::recovery_required) refuses writes and
    /// still reads.
    ///
    /// A root whose recorded coordination implementation is not the one `options` names is refused
    /// with [`StoreError::Coordination`].
    pub fn open(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        options: StoreOptions,
    ) -> Result<(Self, RecoveryNeed), StoreError> {
        let factory = options.coordination();
        let store = Self::open_through(vfs, root, &*factory, options.agent())?;
        store.install(options);
        let found = store.found_on_opening()?;
        Ok((store, found))
    }

    /// [`open`](Store::open) with no nonce source, returning the handle alone, for tests that name
    /// every nonce themselves.
    #[cfg(test)]
    pub(crate) fn open_unsourced(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
    ) -> Result<Self, StoreError> {
        Self::open(
            vfs,
            root,
            StoreOptions::without_nonce_source(crate::tests::test_agent()),
        )
        .map(|(store, _)| store)
    }

    /// Opens the existing store under `root` to read it only, taking neither the root's lock nor
    /// writer exclusion, so any number of processes may read a store another process writes.
    /// It holds the reader tier shared until the last clone is dropped.
    ///
    /// It validates the marker as [`open`](Store::open) does, and recovers nothing: what a writer
    /// that died left stays as it is, and reads classify it as contention through the
    /// [modification count](Store::modifications), which stays odd until a writer opens the store
    /// and finishes recovery. Every write verb is refused with [`StoreError::ReadOnly`].
    ///
    /// `options` names the coordination implementation the root must record, as it does for
    /// [`open`](Store::open); its nonce source is never drawn from.
    pub fn open_read_only(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        options: StoreOptions,
    ) -> Result<Self, StoreError> {
        let root = Self::absolute_root(root)?;
        // Validation first, so a directory that is not a store is refused as one rather than as
        // an implementation mismatch.
        Self::read_genesis(&vfs, &root)?
            .metadata
            .check_identity(options.agent())?;
        let (genesis, coordination) = Self::attach_store(
            &vfs,
            &root,
            &*options.coordination(),
            options.agent(),
            Access::Read,
        )?;
        let store = Store {
            vfs,
            root,
            genesis,
            blob_reads: Arc::default(),
            pointer_reads: Arc::default(),
            coordination: Arc::new(coordination),
            repair_backlog: Arc::default(),
            writer: Arc::default(),
            log_durability: Arc::default(),
            unsynced_dirs: Arc::default(),
            publication: Arc::default(),
            surface: Arc::default(),
        };
        store.install(options);
        Ok(store)
    }

    fn open_through(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        factory: &dyn CoordinationFactory,
        agent: &Agent,
    ) -> Result<Self, StoreError> {
        let root = Self::absolute_root(root)?;

        // Validation before attachment leaves a non-store untouched; admission rechecks under its tiers.
        Self::read_genesis(&vfs, &root)?
            .metadata
            .check_identity(agent)?;
        let (genesis, coordination) =
            Self::attach_store(&vfs, &root, factory, agent, Access::Write)?;

        let store = Store {
            vfs,
            root,
            genesis,
            blob_reads: Arc::default(),
            pointer_reads: Arc::default(),
            coordination: Arc::new(coordination),
            repair_backlog: Arc::default(),
            writer: Arc::default(),
            log_durability: Arc::default(),
            unsynced_dirs: Arc::default(),
            publication: Arc::default(),
            surface: Arc::default(),
        };
        Ok(store)
    }

    /// Migrates earlier supported formats to the current barrier; agent-format metadata is preserved.
    ///
    /// # The root must be offline
    ///
    /// No binary may have the root open, or be starting to open it, while it migrates. The
    /// root's lock, which the migration holds, excludes a process holding the root open; it
    /// cannot exclude an older binary that read the marker before the migration and takes the
    /// lock after it, which would go on as if the root were still at the older version.
    /// Stopping every older binary first is the caller's part.
    ///
    /// # It is crash-consistent and resumable
    ///
    /// Under the root's lock, with the root read again under it, the migration durably records a
    /// migration record ([`MIGRATION_RECORD_NAME`]) naming itself, its inputs and the marker it will
    /// publish; records the coordination implementation; and only then publishes the new marker
    /// atomically and makes it durable, which commits the migration. It then removes the old marker
    /// -- so an older binary finds no marker at all and refuses the root -- and the record. A
    /// migration interrupted at any point leaves a root that is not opened and that running
    /// `migrate` again finishes; one that finds a migration record resumes it if it is for the same
    /// implementation and identical agent inputs, or reports [`StoreError::MigrationInputsDiffer`].
    /// A version-7 root keeps its existing coordination reservation; a version-6 root creates one.
    ///
    /// Returns [`Migration::AlreadyCurrent`] for a root that is migrated and has nothing left to
    /// finish, and [`Migration::Migrated`] when this call did work, a resumed migration included.
    pub fn migrate(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        options: &StoreOptions,
    ) -> Result<Migration, StoreError> {
        let root = Self::absolute_root(root)?;
        let factory = options.coordination();
        if let Found::Current(bytes) = Self::find(&vfs, &root)? {
            Self::parse_genesis(&vfs, &root, &bytes)?
                .metadata
                .check_identity(options.agent())?;
            let participant = factory.open(Arc::clone(&vfs), &root)?;
            participant.confirm_created(MARKER_NAME)?;
            return Ok(Migration::AlreadyCurrent);
        }
        let _lock = Self::lock_root(&vfs, &root)?;
        let record = match Self::find(&vfs, &root)? {
            Found::Current(bytes) => {
                Self::parse_genesis(&vfs, &root, &bytes)?
                    .metadata
                    .check_identity(options.agent())?;
                factory
                    .open(Arc::clone(&vfs), &root)?
                    .confirm_created(MARKER_NAME)?;
                return Ok(Migration::AlreadyCurrent);
            }
            Found::Interrupted(record) => {
                record.check(&root, &*factory, options.agent())?;
                Self::validate_genesis(&vfs, &root, &record.genesis)?;
                *record
            }
            Found::Legacy(bytes) => {
                let record =
                    Self::begin_migration(&vfs, &root, &bytes, &*factory, options.agent())?;
                let bytes = serde_json::to_vec_pretty(&record)?;
                Self::publish_durably(&vfs, &root, MIGRATION_RECORD_NAME, &bytes)?;
                record
            }
        };
        Self::finish_migration(&vfs, &root, &*factory, &record)?;
        Ok(Migration::Migrated)
    }

    /// Reads and validates the marker of a root this build may open.
    ///
    /// An absent marker is [`StoreError::NotAStore`], marker bytes that are not the expected
    /// JSON are [`StoreError::Json`], a root at [`MIGRATABLE_FORMAT_VERSION`] or one whose
    /// migration was begun and not finished is [`StoreError::MigrationRequired`], any other
    /// version this build does not write is [`StoreError::UnsupportedFormatVersion`], and a layout
    /// inconsistent with what is on disk is [`StoreError::MalformedMarker`] or
    /// [`StoreError::MissingPartition`].
    pub(super) fn read_genesis(vfs: &Arc<dyn Vfs>, root: &Path) -> Result<Genesis, StoreError> {
        match Self::find(vfs, root)? {
            Found::Current(bytes) => Self::parse_genesis(vfs, root, &bytes),
            Found::Legacy(bytes) => Err(StoreError::MigrationRequired {
                root: root.to_path_buf(),
                found: Self::probe_version(&bytes)?,
                expected: GENESIS_FORMAT_VERSION,
            }),
            Found::Interrupted(record) => Err(StoreError::MigrationRequired {
                root: root.to_path_buf(),
                found: record.inputs.from_version,
                expected: GENESIS_FORMAT_VERSION,
            }),
        }
    }

    /// The marker's genesis, checked against what is on disk.
    fn parse_genesis(vfs: &Arc<dyn Vfs>, root: &Path, bytes: &[u8]) -> Result<Genesis, StoreError> {
        let genesis: Genesis = serde_json::from_slice(bytes)?;
        Self::validate_genesis(vfs, root, &genesis)?;
        Ok(genesis)
    }

    /// Checks `genesis` against what is on disk: its counts and shard width are in range and the
    /// namespace partitions it names are directories.
    pub(super) fn validate_genesis(
        vfs: &Arc<dyn Vfs>,
        root: &Path,
        genesis: &Genesis,
    ) -> Result<(), StoreError> {
        let root = root.to_path_buf();

        if genesis
            .metadata
            .transition
            .as_ref()
            .is_some_and(|transition| !transition.valid(&genesis.metadata))
        {
            return Err(StoreError::MalformedMarker {
                root,
                reason: "an invalid schema upgrade announcement",
            });
        }

        if genesis.metadata.canopy_schema.oldest_reader > genesis.metadata.canopy_schema.current
            || genesis.metadata.agent_schema.oldest_reader > genesis.metadata.agent_schema.current
        {
            return Err(StoreError::MalformedMarker {
                root,
                reason: "an oldest reader schema exceeds its layer's current schema",
            });
        }

        if genesis.blob_replica_count == 0 || genesis.blob_replica_count > MAX_REPLICA_COUNT {
            return Err(StoreError::MalformedMarker {
                root,
                reason: "recorded blob replica count is not a positive number within the supported range",
            });
        }
        if genesis.pointer_replica_count == 0
            || genesis.pointer_replica_count.is_multiple_of(2)
            || genesis.pointer_replica_count > MAX_REPLICA_COUNT
        {
            return Err(StoreError::MalformedMarker {
                root,
                reason: "recorded pointer replica count is not a positive odd number within the supported range",
            });
        }
        if genesis.shard_width == 0 || genesis.shard_width > MAX_SHARD_WIDTH {
            return Err(StoreError::MalformedMarker {
                root,
                reason: "recorded shard width is not between 1 and the supported maximum",
            });
        }

        for index in 0..genesis.pointer_replica_count {
            let partition = namespace_partition_root(&root, index);
            match vfs.metadata(&partition) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(StoreError::MalformedMarker {
                        root,
                        reason: "a partition path is not a directory",
                    });
                }
                Err(e) if e.kind() == VfsErrorKind::NotFound => {
                    return Err(StoreError::MissingPartition { root, index });
                }
                Err(e) => return Err(StoreError::Vfs(e)),
            }
        }
        Ok(())
    }
}

/// The identity a creation's reservation records and every creator compares: what a store is, and
/// what an opener must also match. Provenance -- the creation time, the agent's version,
/// description and schemas -- is recorded by the marker alone. The encoding is the reservation's
/// inputs, so changing a field, its name or their order changes which interrupted creations a build
/// resumes.
#[derive(Serialize)]
struct CreationIdentity<'genesis> {
    format_version: u32,
    store_id: &'genesis str,
    blob_replica_count: usize,
    pointer_replica_count: usize,
    shard_width: usize,
    agent_identifier: &'genesis str,
}

impl Genesis {
    /// The canonical encoding of this genesis's identity: one identity, one byte string.
    fn creation_inputs(&self) -> Result<Vec<u8>, StoreError> {
        Ok(serde_json::to_vec(&CreationIdentity {
            format_version: self.format_version,
            store_id: &self.store_id,
            blob_replica_count: self.blob_replica_count,
            pointer_replica_count: self.pointer_replica_count,
            shard_width: self.shard_width,
            agent_identifier: &self.metadata.agent_identifier,
        })?)
    }
}

/// Makes a store's artifacts for the creation contract -- the blob tree, the namespace tree, then
/// the marker -- each idempotently, since a resumed creation can find one partly made.
struct GenesisMaker<'a> {
    vfs: &'a Arc<dyn Vfs>,
    root: &'a Path,
    genesis: &'a Genesis,
    marker: &'a [u8],
    /// A participant that lives until the store holds its own coordination, since state a
    /// participant vouches for need not outlive the last participant open on the root.
    vouching: &'a dyn Coordination,
}

impl GenesisMaker<'_> {
    /// Vouches for the coordination state of a root that has nothing to recover. It runs before
    /// the marker exists, so no handle can be attached to the root to have done work since.
    fn vouch(&self) -> CoordResult<()> {
        let mut hold = self.vouching.acquire(Waiting::Wait)?;
        hold.begin_modification(Purpose::Recovery)?;
        hold.end_modification(ModificationEnd::Settled)?;
        hold.complete_recovery()
    }
}

impl ArtifactMaker for GenesisMaker<'_> {
    fn make(&mut self, name: &str, _inputs: &[u8]) -> CoordResult<()> {
        if name == BLOB_TREE_DIR {
            // The blob tree's partitions, whose count is independent of the namespace's
            // (`D316`, `D320`). Shard directories under them are created lazily on first
            // ingest, so an empty store carries 1024 absent directories rather than 1024
            // empty ones.
            for index in 0..self.genesis.blob_replica_count {
                let partition = blob_partition_root(self.root, index);
                self.vfs.create_dir_all(&partition)?;
                self.vfs.fsync(&partition)?;
            }
            self.vfs.fsync(&self.root.join(BLOB_TREE_DIR))?;
        } else if name == NAMESPACE_TREE_DIR {
            // The namespace's partitions, whose count is odd and independent of the blob
            // tree's (`D320`). Node directories under them are created by the first write that
            // needs one, exactly as the blob tree's shards are.
            for index in 0..self.genesis.pointer_replica_count {
                let partition = namespace_partition_root(self.root, index);
                self.vfs.create_dir_all(&partition)?;
                self.vfs.fsync(&partition)?;
            }
            self.vfs.fsync(&self.root.join(NAMESPACE_TREE_DIR))?;
        }
        Ok(())
    }

    /// The marker the coordination publishes atomically and last (D40), once the root's
    /// coordination state is vouched for.
    fn marker(&mut self, _name: &str, _inputs: &[u8]) -> CoordResult<Vec<u8>> {
        self.vouch()?;
        Ok(self.marker.to_vec())
    }
}

/// What [`Store::migrate`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Migration {
    /// A supported older root reached [`GENESIS_FORMAT_VERSION`], including a resumed migration.
    Migrated,
    /// The root was already at [`GENESIS_FORMAT_VERSION`], and nothing was changed.
    AlreadyCurrent,
}
