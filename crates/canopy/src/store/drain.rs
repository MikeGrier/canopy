// Copyright (c) Michael Grier.

use super::coordinated::leases::{DrainToken, LeaseError, OperationLease};
use super::*;

#[cfg(test)]
mod tests;

pub(super) fn refusal(root: &Path, error: LeaseError) -> StoreError {
    match error {
        LeaseError::Draining => StoreError::AttachmentDraining {
            root: root.to_path_buf(),
        },
        LeaseError::InsideOperation => StoreError::UpgradeFromOperation {
            root: root.to_path_buf(),
        },
        LeaseError::Cancelled => StoreError::UpgradeCancelled {
            root: root.to_path_buf(),
        },
        LeaseError::Detached => StoreError::AttachmentDetached {
            root: root.to_path_buf(),
        },
        LeaseError::Busy => StoreError::UpgradeOperationsInFlight {
            root: root.to_path_buf(),
        },
    }
}

/// A cancellable upgrade drain that retains this attachment's shared tiers.
/// Dropping it reopens admission to operations; it performs no tier conversion or schema change.
#[must_use = "dropping the guard cancels the upgrade drain"]
pub struct UpgradeDrain {
    pub(super) store: Store,
    pub(super) token: Arc<DrainToken>,
    registration_error: Option<StoreError>,
}

impl fmt::Debug for UpgradeDrain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UpgradeDrain")
            .field("root", &self.store.root)
            .finish_non_exhaustive()
    }
}

impl UpgradeDrain {
    /// Waits for the outstanding operations to end, without releasing attachment tiers.
    /// Cancellation wakes the wait; waiting from any store operation on the caller's thread is refused.
    pub fn wait(&self) -> Result<(), StoreError> {
        self.store
            .coordination
            .leases()
            .wait(&self.token)
            .map_err(|error| refusal(&self.store.root, error))
    }

    /// Whether every operation has ended, or an error if this drain was cancelled.
    pub fn is_ready(&self) -> Result<bool, StoreError> {
        self.store
            .coordination
            .leases()
            .ready(&self.token)
            .map_err(|error| refusal(&self.store.root, error))
    }

    /// Failure of the initial best-effort registry update, without undoing the drain.
    pub fn registration_error(&self) -> Option<&StoreError> {
        self.registration_error.as_ref()
    }

    /// Cancels an active drain and reopens admission, even if its registry update fails.
    /// A conversion that has taken the drain token is unaffected and requires explicit reattachment.
    /// Call this explicitly to observe a registry failure; dropping the guard cannot report one.
    pub fn cancel(&self) -> Result<(), StoreError> {
        if self.store.coordination.leases().cancel(&self.token) {
            self.store.register_draining()
        } else {
            Ok(())
        }
    }
}

impl Drop for UpgradeDrain {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

impl Store {
    pub(crate) fn operation_lease(&self) -> Result<OperationLease, StoreError> {
        self.coordination
            .leases()
            .enter()
            .map(|lease| lease.keeping(Arc::clone(&self.coordination)))
            .map_err(|error| refusal(&self.root, error))
    }

    /// Refuses new operations on all clones while existing operations drain beneath their tiers.
    /// An attempt from any store operation on the caller's thread is refused before changing its state.
    pub fn drain_upgrade(&self) -> Result<UpgradeDrain, StoreError> {
        let token = self
            .coordination
            .leases()
            .begin()
            .map_err(|error| refusal(&self.root, error))?;
        let mut drain = UpgradeDrain {
            store: self.clone(),
            token,
            registration_error: None,
        };
        drain.registration_error = self.register_draining().err();
        Ok(drain)
    }
}
