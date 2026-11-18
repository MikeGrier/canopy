// Copyright (c) Michael Grier.

use super::*;
use crate::coord::{
    HeldTiers, ListedEntry, Listing, Registration, Registry, RegistryToken, TierHold,
};

#[cfg(test)]
mod tests;

const ENTRIES: usize = 64;
const BASE: usize = 64;
const STRIDE: usize = 4;

/// Changing these values breaks the mapped registry format.
mod format {
    pub const TOKEN: usize = 0;
    pub const READER: usize = 1;
    pub const WRITER: usize = 2;
    pub const BLOB: usize = 3;
    pub const NONE: u64 = 0;
    pub const SHARED: u64 = 1;
    pub const EXCLUSIVE: u64 = 2;
}

fn encoded(tier: Option<TierHold>) -> u64 {
    match tier {
        None => format::NONE,
        Some(TierHold::Shared) => format::SHARED,
        Some(TierHold::Exclusive) => format::EXCLUSIVE,
    }
}

fn decoded(value: u64) -> CoordResult<Option<TierHold>> {
    match value {
        format::NONE => Ok(None),
        format::SHARED => Ok(Some(TierHold::Shared)),
        format::EXCLUSIVE => Ok(Some(TierHold::Exclusive)),
        _ => Err(CoordError::Malformed {
            what: "mapped registry tier",
        }),
    }
}

fn cell(state: &SharedState, index: usize, field: usize) -> &AtomicU64 {
    state.words.get(BASE + index * STRIDE + field)
}

pub(super) struct Entry {
    participant: SharedState,
    index: usize,
    sequence: u64,
    live: Option<Kept>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedEntry")
            .field("index", &self.index)
            .field("sequence", &self.sequence)
            .finish_non_exhaustive()
    }
}

impl SharedState {
    fn registry_lock(&self, file: &str, index: usize) -> CoordResult<Kept> {
        self.alive()?;
        let lock = self
            .vfs
            .try_lock(
                &self.directory.join(file),
                LockMode::Exclusive,
                LockSpan::Range {
                    offset: index as u64,
                    len: 1,
                },
            )
            .map_err(|error| {
                if error.kind() == VfsErrorKind::Locked {
                    CoordError::WouldWait
                } else {
                    error.into()
                }
            })?;
        self.alive()?;
        let mut held = self
            .holds
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.not_abandoned()?;
        let id = NEXT_INSTANCE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| CoordError::Exhausted {
                what: "registry hold identity",
            })?;
        held.insert(id, lock);
        Ok(Kept {
            participant: self.same_participant(),
            id,
        })
    }

    fn next_sequence(&self) -> CoordResult<u64> {
        self.words
            .word(Word::RegistrySequence)
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map(|previous| previous + 1)
            .map_err(|_| CoordError::Exhausted {
                what: "registry sequence",
            })
    }

    fn blob_path(&self, sequence: u64) -> PathBuf {
        self.directory.join(format!(
            "registry-{}-{sequence}.payload",
            self.words.word(Word::Epoch).load(Ordering::Acquire)
        ))
    }

    fn token(&self, sequence: u64) -> RegistryToken {
        RegistryToken::new(
            (u128::from(self.words.word(Word::Epoch).load(Ordering::Acquire)) << u64::BITS)
                | u128::from(sequence),
        )
    }

    /// Records that membership may be incomplete, under the table `abandon` clears, so an
    /// abandoned participant records nothing once `abandon` has returned.
    fn incomplete(&self) -> CoordResult<()> {
        self.alive()?;
        let _held = self
            .holds
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.not_abandoned()?;
        self.words
            .word(Word::RegistryOverflow)
            .store(super::super::format::TRUE, Ordering::Release);
        let mut record = super::super::format::OVERFLOW.to_vec();
        record.extend_from_slice(&self.boot.clone().unwrap_or_default());
        self.vfs
            .write(&self.directory.join("registry-overflow"), &record)?;
        self.vfs.fsync(&self.directory.join("registry-overflow"))?;
        self.vfs.fsync(&self.directory)?;
        Ok(())
    }

    pub(super) fn register_entry(
        &self,
        entry: &mut Option<Entry>,
        tiers: HeldTiers,
        payload: &[u8],
    ) -> CoordResult<Registration> {
        self.alive()?;
        let result = self.register_inner(entry, tiers, payload);
        if result.is_err() && entry.is_none() {
            let _ = self.incomplete();
        }
        result
    }

    fn register_inner(
        &self,
        entry: &mut Option<Entry>,
        tiers: HeldTiers,
        payload: &[u8],
    ) -> CoordResult<Registration> {
        if let Some(held) = entry {
            let claim = self.registry_lock("registry-claims.lock", held.index)?;
            let _guard = self.hold_guard(claim.id)?;
            if !_guard.contains_key(&held.live.as_ref().ok_or(CoordError::Abandoned)?.id) {
                return Err(CoordError::Abandoned);
            }
            if cell(self, held.index, format::TOKEN).load(Ordering::Acquire) != held.sequence {
                return Err(CoordError::Malformed {
                    what: "stale registry token",
                });
            }
            let sequence = self.next_sequence()?;
            self.vfs.publish_new(&self.blob_path(sequence), payload)?;
            let old = cell(self, held.index, format::BLOB).swap(sequence, Ordering::AcqRel);
            cell(self, held.index, format::READER).store(encoded(tiers.reader), Ordering::Release);
            cell(self, held.index, format::WRITER).store(encoded(tiers.writer), Ordering::Release);
            let _ = self.vfs.remove_file(&self.blob_path(old));
            return Ok(Registration::Registered(self.token(held.sequence)));
        }
        for index in 0..ENTRIES {
            let claim = match self.registry_lock("registry-claims.lock", index) {
                Ok(claim) => claim,
                Err(CoordError::WouldWait) => continue,
                Err(error) => return Err(error),
            };
            let live = match self.registry_lock("registry-live.lock", index) {
                Ok(live) => live,
                Err(CoordError::WouldWait) => continue,
                Err(error) => return Err(error),
            };
            let sequence;
            {
                let _guard = self.hold_guard(claim.id)?;
                sequence = self.next_sequence()?;
                self.vfs.publish_new(&self.blob_path(sequence), payload)?;
                let previous_blob = cell(self, index, format::BLOB).load(Ordering::Acquire);
                cell(self, index, format::READER).store(encoded(tiers.reader), Ordering::Release);
                cell(self, index, format::WRITER).store(encoded(tiers.writer), Ordering::Release);
                cell(self, index, format::BLOB).store(sequence, Ordering::Release);
                cell(self, index, format::TOKEN).swap(sequence, Ordering::AcqRel);
                if previous_blob != format::NONE {
                    let _ = self.vfs.remove_file(&self.blob_path(previous_blob));
                }
            }
            *entry = Some(Entry {
                participant: self.same_participant(),
                index,
                sequence,
                live: Some(live),
            });
            return Ok(Registration::Registered(self.token(sequence)));
        }
        self.incomplete()?;
        Ok(Registration::Unregistered)
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Ok(claim) = self
            .participant
            .registry_lock("registry-claims.lock", self.index)
            && let Ok(_guard) = self.participant.hold_guard(claim.id)
            && cell(&self.participant, self.index, format::TOKEN).load(Ordering::Acquire)
                == self.sequence
        {
            let old = cell(&self.participant, self.index, format::BLOB).load(Ordering::Acquire);
            cell(&self.participant, self.index, format::TOKEN)
                .store(format::NONE, Ordering::Release);
            let _ = self
                .participant
                .vfs
                .remove_file(&self.participant.blob_path(old));
        }
        self.live.take();
    }
}

impl Registry for SharedState {
    fn list(&self) -> CoordResult<Listing> {
        self.alive()?;
        let may_be_incomplete = self
            .words
            .word(Word::RegistryOverflow)
            .load(Ordering::Acquire)
            != format::NONE;
        let mut listing = Listing {
            may_be_incomplete,
            ..Listing::default()
        };
        for index in 0..ENTRIES {
            let claim = match self.registry_lock("registry-claims.lock", index) {
                Ok(claim) => claim,
                Err(CoordError::WouldWait) => {
                    listing.unknown += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            match self.registry_lock("registry-live.lock", index) {
                Ok(_free) => continue,
                Err(CoordError::WouldWait) => {}
                Err(error) => return Err(error),
            }
            let _guard = self.hold_guard(claim.id)?;
            let sequence =
                cell(self, index, format::TOKEN).fetch_or(format::NONE, Ordering::AcqRel);
            let inspected = (|| {
                if sequence == format::NONE {
                    return Err(CoordError::Malformed {
                        what: "live registry token",
                    });
                }
                let blob = cell(self, index, format::BLOB).load(Ordering::Acquire);
                Ok(ListedEntry {
                    token: self.token(sequence),
                    tiers: HeldTiers {
                        reader: decoded(cell(self, index, format::READER).load(Ordering::Acquire))?,
                        writer: decoded(cell(self, index, format::WRITER).load(Ordering::Acquire))?,
                    },
                    payload: self.vfs.read(&self.blob_path(blob))?,
                })
            })();
            match inspected {
                Ok(entry) => listing.live.push(entry),
                Err(_) => listing.unknown += 1,
            }
        }
        listing.may_be_incomplete |= listing.unknown > 0;
        Ok(listing)
    }
}
