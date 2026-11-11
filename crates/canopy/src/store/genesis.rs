// Copyright (c) Michael Grier.
// Split from store.rs at 1303f9a.

//! The `init` / `open` paths: writing the genesis marker replicas over an empty
//! root, and reading them back into a live store (LAY-1).
//!
//! The genesis types themselves (`ShireId`, `GenesisConfig`, `Genesis`) stay in
//! the parent module: `Store` holds a `Genesis` field, so the cluster is bound to
//! the struct definition.

use super::*;
use crate::store::blob::blob_partition_root;
use crate::store::namespace::namespace_partition_root;

impl Store {
    /// Takes the root's exclusive-writer lock (`D116`), naming contention as
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

    /// Creates a new store under `root`, writing the genesis marker and the
    /// `N` empty partition replicas (D40/LAY-1).
    ///
    /// `root` must already exist as a directory and hold no store: it must be
    /// empty apart from ignorable dotfiles, and in particular must not already
    /// contain a [`.shire`](MARKER_NAME) marker. The genesis is serialized once
    /// and mirrored into every partition; the root marker is written **last**
    /// (atomically, via a temp sibling and rename) so a crash mid-init leaves no
    /// marker and [`open`](Store::open) cleanly refuses the partial store rather
    /// than adopting it.
    ///
    /// Takes the root's exclusive-writer lock (`D116`) and holds it for the
    /// returned store's life; a root another process already holds is refused
    /// with [`StoreError::RootInUse`].
    ///
    /// `root` must be on a local filesystem; a networked root is unsupported and not
    /// detected (see [`Store`]).
    ///
    /// `options` supplies the nonce source the write verbs draw from; see [`StoreOptions`].
    pub fn init(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        config: GenesisConfig,
        options: StoreOptions,
    ) -> Result<Self, StoreError> {
        let store = Self::init_unsourced(vfs, root, config)?;
        store.install(options);
        Ok(store)
    }

    /// [`init`](Store::init) with no nonce source, for tests that name every nonce themselves.
    pub(crate) fn init_unsourced(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        config: GenesisConfig,
    ) -> Result<Self, StoreError> {
        let root = root.into();

        match vfs.metadata(&root) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(StoreError::RootNotADirectory { root }),
            Err(e) if e.kind() == VfsErrorKind::NotFound => {
                return Err(StoreError::RootNotFound { root });
            }
            Err(e) => return Err(StoreError::Vfs(e)),
        }

        let refuse_occupied = |root: &PathBuf| -> Result<(), StoreError> {
            for entry in vfs.read_dir(root)? {
                let name = entry.file_name;
                if name == MARKER_NAME {
                    return Err(StoreError::AlreadyInitialized { root: root.clone() });
                }
                if !name.to_string_lossy().starts_with('.') {
                    return Err(StoreError::NotEmpty {
                        root: root.clone(),
                        entry: name,
                    });
                }
            }
            Ok(())
        };

        // Scanned twice. Once before locking, so a root that is plainly not ours is refused
        // without leaving a lock file in it. Again under the lock, because a scan taken
        // without it can pass and then lose the root to another `init` that completes and
        // releases it, and this one would write over that store.
        refuse_occupied(&root)?;
        let lock = Self::lock_root(&vfs, &root)?;
        refuse_occupied(&root)?;

        let genesis = Genesis::from_config(config);
        let bytes = serde_json::to_vec_pretty(&genesis)?;

        // Additive first: build both trees before the root marker exists.
        //
        // The blob tree's partitions, whose count is independent of the namespace's
        // (`D316`, `D320`). Shard directories under them are created lazily on first
        // ingest, so an empty store carries 1024 absent directories rather than 1024
        // empty ones.
        for index in 0..genesis.blob_replica_count {
            let partition = blob_partition_root(&root, index);
            vfs.create_dir_all(&partition)?;
            vfs.fsync(&partition)?;
        }
        vfs.fsync(&root.join(BLOB_TREE_DIR))?;

        // The namespace's partitions, whose count is odd and independent of the blob
        // tree's (`D320`). Node directories under them are created by the first write that
        // needs one, exactly as the blob tree's shards are.
        for index in 0..genesis.pointer_replica_count {
            let partition = namespace_partition_root(&root, index);
            vfs.create_dir_all(&partition)?;
            vfs.fsync(&partition)?;
        }
        vfs.fsync(&root.join(NAMESPACE_TREE_DIR))?;

        // Commit: the root marker appears atomically and last (D40).
        let marker = root.join(MARKER_NAME);
        let tmp = root.join(format!("{MARKER_NAME}.tmp"));
        vfs.write(&tmp, &bytes)?;
        vfs.fsync(&tmp)?;
        vfs.rename(&tmp, &marker)?;
        vfs.fsync(&root)?;

        Ok(Store {
            vfs,
            root,
            genesis,
            blob_reads: Arc::default(),
            pointer_reads: Arc::default(),
            modifications: Arc::default(),
            repair_backlog: Arc::default(),
            writer: Arc::default(),
            log_durability: Arc::default(),
            unsynced_dirs: Arc::default(),
            _lock: lock,
            surface: Arc::default(),
        })
    }

    /// Opens the existing store under `root` (D40/LAY-1).
    ///
    /// Reads and validates the [`.shire`](MARKER_NAME) marker: an absent marker
    /// is [`StoreError::NotAShire`], marker bytes that are not the expected JSON
    /// are [`StoreError::Json`], a marker written by an unknown genesis version is
    /// [`StoreError::UnsupportedFormatVersion`], and a marker whose recorded
    /// layout is inconsistent with what is on disk (a bad shard width or replica
    /// count, a missing or non-directory partition) is
    /// [`StoreError::MalformedMarker`] / [`StoreError::MissingPartition`]. Only a
    /// fully valid Shire yields a handle.
    ///
    /// `root` must be on a local filesystem; a networked root is unsupported and not
    /// detected (see [`Store`]).
    ///
    /// `options` supplies the nonce source the write verbs draw from; see [`StoreOptions`].
    pub fn open(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        options: StoreOptions,
    ) -> Result<Self, StoreError> {
        let store = Self::open_unsourced(vfs, root)?;
        store.install(options);
        Ok(store)
    }

    /// [`open`](Store::open) with no nonce source, for tests that name every nonce themselves.
    pub(crate) fn open_unsourced(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
    ) -> Result<Self, StoreError> {
        let root = root.into();
        let marker = root.join(MARKER_NAME);

        let bytes = match vfs.read(&marker) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == VfsErrorKind::NotFound => {
                return Err(StoreError::NotAShire { root });
            }
            Err(e) => return Err(StoreError::Vfs(e)),
        };

        // The version decides refusal before the marker is read as this build's shape
        // (`D325`): a foreign version must never be reported as malformed JSON just because
        // its field set differs from the current one.
        let probe: GenesisVersion = serde_json::from_slice(&bytes)?;
        if probe.format_version != GENESIS_FORMAT_VERSION {
            return Err(StoreError::UnsupportedFormatVersion {
                found: probe.format_version,
                expected: GENESIS_FORMAT_VERSION,
            });
        }

        let genesis: Genesis = serde_json::from_slice(&bytes)?;

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

        // Validation is read-only, so it runs first: a directory that turns out
        // not to be a store never gets a lock file dropped into it. Acquisition
        // is still the serialization point -- two concurrent opens can both pass
        // validation, and exactly one gets the lock.
        let lock = Self::lock_root(&vfs, &root)?;

        let store = Store {
            vfs,
            root,
            genesis,
            blob_reads: Arc::default(),
            pointer_reads: Arc::default(),
            modifications: Arc::default(),
            repair_backlog: Arc::default(),
            writer: Arc::default(),
            log_durability: Arc::default(),
            unsynced_dirs: Arc::default(),
            _lock: lock,
            surface: Arc::default(),
        };
        // Recovery replays the D53 log and writes, so it must happen under the
        // lock rather than before it.
        store.discharge_recorded_dirs()?;
        store.recover()?;
        store.release_stale_claims()?;
        Ok(store)
    }
}
