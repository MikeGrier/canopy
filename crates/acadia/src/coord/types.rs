// Copyright (c) Michael Grier.

//! The values the coordination interface speaks in. None of them is canopy's: what only the caller
//! can interpret crosses the interface as bytes.

use std::borrow::Cow;
use std::fmt;

/// Which implementation coordinates a root.
///
/// Recorded when the root is created and compared on every open, since two conforming
/// implementations need not exclude each other and so must never coordinate one root together.
/// It is short printable ASCII, so it reads as itself wherever it is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImplementationId(Cow<'static, str>);

impl ImplementationId {
    /// The longest identity, in bytes.
    pub const MAX_LEN: usize = 64;

    /// An identity named by a constant. Checked as [`ImplementationId::new`] checks, at compile
    /// time when used in a constant.
    ///
    /// # Panics
    ///
    /// Panics if `id` is not a valid identity.
    #[must_use]
    pub const fn from_static(id: &'static str) -> Self {
        assert!(Self::is_valid(id), "not a valid implementation identity");
        ImplementationId(Cow::Borrowed(id))
    }

    /// An identity read back from where it was recorded, or `None` if `id` is empty, longer than
    /// [`ImplementationId::MAX_LEN`], or holds anything but printable ASCII other than a space.
    #[must_use]
    pub fn new(id: &str) -> Option<Self> {
        Self::is_valid(id).then(|| ImplementationId(Cow::Owned(id.to_owned())))
    }

    /// The identity as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    const fn is_valid(id: &str) -> bool {
        let bytes = id.as_bytes();
        if bytes.is_empty() || bytes.len() > Self::MAX_LEN {
            return false;
        }
        let mut i = 0;
        while i < bytes.len() {
            if !bytes[i].is_ascii_graphic() {
                return false;
            }
            i += 1;
        }
        true
    }
}

impl fmt::Display for ImplementationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The nonce of one creation, minted by its creator and recorded in its reservation.
///
/// It tells two creations of one root apart; a resumer adopts the nonce of the creation it
/// resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CreationNonce([u8; 16]);

impl CreationNonce {
    /// A nonce of the given bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        CreationNonce(bytes)
    }

    /// The nonce's bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }
}

/// The count of acquisitions of writer exclusion, by any participant on the root.
///
/// It advances on every acquisition and changes only under exclusion. A participant that finds,
/// on acquiring, the generation it last advanced to knows no other holder came between, and may
/// keep what it cached about durable state; any other value means it must read that state again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

impl Generation {
    /// A generation of the given value. Only an implementation makes one, from its own state.
    #[must_use]
    pub(crate) const fn new(value: u64) -> Self {
        Generation(value)
    }

    /// The generation's value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A sample of the published modification count.
///
/// The count is odd from before a modification's first effect until after its last, so an even
/// sample is a settled store and an odd one a modification in progress -- or one a dead holder
/// left, or state the implementation cannot vouch for, both of which read as odd until recovery
/// completes. Two equal even samples bracketing a read mean nothing modified the store during it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModCount(u64);

impl ModCount {
    /// A count of the given value. Only an implementation makes one, from its own state.
    #[must_use]
    pub(crate) const fn new(value: u64) -> Self {
        ModCount(value)
    }

    /// The count's value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Whether the count is even: no modification in progress, and the state vouched for.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        self.0.is_multiple_of(2)
    }
}

/// Whether an acquisition may wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Waiting {
    /// Wait until granted. The wait cannot be bounded or cancelled, and no order among waiters
    /// is promised.
    Wait,
    /// Fail at once with [`CoordError::WouldWait`](super::CoordError::WouldWait) instead of
    /// waiting, changing nothing.
    Never,
}

/// What an attached participant may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    /// Read only: the reader tier is held shared.
    Read,
    /// Read and write: the reader and writer tiers are held shared.
    Write,
}

/// Whether an upgrade's result can be read by every admissible reader schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReaderSafety {
    /// Readers stay attached; the upgrade holds the writer tier exclusively.
    Safe,
    /// Every reader is drained; the upgrade holds the reader and writer tiers exclusively.
    Unsafe,
}

/// How a tier is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TierHold {
    /// Alongside other shared holders.
    Shared,
    /// Alone.
    Exclusive,
}

/// The tiers a participant holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HeldTiers {
    /// The reader tier, if held.
    pub reader: Option<TierHold>,
    /// The writer tier, if held.
    pub writer: Option<TierHold>,
}

/// Whether the coordination state can be vouched for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trust {
    /// Maintained continuously since it was last made trusted by a completed recovery.
    Vouched,
    /// Not shown to be continuously maintained, so recovery is required.
    Untrusted(Untrusted),
}

/// Why the coordination state cannot be vouched for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Untrusted {
    /// The host has restarted since the state was made.
    HostRestarted,
    /// The state was made, or made again, and no recovery has completed on it since.
    Created,
    /// The implementation cannot show either way.
    CannotTell,
}

/// The ending of a modification, given to
/// [`ExclusionHold::end_modification`](super::ExclusionHold::end_modification).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModificationEnd {
    /// Every effect is complete or none landed -- a rollback that succeeded included -- so the
    /// count closes even.
    Settled,
    /// An effect landed and could not be resolved: the fence is set and the count stays odd until
    /// recovery completes.
    Unresolved,
}

/// Why a modification is begun.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// An ordinary write, refused while recovery is required.
    Ordinary,
    /// Recovery's own replay, which recovery being required does not refuse.
    Recovery,
}

/// What an acquisition of writer exclusion found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Acquired {
    found: Generation,
    advanced_to: Generation,
    holder_died: bool,
    fenced: bool,
    trust: Trust,
}

impl Acquired {
    #[must_use]
    pub(crate) const fn new(
        found: Generation,
        advanced_to: Generation,
        holder_died: bool,
        fenced: bool,
        trust: Trust,
    ) -> Self {
        Acquired {
            found,
            advanced_to,
            holder_died,
            fenced,
            trust,
        }
    }

    /// The generation found before this acquisition advanced it.
    #[must_use]
    pub const fn found(&self) -> Generation {
        self.found
    }

    /// The generation this acquisition advanced to, which the participant remembers.
    #[must_use]
    pub const fn advanced_to(&self) -> Generation {
        self.advanced_to
    }

    /// Whether an ownership record was left behind: a holder died holding exclusion. The fence
    /// was set before the record was replaced.
    #[must_use]
    pub const fn holder_died(&self) -> bool {
        self.holder_died
    }

    /// Whether the fence was set when exclusion was granted, by this acquisition or before it.
    #[must_use]
    pub const fn fenced(&self) -> bool {
        self.fenced
    }

    /// Whether the coordination state could be vouched for.
    #[must_use]
    pub const fn trust(&self) -> Trust {
        self.trust
    }

    /// Whether recovery is required before any ordinary write: the fence is set or the state
    /// cannot be vouched for.
    #[must_use]
    pub const fn recovery_required(&self) -> bool {
        self.fenced || !matches!(self.trust, Trust::Vouched)
    }
}

/// A token unique to one registration of an attachment in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegistryToken(u128);

impl RegistryToken {
    #[must_use]
    pub(crate) const fn new(value: u128) -> Self {
        RegistryToken(value)
    }

    /// The token's value.
    #[must_use]
    pub const fn get(self) -> u128 {
        self.0
    }
}

/// Whether a registrant got an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Registration {
    /// It holds an entry under this token.
    Registered(RegistryToken),
    /// Every entry was held, so it is attached without one, and every listing says it may be
    /// incomplete.
    Unregistered,
}

/// An entry a listing found live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedEntry {
    /// The token of the registration.
    pub token: RegistryToken,
    /// The tiers its attachment holds.
    pub tiers: HeldTiers,
    /// The payload its registrant supplied, which the interface does not interpret.
    pub payload: Vec<u8>,
}

/// What a listing of the registry found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listing {
    /// The entries whose liveness lock was held when probed, by the registration their token
    /// names. Liveness is as of that probe.
    pub live: Vec<ListedEntry>,
    /// How many entries were being claimed by someone else, or held with a record that could not
    /// be read, and so could not be listed.
    pub unknown: usize,
    /// Whether some attached participant may be missing: an entry was unknown, or a participant
    /// attached unregistered since the host last restarted.
    pub may_be_incomplete: bool,
}

/// Whether [`Creation::create`](super::Creation::create) made the root or finished one begun
/// before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Created {
    /// This call published the reservation and made every artifact.
    Created,
    /// An interrupted creation with identical inputs was completed.
    Resumed,
}
