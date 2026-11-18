// Copyright (c) Michael Grier.

use super::*;
use crate::coord::{CoordError, UpgradeConversion};

#[derive(Debug)]
pub(super) struct FileConversion {
    held: Option<Held>,
    gate: Option<HoldId>,
    reader: bool,
    safety: Option<ReaderSafety>,
}

impl FileConversion {
    pub(super) fn new(participant: &FileCoordination) -> Self {
        Self {
            held: Some(Held::new(participant)),
            gate: None,
            reader: false,
            safety: None,
        }
    }

    fn gate(&mut self) -> CoordResult<bool> {
        let held = self.held.as_mut().ok_or(CoordError::Malformed {
            what: "finished conversion",
        })?;
        held.participant.alive()?;
        if self.gate.is_some() {
            return Ok(true);
        }
        let path = held.participant.layout().gate_lock();
        match held.take(&path, LockMode::Exclusive, LockSpan::File, Waiting::Never) {
            Ok(gate) => {
                self.gate = Some(gate);
                Ok(true)
            }
            Err(CoordError::WouldWait) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn tiers(&mut self, safety: ReaderSafety) -> CoordResult<Option<Box<dyn Upgrade>>> {
        if self.gate.is_none() {
            return Err(CoordError::Malformed {
                what: "conversion without its gate",
            });
        }
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
        let path = held.participant.layout().tiers_lock();
        if safety == ReaderSafety::Unsafe && !self.reader {
            match held.take(
                &path,
                LockMode::Exclusive,
                Layout::reader_tier(),
                Waiting::Never,
            ) {
                Ok(_) => self.reader = true,
                Err(CoordError::WouldWait) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        match held.take(
            &path,
            LockMode::Exclusive,
            Layout::writer_tier(),
            Waiting::Never,
        ) {
            Ok(_) => Ok(Some(Box::new(FileUpgrade {
                safety,
                held: self.held.take().expect("the conversion holds its tiers"),
                gate: self.gate.take(),
            }))),
            Err(CoordError::WouldWait) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl UpgradeConversion for FileConversion {
    fn poll_gate(&mut self) -> CoordResult<bool> {
        let result = self.gate();
        if result.is_err() {
            self.held.take();
            self.gate = None;
        }
        result
    }

    fn poll_tiers(&mut self, safety: ReaderSafety) -> CoordResult<Option<Box<dyn Upgrade>>> {
        let result = self.tiers(safety);
        if result.is_err() {
            self.held.take();
            self.gate = None;
        }
        result
    }
}
