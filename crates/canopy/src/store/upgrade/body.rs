// Copyright (c) Michael Grier.

use super::*;

#[cfg(test)]
mod tests;

thread_local! {
    static UNIT_RUNNING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct UnitCall;

impl UnitCall {
    fn enter() -> Self {
        UNIT_RUNNING.with(|running| running.set(true));
        Self
    }
}

impl Drop for UnitCall {
    fn drop(&mut self) {
        UNIT_RUNNING.with(|running| running.set(false));
    }
}

/// The root record of migration intent and completed units. Changing its name breaks the layout.
pub const UPGRADE_INTENT_NAME: &str = ".schema-upgrade";

mod intent_tags {
    // Changing this tag breaks the intent encoding.
    pub const FORMAT_VERSION: u32 = 1;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Intent {
    format_version: u32,
    pub transition: Transition,
    pub units: u64,
    pub next_unit: u64,
}

impl Intent {
    pub(super) fn new(transition: Transition, units: u64) -> Self {
        Self {
            format_version: intent_tags::FORMAT_VERSION,
            transition,
            units,
            next_unit: 0,
        }
    }

    fn valid(&self) -> bool {
        self.format_version == intent_tags::FORMAT_VERSION
            && self.next_unit <= self.units
            && UpgradeRequest::new(
                self.transition.request.layer,
                self.transition.request.source,
                self.transition.request.target,
                self.transition.request.oldest_reader,
            )
            .is_ok()
    }

    pub(super) fn read(store: &Store) -> Result<Option<Self>, StoreError> {
        Self::read_at(&store.vfs, &store.root)
    }

    pub(super) fn read_at(vfs: &Arc<dyn Vfs>, root: &Path) -> Result<Option<Self>, StoreError> {
        let bytes = match vfs.read(&root.join(UPGRADE_INTENT_NAME)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == VfsErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let intent: Self = serde_json::from_slice(&bytes)?;
        if !intent.valid() {
            return Err(StoreError::InvalidUpgrade {
                reason: "an invalid migration intent or unit cursor",
            });
        }
        Ok(Some(intent))
    }

    pub(super) fn write(&self, store: &Store) -> Result<(), StoreError> {
        if !self.valid() {
            return Err(StoreError::InvalidUpgrade {
                reason: "an invalid migration intent or unit cursor",
            });
        }
        Store::publish_durably(
            &store.vfs,
            &store.root,
            UPGRADE_INTENT_NAME,
            &serde_json::to_vec_pretty(self)?,
        )
    }

    pub(super) fn clear(store: &Store) -> Result<(), StoreError> {
        match store.vfs.remove_file(&store.root.join(UPGRADE_INTENT_NAME)) {
            Ok(()) => {}
            Err(error) if error.kind() == VfsErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        store.vfs.fsync(&store.root)?;
        Ok(())
    }
}

/// Durable progress of caller-driven migration units; schema publication follows the final unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpgradeProgress {
    pub request: UpgradeRequest,
    pub units: u64,
    pub next_unit: u64,
}

/// One idempotent migration unit, run under the upgrade's exclusion or recovery's writer guard.
/// The caller must not wait for another writer or perform non-idempotent external effects.
pub struct MigrationUnit<'store> {
    store: &'store Store,
    progress: UpgradeProgress,
    stamp: std::cell::RefCell<Result<(), StoreError>>,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl MigrationUnit<'_> {
    pub fn progress(&self) -> UpgradeProgress {
        self.progress
    }

    pub fn read_document(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
    ) -> Result<crate::Read, StoreError> {
        self.store.read_document(node, document)
    }

    pub fn list_documents(&self) -> Result<Vec<(NodePath, DocumentName)>, StoreError> {
        self.store.list_documents()
    }

    pub fn read_blob(&self, reference: &BlobRef) -> Result<super::super::BlobRead, StoreError> {
        self.store.read_blob(reference)
    }

    /// Applies a transaction and retains its independent stamp error in the unit's receipt.
    pub fn apply_transaction(&self, swaps: &SwapSet) -> Result<Commit, StoreError> {
        self.receipt(self.store.apply_transaction(swaps)?)
    }

    pub fn write_document(
        &self,
        node: impl Into<NodePath>,
        document: &DocumentName,
        body: &DocumentBody,
    ) -> Result<(), StoreError> {
        self.receipt(
            self.store
                .write_document_replicated(node, document, body)?
                .map(|_| ()),
        )
    }

    fn receipt<T>(&self, receipt: WriteOutcome<T>) -> Result<T, StoreError> {
        let mut stamp = self.stamp.borrow_mut();
        if stamp.is_ok() {
            *stamp = receipt.agent_stamp;
        }
        Ok(receipt.value)
    }
}

pub(super) struct MigrationScope<'store>(&'store Coordinated, bool);

impl<'store> MigrationScope<'store> {
    fn enter(coordination: &'store Coordinated) -> Self {
        let previous = coordination.is_recovering();
        coordination.set_recovering(true);
        Self(coordination, previous)
    }
}

impl Drop for MigrationScope<'_> {
    fn drop(&mut self) {
        self.0.set_recovering(self.1);
    }
}

struct RestoreRefusal {
    writer: Arc<super::super::writer::WriterGate>,
    restore: bool,
}

impl Drop for RestoreRefusal {
    fn drop(&mut self) {
        if self.restore {
            self.writer.mark_unresolved();
        }
    }
}

impl UpgradeDrain {
    /// Declares a resumable agent-schema body before acquiring its tiers and publishing intent.
    /// Undefined Canopy transitions are refused; Canopy units belong to the store, not the caller.
    pub fn begin_migration(
        &self,
        request: UpgradeRequest,
        units: u64,
    ) -> Result<SchemaUpgrade, StoreError> {
        if request.layer == SchemaLayer::Canopy {
            return Err(StoreError::InvalidUpgrade {
                reason: "no store-owned Canopy migration is defined for this transition",
            });
        }
        let upgrade = self.begin_conversion(request)?;
        upgrade
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .body_units = Some(units);
        Ok(upgrade)
    }
}

impl SchemaUpgrade {
    /// Runs one idempotent caller unit. A failed unit leaves its cursor unchanged and fences the body.
    pub fn step(
        &self,
        unit: impl FnOnce(&MigrationUnit<'_>) -> Result<(), StoreError>,
    ) -> Result<WriteOutcome<UpgradeProgress>, StoreError> {
        coordinated::leases::Leases::control_context()
            .map_err(|error| drain::refusal(&self.store.root, error))?;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.active || state.finished || state.body_units.is_none() {
            return Err(StoreError::UpgradeBodyRequired {
                root: self.store.root.clone(),
            });
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _lease = self
                .store
                .coordination
                .leases()
                .enter_migration()
                .map(|lease| lease.keeping(Arc::clone(&self.store.coordination)))
                .map_err(|error| drain::refusal(&self.store.root, error))?;
            let _writer = self.store.writer.enter();
            let _scope = MigrationScope::enter(&self.store.coordination);
            self.store.run_upgrade_unit(unit, false)
        }));
        match result {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(error)) => {
                self.abandon(&mut state);
                Err(error)
            }
            Err(panic) => {
                self.abandon(&mut state);
                std::panic::resume_unwind(panic)
            }
        }
    }

    pub fn progress(&self) -> Result<UpgradeProgress, StoreError> {
        coordinated::leases::Leases::control_context()
            .map_err(|error| drain::refusal(&self.store.root, error))?;
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let intent = match Intent::read(&self.store)? {
            Some(intent) => intent,
            None if state.finished
                && state.body_units.is_some()
                && schema(
                    &Store::read_genesis(&self.store.vfs, &self.store.root)?.metadata,
                    self.request.layer,
                )
                .current
                    >= self.request.target =>
            {
                let units = state
                    .body_units
                    .expect("the completed body declared its units");
                return Ok(UpgradeProgress {
                    request: self.request,
                    units,
                    next_unit: units,
                });
            }
            None => {
                return Err(StoreError::UpgradeBodyRequired {
                    root: self.store.root.clone(),
                });
            }
        };
        Ok(UpgradeProgress {
            request: intent.transition.request,
            units: intent.units,
            next_unit: intent.next_unit,
        })
    }
}

impl Store {
    pub(crate) fn refuse_unit_control(&self) -> Result<(), StoreError> {
        if UNIT_RUNNING.with(|running| running.get()) {
            return Err(StoreError::UpgradeFromOperation {
                root: self.root.clone(),
            });
        }
        Ok(())
    }

    pub(crate) fn upgrade_units_remaining(&self) -> Result<u64, StoreError> {
        Ok(Intent::read(self)?.map_or(0, |intent| intent.units - intent.next_unit))
    }

    pub(crate) fn run_upgrade_unit(
        &self,
        unit: impl FnOnce(&MigrationUnit<'_>) -> Result<(), StoreError>,
        check_target: bool,
    ) -> Result<WriteOutcome<UpgradeProgress>, StoreError> {
        let mut intent = Intent::read(self)?.ok_or_else(|| StoreError::UpgradeBodyRequired {
            root: self.root.clone(),
        })?;
        if intent.transition.request.layer == SchemaLayer::Canopy {
            return Err(StoreError::InvalidUpgrade {
                reason: "no store-owned Canopy migration is defined for this transition",
            });
        }
        if check_target {
            self.check_upgrade_admission(&self.intent_admission()?.metadata)?;
        }
        let progress = UpgradeProgress {
            request: intent.transition.request,
            units: intent.units,
            next_unit: intent.next_unit,
        };
        if intent.next_unit == intent.units {
            return Ok(WriteOutcome::new(progress, Ok(())));
        }
        self.resolve_data_work()?;
        let _refusal = RestoreRefusal {
            writer: Arc::clone(&self.writer),
            restore: self.writer.is_unresolved(),
        };
        self.writer.clear_unresolved();
        let context = MigrationUnit {
            store: self,
            progress,
            stamp: std::cell::RefCell::new(Ok(())),
            _not_send: std::marker::PhantomData,
        };
        {
            let _call = UnitCall::enter();
            unit(&context)?;
        }
        if self.writer.is_unresolved() {
            return Err(StoreError::RecoveryRequired {
                root: self.root.clone(),
            });
        }
        self.resolve_data_work()?;
        intent.next_unit += 1;
        intent.write(self)?;
        let mut stamp = context.stamp.into_inner();
        self.accumulate_stamp(&mut stamp);
        Ok(WriteOutcome::new(
            UpgradeProgress {
                next_unit: intent.next_unit,
                ..progress
            },
            stamp,
        ))
    }
}
