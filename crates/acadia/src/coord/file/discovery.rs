// Copyright (c) Michael Grier.

//! [`RecoveryDiscovery`] for the file-based implementation.
//!
//! One observation is made consistent by trying the writer lock without waiting. A lock that is
//! free means no holder is alive, and the fence, the ownership record and the boot record are read
//! while this participant holds it, so no acquirer comes between the readings and an ownership
//! record found is a dead holder's. A lock that is held means a holder is alive, whose record is
//! its transaction, and only the fence and the boot record -- which a live holder's own
//! recovery also reads -- are reported. The probe takes the lock and drops it without the
//! bookkeeping of a hold: it sets no fence, replaces no record and advances no generation.

use crate::coord::{
    CoordError, CoordResult, RecoveryDiscovery, RecoveryNeed, RestartRecognition, Trust, Waiting,
};

use super::FileCoordination;

impl RecoveryDiscovery for FileCoordination {
    fn discover_recovery(&self) -> CoordResult<RecoveryNeed> {
        self.alive()?;
        let layout = self.layout();
        // A root whose state directory is not there has had no holder, so it has no lock to probe.
        let (probe, holder_alive) = if self.vfs().exists(&layout.state_dir())? {
            match self.acquire_exclusion(Waiting::Never) {
                Ok(probe) => (Some(probe), false),
                Err(CoordError::WouldWait) => (None, true),
                Err(error) => return Err(error),
            }
        } else {
            (None, false)
        };
        let fenced = self.fence_is_set()?;
        let holder_died = !holder_alive && self.vfs().exists(&layout.owner())?;
        let trust = self.trust()?;
        drop(probe);
        Ok(RecoveryNeed::new(
            fenced,
            match trust {
                Trust::Vouched => None,
                Trust::Untrusted(why) => Some(why),
            },
            holder_died,
        ))
    }
}
