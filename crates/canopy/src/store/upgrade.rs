// Copyright (c) Michael Grier.

use super::*;
use acadia::coord::{Access, ReaderSafety, Upgrade, UpgradeConversion};

#[cfg(test)]
mod tests;

mod body;
pub use body::{MigrationUnit, UPGRADE_INTENT_NAME, UpgradeProgress};

/// One layer's source, target and the oldest reader that can read the upgrade's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeRequest {
    pub layer: SchemaLayer,
    pub source: u32,
    pub target: u32,
    pub oldest_reader: u32,
}

impl UpgradeRequest {
    pub fn new(
        layer: SchemaLayer,
        source: u32,
        target: u32,
        oldest_reader: u32,
    ) -> Result<Self, StoreError> {
        if target <= source || oldest_reader > target {
            return Err(StoreError::InvalidUpgrade {
                reason: "the target must advance and the reader floor must not exceed it",
            });
        }
        Ok(Self {
            layer,
            source,
            target,
            oldest_reader,
        })
    }

    fn check(
        self,
        metadata: &agent::AgentMetadata,
        root: &Path,
    ) -> Result<ReaderSafety, StoreError> {
        Self::new(self.layer, self.source, self.target, self.oldest_reader)?;
        if metadata.transition.is_some() || schema(metadata, self.layer).current != self.source {
            return Err(StoreError::UpgradeChanged {
                root: root.to_path_buf(),
            });
        }
        Ok(
            if self.oldest_reader <= schema(metadata, self.layer).oldest_reader {
                ReaderSafety::Safe
            } else {
                ReaderSafety::Unsafe
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Transition {
    pub request: UpgradeRequest,
    pub safe: bool,
}

impl Transition {
    pub(super) fn valid(&self, metadata: &agent::AgentMetadata) -> bool {
        self.request.target > self.request.source
            && self.request.oldest_reader <= self.request.target
            && schema(metadata, self.request.layer).current == self.request.source
            && self.safe
                == (self.request.oldest_reader
                    <= schema(metadata, self.request.layer).oldest_reader)
    }
}

fn schema(metadata: &agent::AgentMetadata, layer: SchemaLayer) -> &SchemaVersions {
    match layer {
        SchemaLayer::Canopy => &metadata.canopy_schema,
        SchemaLayer::Agent => &metadata.agent_schema,
    }
}

fn advance(genesis: &mut Genesis, transition: &Transition) {
    let schema = match transition.request.layer {
        SchemaLayer::Canopy => &mut genesis.metadata.canopy_schema,
        SchemaLayer::Agent => &mut genesis.metadata.agent_schema,
    };
    schema.current = transition.request.target;
    if !transition.safe {
        schema.oldest_reader = transition.request.target;
    }
    genesis.metadata.transition = None;
}

struct State {
    conversion: Option<Box<dyn UpgradeConversion>>,
    upgrade: Option<Box<dyn Upgrade>>,
    safety: Option<ReaderSafety>,
    active: bool,
    finished: bool,
    registration_error: Option<Arc<StoreError>>,
    body_units: Option<u64>,
}

/// A nonblocking conversion and schema-only upgrade. Its source handle remains detached afterwards.
#[must_use = "dropping the conversion cancels it and leaves the source attachment detached"]
pub struct SchemaUpgrade {
    store: Store,
    request: UpgradeRequest,
    state: Mutex<State>,
}

impl fmt::Debug for SchemaUpgrade {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SchemaUpgrade")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

impl SchemaUpgrade {
    /// Advances acquisition without blocking. True means the upgrade is ready to complete.
    pub fn poll(&self) -> Result<bool, StoreError> {
        coordinated::leases::Leases::control_context()
            .map_err(|error| drain::refusal(&self.store.root, error))?;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.finished {
            return Err(StoreError::UpgradeCancelled {
                root: self.store.root.clone(),
            });
        }
        if state.active {
            return Ok(true);
        }
        let result = self.prepare(&mut state);
        if result.is_err() {
            self.abandon(&mut state);
        }
        result
    }

    fn prepare(&self, state: &mut State) -> Result<bool, StoreError> {
        if state.upgrade.is_none() {
            let conversion = state
                .conversion
                .as_mut()
                .expect("an unfinished conversion exists");
            if !conversion.poll_gate()? {
                return Ok(false);
            }
            if state.safety.is_none() {
                let genesis = Store::read_genesis(&self.store.vfs, &self.store.root)?;
                self.store.check_upgrade_admission(&genesis.metadata)?;
                state.safety = Some(self.request.check(&genesis.metadata, &self.store.root)?);
            }
            let safety = state.safety.expect("the gate protects the safety judgment");
            let Some(upgrade) = conversion.poll_tiers(safety)? else {
                return Ok(false);
            };
            state.upgrade = Some(upgrade);
            state.conversion = None;
        }
        let safety = state.safety.expect("the gate protects the safety judgment");
        let mut genesis = Store::read_genesis(&self.store.vfs, &self.store.root)?;
        self.store.check_upgrade_admission(&genesis.metadata)?;
        if self.request.check(&genesis.metadata, &self.store.root)? != safety {
            return Err(StoreError::UpgradeChanged {
                root: self.store.root.clone(),
            });
        }
        let (acquired, _) = match self.store.coordination.try_acquire() {
            Ok(acquired) => acquired,
            Err(StoreError::Coordination(acadia::coord::CoordError::WouldWait)) => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        if acquired.recovery_required() {
            self.store.coordination.release();
            return Err(StoreError::RecoveryRequired {
                root: self.store.root.clone(),
            });
        }
        self.store.coordination.begin(&self.store.root)?;
        state.active = true;
        genesis.metadata.transition = Some(Transition {
            request: self.request,
            safe: safety == ReaderSafety::Safe,
        });
        if let Some(units) = state.body_units {
            body::Intent::new(
                genesis
                    .metadata
                    .transition
                    .clone()
                    .expect("the announced transition"),
                units,
            )
            .write(&self.store)?;
        }
        Store::publish_durably(
            &self.store.vfs,
            &self.store.root,
            MARKER_NAME,
            &serde_json::to_vec_pretty(&genesis)?,
        )?;
        state
            .upgrade
            .as_mut()
            .expect("the upgrade retains its tiers")
            .admit_readers()?;
        let upgrade = state
            .upgrade
            .as_mut()
            .expect("the upgrade retains its tiers");
        let registration = self
            .store
            .upgrade_registration(upgrade.tiers())
            .and_then(|payload| {
                upgrade
                    .register(&payload)
                    .map(|_| ())
                    .map_err(StoreError::from)
            });
        state.registration_error = registration.err().map(Arc::new);
        state.conversion = None;
        Ok(true)
    }

    /// Atomically advances the selected layer, preserving every other marker field.
    /// The prepared guard retains writer exclusion; do not wait on another writer from its holder.
    pub fn complete(&self) -> Result<WriteOutcome<()>, StoreError> {
        coordinated::leases::Leases::control_context()
            .map_err(|error| drain::refusal(&self.store.root, error))?;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.active || state.finished {
            return Err(StoreError::UpgradeChanged {
                root: self.store.root.clone(),
            });
        }
        if let Some(units) = state.body_units {
            let intent = match body::Intent::read(&self.store) {
                Ok(Some(intent))
                    if intent.units == units && intent.transition.request == self.request =>
                {
                    intent
                }
                Ok(_) => {
                    return Err(StoreError::UpgradeBodyRequired {
                        root: self.store.root.clone(),
                    });
                }
                Err(error) => {
                    self.abandon(&mut state);
                    return Err(error);
                }
            };
            if intent.next_unit != intent.units {
                return Err(StoreError::UpgradeBodyRequired {
                    root: self.store.root.clone(),
                });
            }
        }
        match self.store.finish_schema_transition() {
            Ok(true) => {}
            Ok(false) => {
                self.abandon(&mut state);
                return Err(StoreError::UpgradeChanged {
                    root: self.store.root.clone(),
                });
            }
            Err(error) => {
                self.abandon(&mut state);
                return Err(error);
            }
        }
        let writer = self.store.writer.enter();
        let outcome = self.store.stamped((), true);
        let end = self.store.coordination.end_upgrade(true);
        self.store.coordination.release();
        state.active = false;
        state.upgrade = None;
        state.finished = true;
        self.store.coordination.leases().conversion_finished();
        drop(writer);
        end?;
        Ok(outcome)
    }

    /// A best-effort registry error, without changing the conversion outcome.
    pub fn registration_error(&self) -> Option<Arc<StoreError>> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .registration_error
            .clone()
    }

    /// Cancels acquisition; a published announcement is fenced for recovery, never rolled back.
    pub fn cancel(&self) -> Result<(), StoreError> {
        coordinated::leases::Leases::control_context()
            .map_err(|error| drain::refusal(&self.store.root, error))?;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.finished {
            return Ok(());
        }
        let end = if state.active {
            self.store.coordination.end_upgrade(false)
        } else {
            Ok(())
        };
        state.active = false;
        self.abandon(&mut state);
        end?;
        Ok(())
    }

    fn abandon(&self, state: &mut State) {
        if state.active {
            let _ = self.store.coordination.end_upgrade(false);
        }
        self.store.coordination.release();
        state.active = false;
        state.conversion = None;
        state.upgrade = None;
        state.finished = true;
        self.store.coordination.leases().conversion_finished();
    }
}

impl Drop for SchemaUpgrade {
    fn drop(&mut self) {
        if self.cancel().is_err() {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            self.abandon(&mut state);
        }
    }
}

impl UpgradeDrain {
    /// Releases drained shared tiers and starts cancellable, nonblocking conversion.
    pub fn begin_conversion(&self, request: UpgradeRequest) -> Result<SchemaUpgrade, StoreError> {
        self.store.refuse_if_read_only()?;
        UpgradeRequest::new(
            request.layer,
            request.source,
            request.target,
            request.oldest_reader,
        )?;
        self.store
            .coordination
            .leases()
            .detach(&self.token)
            .map_err(|error| drain::refusal(&self.store.root, error))?;
        let conversion = match self.store.coordination.begin_conversion() {
            Ok(conversion) => conversion,
            Err(error) => {
                self.store.coordination.leases().conversion_finished();
                return Err(error);
            }
        };
        Ok(SchemaUpgrade {
            store: self.store.clone(),
            request,
            state: Mutex::new(State {
                conversion: Some(conversion),
                upgrade: None,
                safety: None,
                active: false,
                finished: false,
                registration_error: None,
                body_units: None,
            }),
        })
    }
}

impl Store {
    /// Reattaches a detached handle through ordinary, freshly checked admission.
    /// Agent and nonce options are installed only after admission; the original participant is retained.
    pub fn reattach(&self, options: StoreOptions, access: Access) -> Result<(), StoreError> {
        if options.coordination().implementation() != self.coordination.implementation() {
            return Err(StoreError::Coordination(
                acadia::coord::CoordError::ImplementationMismatch {
                    recorded: self.coordination.implementation().clone(),
                },
            ));
        }
        self.coordination
            .leases()
            .begin_attachment()
            .map_err(|error| drain::refusal(&self.root, error))?;
        if let Err(error) =
            self.coordination
                .attach_again(&self.vfs, &self.root, options.agent(), access)
        {
            self.coordination.leases().conversion_finished();
            return Err(error);
        }
        self.install(options);
        self.coordination.leases().attached();
        Ok(())
    }

    pub(crate) fn finish_schema_transition(&self) -> Result<bool, StoreError> {
        let intent = body::Intent::read(self)?;
        if intent
            .as_ref()
            .is_some_and(|intent| intent.next_unit != intent.units)
        {
            return Err(StoreError::UpgradeBodyRequired {
                root: self.root.clone(),
            });
        }
        let mut genesis = Self::read_admission_genesis(&self.vfs, &self.root)?;
        let Some(transition) = genesis.metadata.transition.clone() else {
            if let Some(intent) = intent {
                if schema(&genesis.metadata, intent.transition.request.layer).current
                    != intent.transition.request.target
                {
                    return Err(StoreError::UpgradeChanged {
                        root: self.root.clone(),
                    });
                }
                body::Intent::clear(self)?;
                return Ok(true);
            }
            return Ok(false);
        };
        UpgradeRequest::new(
            transition.request.layer,
            transition.request.source,
            transition.request.target,
            transition.request.oldest_reader,
        )?;
        if schema(&genesis.metadata, transition.request.layer).current != transition.request.source
        {
            return Err(StoreError::UpgradeChanged {
                root: self.root.clone(),
            });
        }
        advance(&mut genesis, &transition);
        Self::publish_durably(
            &self.vfs,
            &self.root,
            MARKER_NAME,
            &serde_json::to_vec_pretty(&genesis)?,
        )?;
        if intent.is_some() {
            body::Intent::clear(self)?;
        }
        Ok(true)
    }

    pub(super) fn intent_admission(&self) -> Result<Genesis, StoreError> {
        Self::read_admission_genesis(&self.vfs, &self.root)
    }

    pub(super) fn read_admission_genesis(
        vfs: &Arc<dyn Vfs>,
        root: &Path,
    ) -> Result<Genesis, StoreError> {
        let mut genesis = Self::read_genesis(vfs, root)?;
        if let Some(intent) = body::Intent::read_at(vfs, root)? {
            let current = schema(&genesis.metadata, intent.transition.request.layer).current;
            if current == intent.transition.request.source {
                if genesis
                    .metadata
                    .transition
                    .as_ref()
                    .is_some_and(|transition| *transition != intent.transition)
                {
                    return Err(StoreError::UpgradeChanged {
                        root: root.to_path_buf(),
                    });
                }
                if !intent.transition.valid(&genesis.metadata) {
                    return Err(StoreError::UpgradeChanged {
                        root: root.to_path_buf(),
                    });
                }
                genesis.metadata.transition = Some(intent.transition);
            } else if current != intent.transition.request.target
                || intent.next_unit != intent.units
            {
                return Err(StoreError::UpgradeChanged {
                    root: root.to_path_buf(),
                });
            }
        }
        Ok(genesis)
    }
}
