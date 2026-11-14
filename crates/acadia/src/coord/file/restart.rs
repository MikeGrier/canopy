// Copyright (c) Michael Grier.

//! [`RestartRecognition`] for the file-based implementation: the boot record.
//!
//! The state directory's boot record names the boot it was made in and whether a completed
//! recovery has vouched for it since. State made in another boot, state never made, and state no
//! recovery has vouched for are all untrusted.
//!
//! The record is replaced whole, by renaming a fresh file over it, so a reader sees one record or
//! the other and never part of one. It is never synced: a restart is the one thing that could
//! lose an unsynced write, and a restart makes the record untrusted whatever it holds.

use crate::VfsErrorKind;
use crate::coord::{CoordResult, RestartRecognition, Trust, Untrusted};

use super::FileCoordination;

/// The boot record's encoding. Changing any value is a breaking change to the state's format.
mod format {
    /// The bytes every boot record begins with.
    pub const MAGIC: [u8; 4] = *b"ACBT";
    /// The standing of state no recovery has vouched for.
    pub const UNTRUSTED: u8 = 0;
    /// The standing of state a completed recovery vouched for.
    pub const VOUCHED: u8 = 1;
}

/// Whether a recovery has vouched for the state since it was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    Untrusted,
    Vouched,
}

/// The standing and boot identity `bytes` record, or `None` if they are not a boot record.
fn decode(bytes: &[u8]) -> Option<(Standing, &[u8])> {
    let rest = bytes.strip_prefix(&format::MAGIC)?;
    let (&standing, boot) = rest.split_first()?;
    let standing = match standing {
        format::UNTRUSTED => Standing::Untrusted,
        format::VOUCHED => Standing::Vouched,
        _ => return None,
    };
    Some((standing, boot))
}

impl FileCoordination {
    /// This boot's identity, or `None` where the filesystem cannot identify a boot.
    pub(super) fn current_boot(&self) -> CoordResult<Option<Vec<u8>>> {
        match self.vfs().boot_identity() {
            Ok(boot) => Ok(Some(boot)),
            Err(error) if error.kind() == VfsErrorKind::Unsupported => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Whether the boot record must be made again: this boot can be identified, and the record is
    /// missing, unreadable, or from another boot.
    pub(super) fn boot_record_needs_remaking(&self) -> CoordResult<bool> {
        let Some(boot) = self.current_boot()? else {
            return Ok(false);
        };
        match self.vfs().read(&self.layout().boot()) {
            Ok(bytes) => Ok(decode(&bytes).is_none_or(|(_, recorded)| recorded != boot.as_slice())),
            Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(true),
            Err(error) => Err(error.into()),
        }
    }

    /// Records the state as made in this boot with `standing`, making the state directory if it
    /// is not there. The record is the magic, the standing, then the boot identity.
    pub(crate) fn record_boot(&self, standing: Standing) -> CoordResult<()> {
        self.alive()?;
        let Some(boot) = self.current_boot()? else {
            return Err(crate::VfsError::new(
                VfsErrorKind::Unsupported,
                "boot_identity",
                "this filesystem cannot identify a boot",
            )
            .into());
        };
        let layout = self.layout();
        self.vfs().create_dir_all(&layout.state_dir())?;
        let mut record = Vec::with_capacity(format::MAGIC.len() + 1 + boot.len());
        record.extend_from_slice(&format::MAGIC);
        record.push(match standing {
            Standing::Untrusted => format::UNTRUSTED,
            Standing::Vouched => format::VOUCHED,
        });
        record.extend_from_slice(&boot);
        self.replace_whole(
            &layout.boot_staging(&super::staging_suffix()),
            &layout.boot(),
            &record,
        )
    }
}

impl RestartRecognition for FileCoordination {
    fn trust(&self) -> CoordResult<Trust> {
        self.alive()?;
        let Some(boot) = self.current_boot()? else {
            return Ok(Trust::Untrusted(Untrusted::CannotTell));
        };
        let bytes = match self.vfs().read(&self.layout().boot()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == VfsErrorKind::NotFound => {
                return Ok(Trust::Untrusted(Untrusted::Created));
            }
            Err(error) => return Err(error.into()),
        };
        Ok(match decode(&bytes) {
            None => Trust::Untrusted(Untrusted::CannotTell),
            Some((_, recorded)) if recorded != boot.as_slice() => {
                Trust::Untrusted(Untrusted::HostRestarted)
            }
            Some((Standing::Untrusted, _)) => Trust::Untrusted(Untrusted::Created),
            Some((Standing::Vouched, _)) => Trust::Vouched,
        })
    }
}
