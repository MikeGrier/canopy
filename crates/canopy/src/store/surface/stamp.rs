// Copyright (c) Michael Grier.

use super::*;

#[cfg(test)]
mod tests;

/// A successful operation and its independent agent-version stamp result.
/// A batch retains its first stamp failure even when a later unit successfully retries the stamp.
#[derive(Debug)]
#[must_use = "the operation succeeded; inspect agent_stamp before discarding its metadata outcome"]
pub struct WriteOutcome<T> {
    /// The operation's result, retained even if its metadata stamp failed.
    pub value: T,
    /// Failure here never means the successful operation should be repeated; it does not fence the store.
    pub agent_stamp: Result<(), StoreError>,
}

impl<T> WriteOutcome<T> {
    pub(crate) fn new(value: T, agent_stamp: Result<(), StoreError>) -> Self {
        Self { value, agent_stamp }
    }

    /// Separates the operation's result from the stamp result without losing either.
    pub fn into_parts(self) -> (T, Result<(), StoreError>) {
        (self.value, self.agent_stamp)
    }

    /// Maps the operation's value while preserving its stamp result.
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> WriteOutcome<U> {
        WriteOutcome::new(map(self.value), self.agent_stamp)
    }
}

impl<T> std::ops::Deref for WriteOutcome<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl Store {
    pub(crate) fn refresh_stamp_generation(
        &self,
        acquired: acadia::coord::Acquired,
        changed: bool,
    ) {
        let mut cache = self
            .surface
            .stamp_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if changed {
            *cache = None;
        } else if let Some((generation, _)) = cache.as_mut() {
            *generation = acquired.advanced_to();
        }
    }

    /// Durably changes the agent description; a failed version stamp does not undo that change.
    pub fn set_agent_description(
        &self,
        description: impl Into<String>,
    ) -> Result<WriteOutcome<()>, StoreError> {
        let _lease = self.operation_lease()?;
        let description = description.into();
        let writer = self.writable()?;
        let mut pending = self
            .surface
            .description_pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut genesis = Self::read_genesis(&self.vfs, &self.root)?;
        if genesis.metadata.agent_description == description {
            let completing = pending.as_ref() == Some(&description);
            // Visibility does not prove the marker rename durable.
            self.vfs.fsync(&self.root)?;
            *pending = None;
            return Ok(self.stamped((), completing || writer.publication_settled));
        }
        *pending = Some(description.clone());
        genesis.metadata.agent_description = description;
        let bytes = serde_json::to_vec_pretty(&genesis)?;
        Self::publish_durably(&self.vfs, &self.root, MARKER_NAME, &bytes)?;
        *pending = None;
        Ok(self.stamped((), true))
    }

    pub(crate) fn accumulate_stamp(&self, accumulated: &mut Result<(), StoreError>) {
        let result = self.stamp_agent();
        if accumulated.is_ok() {
            *accumulated = result;
        }
    }

    pub(crate) fn stamped<T>(&self, value: T, changed: bool) -> WriteOutcome<T> {
        WriteOutcome::new(value, if changed { self.stamp_agent() } else { Ok(()) })
    }

    pub(crate) fn stamp_agent(&self) -> Result<(), StoreError> {
        debug_assert!(self.writer.held_depth() > 0);
        let agent = self
            .surface
            .agent
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let agent = agent.as_ref().expect("a store's open installs its agent");
        let generation = self
            .coordination
            .acquired()
            .ok_or_else(|| StoreError::ReadOnly {
                root: self.root.clone(),
            })?
            .advanced_to();
        let mut cache = self
            .surface
            .stamp_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if cache
            .as_ref()
            .is_none_or(|(cached, _)| *cached != generation)
        {
            let genesis = Self::read_genesis(&self.vfs, &self.root)?;
            genesis.metadata.check_identity(agent)?;
            *cache = Some((generation, genesis));
        }
        let (_, recorded) = cache.as_mut().expect("the generation has a cached marker");
        if recorded.metadata.agent_version == agent.version() {
            return Ok(());
        }
        let mut replacement = Self::read_genesis(&self.vfs, &self.root)?;
        replacement.metadata.check_identity(agent)?;
        replacement.metadata.agent_version = agent.version().to_owned();
        let bytes = serde_json::to_vec_pretty(&replacement)?;
        Self::publish_durably(&self.vfs, &self.root, MARKER_NAME, &bytes)?;
        *recorded = replacement;
        Ok(())
    }
}
