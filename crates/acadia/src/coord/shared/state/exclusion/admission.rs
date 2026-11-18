// Copyright (c) Michael Grier.

use super::registry::Entry;
use super::*;
use crate::coord::{
    Access, Admission, Attachment, HeldTiers, ReaderSafety, Registrant, Registration, TierHold,
    Upgrade, UpgradeConversion,
};

#[cfg(test)]
mod tests;

struct Held {
    participant: SharedState,
    gate: Option<Kept>,
    reader: Option<Kept>,
    writer: Option<Kept>,
    entry: Option<Entry>,
}

impl std::fmt::Debug for Held {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedTiers")
            .field("gate", &self.gate.is_some())
            .field("reader", &self.reader.is_some())
            .field("writer", &self.writer.is_some())
            .finish_non_exhaustive()
    }
}

impl Held {
    fn new(participant: &SharedState) -> Self {
        Self {
            participant: participant.same_participant(),
            gate: None,
            reader: None,
            writer: None,
            entry: None,
        }
    }
    fn take(&self, file: &str, mode: LockMode, waiting: Waiting) -> CoordResult<Kept> {
        self.participant
            .take_lock(&self.participant.directory.join(file), mode, waiting)
    }
    fn clear(&mut self) {
        self.entry.take();
        self.writer.take();
        self.reader.take();
        self.gate.take();
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.participant.abandon();
            return;
        }
        self.clear();
    }
}

#[derive(Debug)]
struct SharedAttachment {
    access: Access,
    held: Held,
}

#[derive(Debug)]
struct SharedUpgrade {
    safety: ReaderSafety,
    held: Held,
}

#[derive(Debug)]
struct Conversion {
    held: Option<Held>,
    safety: Option<ReaderSafety>,
}

impl Admission for SharedState {
    fn attach(&self, access: Access, waiting: Waiting) -> CoordResult<Box<dyn Attachment>> {
        let mut held = Held::new(self);
        held.gate = Some(held.take("gate.lock", LockMode::Shared, waiting)?);
        held.reader = Some(held.take("reader-tier.lock", LockMode::Shared, waiting)?);
        held.gate.take();
        if access == Access::Write {
            held.writer = Some(held.take("writer-tier.lock", LockMode::Shared, waiting)?);
        }
        Ok(Box::new(SharedAttachment { access, held }))
    }
}

impl Registrant for SharedAttachment {
    fn register(&mut self, payload: &[u8]) -> CoordResult<Registration> {
        let tiers = self.tiers();
        self.held
            .participant
            .register_entry(&mut self.held.entry, tiers, payload)
    }
}

impl Registrant for SharedUpgrade {
    fn register(&mut self, payload: &[u8]) -> CoordResult<Registration> {
        let tiers = self.tiers();
        self.held
            .participant
            .register_entry(&mut self.held.entry, tiers, payload)
    }
}

impl Attachment for SharedAttachment {
    fn access(&self) -> Access {
        self.access
    }
    fn tiers(&self) -> HeldTiers {
        HeldTiers {
            reader: Some(TierHold::Shared),
            writer: (self.access == Access::Write).then_some(TierHold::Shared),
        }
    }
    fn begin_conversion(mut self: Box<Self>) -> CoordResult<Box<dyn UpgradeConversion>> {
        self.held.clear();
        self.held.participant.alive()?;
        Ok(Box::new(Conversion {
            held: Some(Held::new(&self.held.participant)),
            safety: None,
        }))
    }
    fn upgrade(
        mut self: Box<Self>,
        safety: ReaderSafety,
        waiting: Waiting,
    ) -> CoordResult<Box<dyn Upgrade>> {
        self.held.clear();
        let mut held = Held::new(&self.held.participant);
        held.gate = Some(held.take("gate.lock", LockMode::Exclusive, waiting)?);
        if safety == ReaderSafety::Unsafe {
            held.reader = Some(held.take("reader-tier.lock", LockMode::Exclusive, waiting)?);
        }
        held.writer = Some(held.take("writer-tier.lock", LockMode::Exclusive, waiting)?);
        if safety == ReaderSafety::Safe {
            held.gate.take();
        }
        Ok(Box::new(SharedUpgrade { safety, held }))
    }
}

impl Upgrade for SharedUpgrade {
    fn safety(&self) -> ReaderSafety {
        self.safety
    }
    fn tiers(&self) -> HeldTiers {
        HeldTiers {
            reader: (self.safety == ReaderSafety::Unsafe).then_some(TierHold::Exclusive),
            writer: Some(TierHold::Exclusive),
        }
    }
    fn admit_readers(&mut self) -> CoordResult<()> {
        self.held.participant.alive()?;
        if self.safety == ReaderSafety::Safe {
            self.held.gate.take();
        }
        Ok(())
    }
}

impl Conversion {
    fn gate(&mut self) -> CoordResult<bool> {
        let held = self.held.as_mut().ok_or(CoordError::Malformed {
            what: "finished conversion",
        })?;
        held.participant.alive()?;
        if held.gate.is_some() {
            return Ok(true);
        }
        match held.take("gate.lock", LockMode::Exclusive, Waiting::Never) {
            Ok(gate) => {
                held.gate = Some(gate);
                Ok(true)
            }
            Err(CoordError::WouldWait) => Ok(false),
            Err(error) => Err(error),
        }
    }
    fn tiers(&mut self, safety: ReaderSafety) -> CoordResult<Option<Box<dyn Upgrade>>> {
        if self.safety.is_some_and(|previous| previous != safety) {
            return Err(CoordError::Malformed {
                what: "changed conversion safety",
            });
        }
        self.safety = Some(safety);
        let held = self.held.as_mut().ok_or(CoordError::Malformed {
            what: "finished conversion",
        })?;
        held.participant.alive()?;
        if held.gate.is_none() {
            return Err(CoordError::Malformed {
                what: "conversion without gate",
            });
        }
        if safety == ReaderSafety::Unsafe && held.reader.is_none() {
            match held.take("reader-tier.lock", LockMode::Exclusive, Waiting::Never) {
                Ok(reader) => held.reader = Some(reader),
                Err(CoordError::WouldWait) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        match held.take("writer-tier.lock", LockMode::Exclusive, Waiting::Never) {
            Ok(writer) => {
                held.writer = Some(writer);
                Ok(Some(Box::new(SharedUpgrade {
                    safety,
                    held: self.held.take().expect("conversion holds its tiers"),
                })))
            }
            Err(CoordError::WouldWait) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl UpgradeConversion for Conversion {
    fn poll_gate(&mut self) -> CoordResult<bool> {
        let result = self.gate();
        if result.is_err() {
            self.held.take();
        }
        result
    }
    fn poll_tiers(&mut self, safety: ReaderSafety) -> CoordResult<Option<Box<dyn Upgrade>>> {
        let result = self.tiers(safety);
        if result.is_err() {
            self.held.take();
        }
        result
    }
}
