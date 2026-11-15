// Copyright (c) Michael Grier.

use super::*;
use acadia::coord::{Coordination, CoordinationFactory, ImplementationId};
use std::ffi::OsStr;

/// Store-level metadata read together from one atomically published marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreProperties {
    pub agent_identifier: String,
    pub agent_version: String,
    pub agent_description: String,
    pub coordination_implementation: ImplementationId,
    pub canopy_schema: SchemaVersions,
    pub agent_schema: SchemaVersions,
    pub schema_upgrade: Option<super::UpgradeRequest>,
    pub store_id: String,
    pub created_at_unix_ms: u64,
    pub genesis_format_version: u32,
    pub blob_replica_count: usize,
    pub pointer_replica_count: usize,
    pub shard_width: usize,
}

fn properties(
    vfs: &Arc<dyn Vfs>,
    root: &Path,
    implementation: &ImplementationId,
) -> Result<StoreProperties, StoreError> {
    let genesis = Store::read_admission_genesis(vfs, root)?;
    Ok(StoreProperties {
        agent_identifier: genesis.metadata.agent_identifier,
        agent_version: genesis.metadata.agent_version,
        agent_description: genesis.metadata.agent_description,
        coordination_implementation: implementation.clone(),
        canopy_schema: genesis.metadata.canopy_schema,
        agent_schema: genesis.metadata.agent_schema,
        schema_upgrade: genesis
            .metadata
            .transition
            .map(|transition| transition.request),
        store_id: genesis.store_id,
        created_at_unix_ms: genesis.created_at_unix_ms,
        genesis_format_version: genesis.format_version,
        blob_replica_count: genesis.blob_replica_count,
        pointer_replica_count: genesis.pointer_replica_count,
        shard_width: genesis.shard_width,
    })
}

/// A view that takes no attachment tier and exposes no document reads or mutations.
///
/// ```compile_fail
/// fn read(view: &canopy::DiagnosticView) {
///     view.read_document("docs", &canopy::DocumentName::new("one").unwrap());
/// }
/// ```
/// ```compile_fail
/// fn mutate(view: &canopy::DiagnosticView) {
///     view.set_agent_description("not permitted");
/// }
/// ```
pub struct DiagnosticView {
    vfs: Arc<dyn Vfs>,
    root: PathBuf,
    participant: Box<dyn Coordination>,
}

impl fmt::Debug for DiagnosticView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticView")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl DiagnosticView {
    /// Lists attached handles without taking an attachment tier or waiting for admission.
    pub fn attached_handles(&self) -> Result<AttachedHandles, StoreError> {
        Ok(super::agent::attached_handles(self.participant.list()?))
    }

    /// Opens a diagnostic view through the default coordination implementation, without an agent.
    pub fn open(vfs: Arc<dyn Vfs>, root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_with_coordination(vfs, root, super::surface::default_coordination())
    }

    /// Builds coordination from this view's filesystem and root and checks its recorded identity.
    pub fn open_with_coordination(
        vfs: Arc<dyn Vfs>,
        root: impl Into<PathBuf>,
        factory: Arc<dyn CoordinationFactory>,
    ) -> Result<Self, StoreError> {
        let root = root.into();
        Store::read_genesis(&vfs, &root)?;
        let participant = factory.open(Arc::clone(&vfs), &root)?;
        participant.confirm_created(OsStr::new(MARKER_NAME))?;
        Store::read_genesis(&vfs, &root)?;
        Ok(Self {
            vfs,
            root,
            participant,
        })
    }

    /// Reads fresh metadata without waiting for admission or writer exclusion.
    pub fn properties(&self) -> Result<StoreProperties, StoreError> {
        properties(&self.vfs, &self.root, self.participant.implementation())
    }

    /// Observes recovery status without resolving anything or interpreting a live writer's work list.
    pub fn recovery_need(&self) -> Result<RecoveryNeed, StoreError> {
        let mut work = None;
        let need = self.participant.discover_recovery_with(&mut || {
            work = Some(Store::root_has_unresolved_work(&self.vfs, &self.root));
        })?;
        let mut observed = RecoveryNeed::observed(need);
        observed.set_unresolved_work(work.transpose()?);
        Ok(observed)
    }
}

impl Store {
    /// Lists attached handles, retaining unknown entries and possible incompleteness.
    pub fn attached_handles(&self) -> Result<AttachedHandles, StoreError> {
        let _lease = self.operation_lease()?;
        Ok(super::agent::attached_handles(self.coordination.list()?))
    }

    /// Reads fresh store metadata rather than this handle's open-time marker.
    pub fn properties(&self) -> Result<StoreProperties, StoreError> {
        let _lease = self.operation_lease()?;
        properties(&self.vfs, &self.root, self.coordination.implementation())
    }
}
