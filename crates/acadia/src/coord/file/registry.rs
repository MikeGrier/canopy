// Copyright (c) Michael Grier.

//! The registry of attached participants for the file-based implementation: an entry is a pair of
//! bytes in the registry lock file -- its claim lock and its liveness lock -- and a record file in
//! the registry directory, read and written only under the claim lock.
//!
//! Every registry lock is taken without waiting. A registrant holds an entry's claim lock from
//! before it takes the liveness lock until its record is written, so with the claim lock free an
//! entry's token names the holder of its liveness lock. Releasing the liveness lock is what ends a
//! registration; clearing the record is a courtesy an owner skips when it cannot claim the entry.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::coord::{
    CoordError, CoordResult, HeldTiers, ListedEntry, Listing, Registration, Registry,
    RegistryToken, TierHold, Waiting,
};
use crate::{LockMode, VfsErrorKind};

use super::layout::{Layout, REGISTRY_ENTRIES};
use super::{FileCoordination, HoldId};

/// The encodings of an entry's record and of the overflow record. Changing any value is a
/// breaking change to the state's format.
mod format {
    /// The bytes every entry record begins with.
    pub const ENTRY_MAGIC: [u8; 4] = *b"ACRG";
    /// The bytes every overflow record begins with.
    pub const OVERFLOW_MAGIC: [u8; 4] = *b"ACOV";
    /// A tier not held.
    pub const TIER_NONE: u8 = 0;
    /// A tier held shared.
    pub const TIER_SHARED: u8 = 1;
    /// A tier held exclusively.
    pub const TIER_EXCLUSIVE: u8 = 2;
}

/// The entry a registration holds: its liveness lock, held for as long as the registration lasts.
#[derive(Debug)]
pub(crate) struct Entry {
    index: usize,
    token: RegistryToken,
    liveness: HoldId,
}

/// A token no other registration has: this process's identifier, the time its first token was
/// made, and a count of the tokens it has made.
fn next_token() -> RegistryToken {
    static STARTED: OnceLock<u64> = OnceLock::new();
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let started = *STARTED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64)
    });
    RegistryToken::new(
        (u128::from(std::process::id()) << 96)
            | (u128::from(started) << 32)
            | u128::from(NEXT.fetch_add(1, Ordering::Relaxed)),
    )
}

fn encode_tier(tier: Option<TierHold>) -> u8 {
    match tier {
        None => format::TIER_NONE,
        Some(TierHold::Shared) => format::TIER_SHARED,
        Some(TierHold::Exclusive) => format::TIER_EXCLUSIVE,
    }
}

fn decode_tier(byte: u8) -> Option<Option<TierHold>> {
    match byte {
        format::TIER_NONE => Some(None),
        format::TIER_SHARED => Some(Some(TierHold::Shared)),
        format::TIER_EXCLUSIVE => Some(Some(TierHold::Exclusive)),
        _ => None,
    }
}

/// An entry's record: the magic, the token, the reader and writer tiers, then the payload's length
/// and the payload.
fn encode(token: RegistryToken, tiers: HeldTiers, payload: &[u8]) -> Vec<u8> {
    let mut record = Vec::with_capacity(format::ENTRY_MAGIC.len() + 16 + 2 + 8 + payload.len());
    record.extend_from_slice(&format::ENTRY_MAGIC);
    record.extend_from_slice(&token.get().to_le_bytes());
    record.push(encode_tier(tiers.reader));
    record.push(encode_tier(tiers.writer));
    record.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    record.extend_from_slice(payload);
    record
}

/// The entry `bytes` record, or `None` if they are not an entry record.
pub(crate) fn decode(bytes: &[u8]) -> Option<ListedEntry> {
    let rest = bytes.strip_prefix(&format::ENTRY_MAGIC)?;
    let (token, rest) = rest.split_first_chunk::<16>()?;
    let (&[reader, writer], rest) = rest.split_first_chunk::<2>()?;
    let (length, payload) = rest.split_first_chunk::<8>()?;
    if u64::try_from(payload.len()).ok()? != u64::from_le_bytes(*length) {
        return None;
    }
    Some(ListedEntry {
        token: RegistryToken::new(u128::from_le_bytes(*token)),
        tiers: HeldTiers {
            reader: decode_tier(reader)?,
            writer: decode_tier(writer)?,
        },
        payload: payload.to_vec(),
    })
}

/// What a listing found in one entry it claimed.
enum Inspected {
    /// Its liveness lock was free.
    Free,
    Live(ListedEntry),
    /// Its liveness lock was held and its record could not be read.
    Unreadable,
}

impl FileCoordination {
    /// Tries entry `index`'s claim lock: `None` if someone else holds it.
    fn try_claim(&self, index: usize) -> CoordResult<Option<HoldId>> {
        match self.take_lock(
            &self.layout().registry_lock(),
            LockMode::Exclusive,
            Layout::claim(index),
            Waiting::Never,
        ) {
            Ok(claim) => Ok(Some(claim)),
            Err(CoordError::WouldWait) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Registers with `tiers` and `payload`, rewriting `entry`'s record if it holds one and taking
    /// the first entry it can claim whose liveness lock is free otherwise. Finding none, it records
    /// the overflow and attaches unregistered.
    pub(crate) fn register_entry(
        &self,
        entry: &mut Option<Entry>,
        tiers: HeldTiers,
        payload: &[u8],
    ) -> CoordResult<Registration> {
        self.alive()?;
        let layout = self.layout();
        if let Some(held) = entry {
            let claim = self.try_claim(held.index)?.ok_or(CoordError::WouldWait)?;
            let written = self.vfs().write(
                &layout.registry_entry(held.index),
                &encode(held.token, tiers, payload),
            );
            drop(self.release_lock(claim));
            written?;
            return Ok(Registration::Registered(held.token));
        }
        self.vfs().create_dir_all(&layout.registry_dir())?;
        let lock = layout.registry_lock();
        for index in 0..REGISTRY_ENTRIES {
            let Some(claim) = self.try_claim(index)? else {
                continue;
            };
            let liveness = match self.take_lock(
                &lock,
                LockMode::Exclusive,
                Layout::liveness(index),
                Waiting::Never,
            ) {
                Ok(liveness) => liveness,
                Err(error) => {
                    drop(self.release_lock(claim));
                    match error {
                        CoordError::WouldWait => continue,
                        error => return Err(error),
                    }
                }
            };
            let token = next_token();
            let written = self.vfs().write(
                &layout.registry_entry(index),
                &encode(token, tiers, payload),
            );
            if let Err(error) = written {
                drop(self.release_lock(liveness));
                drop(self.release_lock(claim));
                return Err(error.into());
            }
            drop(self.release_lock(claim));
            *entry = Some(Entry {
                index,
                token,
                liveness,
            });
            return Ok(Registration::Registered(token));
        }
        self.record_overflow()?;
        Ok(Registration::Unregistered)
    }

    /// Ends the registration `entry` holds: clears its record under its claim lock if the claim
    /// lock can be had, then releases its liveness lock. An abandoned participant does neither,
    /// since its locks were released already.
    pub(crate) fn unregister_entry(&self, entry: Entry) {
        let claim = self.try_claim(entry.index).ok().flatten();
        if claim.is_some() {
            let _ = self
                .vfs()
                .remove_file(&self.layout().registry_entry(entry.index));
        }
        drop(self.release_lock(entry.liveness));
        if let Some(claim) = claim {
            drop(self.release_lock(claim));
        }
    }

    /// Records that a participant attached unregistered in this boot. The record names the boot,
    /// so a host restart discards it.
    fn record_overflow(&self) -> CoordResult<()> {
        let mut record = format::OVERFLOW_MAGIC.to_vec();
        record.extend_from_slice(&self.current_boot()?.unwrap_or_default());
        let layout = self.layout();
        self.replace_whole(
            &layout.registry_overflow_staging(&super::staging_suffix()),
            &layout.registry_overflow(),
            &record,
        )
    }

    /// Whether a participant attached unregistered since the host last restarted: there is an
    /// overflow record, and it names this boot, or it cannot be read, or this boot cannot be
    /// identified.
    fn overflowed(&self) -> CoordResult<bool> {
        let bytes = match self.vfs().read(&self.layout().registry_overflow()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == VfsErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let Some(recorded) = bytes.strip_prefix(&format::OVERFLOW_MAGIC) else {
            return Ok(true);
        };
        Ok(self
            .current_boot()?
            .is_none_or(|boot| boot.as_slice() == recorded))
    }

    /// Inspects entry `index`, whose claim lock the caller holds: probes its liveness lock shared,
    /// and reads its record only if the probe is refused.
    fn inspect(&self, index: usize) -> CoordResult<Inspected> {
        match self.take_lock(
            &self.layout().registry_lock(),
            LockMode::Shared,
            Layout::liveness(index),
            Waiting::Never,
        ) {
            Ok(probe) => {
                drop(self.release_lock(probe));
                return Ok(Inspected::Free);
            }
            Err(CoordError::WouldWait) => {}
            Err(error) => return Err(error),
        }
        match self.vfs().read(&self.layout().registry_entry(index)) {
            Ok(bytes) => Ok(decode(&bytes).map_or(Inspected::Unreadable, Inspected::Live)),
            Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(Inspected::Unreadable),
            Err(error) => Err(error.into()),
        }
    }
}

impl Registry for FileCoordination {
    fn list(&self) -> CoordResult<Listing> {
        self.alive()?;
        self.vfs().create_dir_all(&self.layout().state_dir())?;
        let mut listing = Listing::default();
        for index in 0..REGISTRY_ENTRIES {
            let Some(claim) = self.try_claim(index)? else {
                listing.unknown += 1;
                continue;
            };
            let inspected = self.inspect(index);
            drop(self.release_lock(claim));
            match inspected? {
                Inspected::Free => {}
                Inspected::Live(entry) => listing.live.push(entry),
                Inspected::Unreadable => listing.unknown += 1,
            }
        }
        listing.may_be_incomplete = listing.unknown > 0 || self.overflowed()?;
        Ok(listing)
    }
}
