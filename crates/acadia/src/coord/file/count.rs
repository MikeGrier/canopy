// Copyright (c) Michael Grier.

//! [`ModificationCount`] for the file-based implementation: the published count, and how the
//! holder of writer exclusion raises it.
//!
//! The count record holds the value, the boot it was published in, and whether a completed
//! recovery has vouched for the state since. It is replaced whole by renaming a fresh file over
//! it, so a sample is one read of one complete record, and a publisher suspended or killed before
//! its rename leaves the previous record published. Nothing caches a sample, so none is ever stale.
//! A record from another boot, a record no recovery has vouched for, and no record at all all
//! sample odd: state that cannot be vouched for never reads as settled.

use crate::VfsErrorKind;
use crate::coord::{CoordResult, ModCount, ModificationCount};

use super::FileCoordination;
use super::restart::Standing;

pub(crate) mod raising;

/// The count record's encoding. Changing any value is a breaking change to the state's format.
mod format {
    /// The bytes every count record begins with.
    pub const MAGIC: [u8; 4] = *b"ACCT";
    /// The standing of a count no recovery has vouched for.
    pub const UNTRUSTED: u8 = 0;
    /// The standing of a count a completed recovery vouched for.
    pub const VOUCHED: u8 = 1;
    /// Bytes of the value, little-endian.
    pub const VALUE_WIDTH: usize = 8;
}

/// A count record as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Published {
    pub(crate) value: u64,
    /// The standing the record carries into this boot: vouched only if it was published in this
    /// boot and vouched.
    pub(crate) standing: Standing,
}

impl Published {
    const NONE: Published = Published {
        value: 0,
        standing: Standing::Untrusted,
    };

    /// What a sample reports: the value, made odd unless the state is vouched for.
    fn sample(&self) -> ModCount {
        match self.standing {
            Standing::Vouched => ModCount::new(self.value),
            Standing::Untrusted => ModCount::new(self.value | 1),
        }
    }
}

/// The value, standing and boot `bytes` record, or `None` if they are not a count record.
fn decode(bytes: &[u8]) -> Option<(u64, Standing, &[u8])> {
    let rest = bytes.strip_prefix(&format::MAGIC)?;
    let (&standing, rest) = rest.split_first()?;
    let standing = match standing {
        format::UNTRUSTED => Standing::Untrusted,
        format::VOUCHED => Standing::Vouched,
        _ => return None,
    };
    let (value, boot) = rest.split_first_chunk::<{ format::VALUE_WIDTH }>()?;
    Some((u64::from_le_bytes(*value), standing, boot))
}

impl FileCoordination {
    /// The published count record, as it stands in this boot.
    pub(crate) fn read_count(&self) -> CoordResult<Published> {
        let boot = self.current_boot()?;
        let bytes = match self.vfs().read(&self.layout().count()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == VfsErrorKind::NotFound => return Ok(Published::NONE),
            Err(error) => return Err(error.into()),
        };
        Ok(match decode(&bytes) {
            None => Published::NONE,
            Some((value, standing, recorded)) => Published {
                value,
                standing: match (standing, boot) {
                    (Standing::Vouched, Some(boot)) if boot == recorded => Standing::Vouched,
                    _ => Standing::Untrusted,
                },
            },
        })
    }
}

impl ModificationCount for FileCoordination {
    fn sample(&self) -> CoordResult<ModCount> {
        self.alive()?;
        Ok(self.read_count()?.sample())
    }
}
