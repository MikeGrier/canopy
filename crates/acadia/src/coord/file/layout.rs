// Copyright (c) Michael Grier.

//! Where the file-based implementation keeps its state beneath a root.
//!
//! Every name directly beneath the root begins with
//! [`RESERVED_PREFIX`](crate::coord::RESERVED_PREFIX). A lock file holds no data
//! and a data file is never locked, because what a read or write does to bytes inside a span
//! another holder has locked is the platform's ([`Vfs::try_lock`](crate::Vfs::try_lock)).
//!
//! Changing any name or span is a change to the state's format: participants of two builds that
//! disagree on it do not exclude each other.

use std::path::{Path, PathBuf};

use crate::LockSpan;

/// Names directly beneath the root.
mod root {
    /// The creation reservation, published once and kept: it is the record of which
    /// implementation coordinates the root.
    pub const RESERVATION: &str = ".coord-reservation";
    /// The directory holding the rest of the state.
    pub const STATE: &str = ".coord";
    /// The lock file of the file-based implementation's creation exclusion, which exists before
    /// the state directory and so lives beside it.
    pub const CREATION_LOCK: &str = ".coord-creation.lock";
    /// The creation exclusion of the second implementation tests use, which differs from the
    /// file-based one in this alone besides its identity.
    #[cfg(test)]
    pub const ALTERNATE_CREATION_LOCK: &str = ".coord-alternate-creation.lock";
}

/// Names in the state directory.
mod state {
    /// The boot identity the state was made under, and whether a recovery has vouched for it.
    pub const BOOT: &str = "boot";
    /// The prefix of a fresh boot record staged to be renamed over it.
    pub const BOOT_STAGING: &str = "boot.staging.";
    /// The published modification count, replaced by renaming a fresh file over it.
    pub const COUNT: &str = "count";
    /// The prefix of a fresh count file staged for that rename.
    pub const COUNT_STAGING: &str = "count.staging.";
    /// The ownership record of writer exclusion's holder.
    pub const OWNER: &str = "owner";
    /// The prefix of a fresh ownership record staged to be renamed over it.
    pub const OWNER_STAGING: &str = "owner.staging.";
    /// The fence, set while it exists.
    pub const FENCE: &str = "fence";
    /// The generation.
    pub const GENERATION: &str = "generation";
    /// The prefix of a fresh generation staged to be renamed over it.
    pub const GENERATION_STAGING: &str = "generation.staging.";
    /// The directory of registry entries, one file per entry.
    pub const REGISTRY: &str = "registry";
    /// The record that a participant attached unregistered, kept until the host restarts.
    pub const REGISTRY_OVERFLOW: &str = "registry-overflow";
    /// The prefix of a fresh overflow record staged to be renamed over it.
    pub const REGISTRY_OVERFLOW_STAGING: &str = "registry-overflow.staging.";
    /// Writer exclusion, the whole file.
    pub const EXCLUSION_LOCK: &str = "exclusion.lock";
    /// The admission gate, the whole file.
    pub const GATE_LOCK: &str = "gate.lock";
    /// The attachment tiers, one byte each.
    pub const TIERS_LOCK: &str = "tiers.lock";
    /// The registry's claim and liveness locks, a pair of bytes per entry.
    pub const REGISTRY_LOCK: &str = "registry.lock";
}

/// The positions of the locks within a lock file.
mod spans {
    /// The reader tier's byte in the tiers lock file.
    pub const READER_TIER: u64 = 0;
    /// The writer tier's byte in the tiers lock file.
    pub const WRITER_TIER: u64 = 1;
    /// Bytes per registry entry: its claim lock, then its liveness lock.
    pub const REGISTRY_STRIDE: u64 = 2;
    /// The claim lock's offset within an entry's bytes.
    pub const CLAIM: u64 = 0;
    /// The liveness lock's offset within an entry's bytes.
    pub const LIVENESS: u64 = 1;
}

/// How many entries the registry has. A participant that finds all of them held attaches
/// unregistered.
pub(crate) const REGISTRY_ENTRIES: usize = 64;

/// The state of one implementation beneath one root.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    root: PathBuf,
    creation_lock: &'static str,
}

impl Layout {
    /// The file-based implementation's layout beneath `root`.
    pub(crate) fn new(root: &Path) -> Self {
        Layout {
            root: root.to_path_buf(),
            creation_lock: root::CREATION_LOCK,
        }
    }

    /// The second implementation's layout beneath `root`.
    #[cfg(test)]
    pub(crate) fn alternate(root: &Path) -> Self {
        Layout {
            root: root.to_path_buf(),
            creation_lock: root::ALTERNATE_CREATION_LOCK,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn reservation(&self) -> PathBuf {
        self.root.join(root::RESERVATION)
    }

    pub(crate) fn creation_lock(&self) -> PathBuf {
        self.root.join(self.creation_lock)
    }

    pub(crate) fn state_dir(&self) -> PathBuf {
        self.root.join(root::STATE)
    }

    fn state(&self, name: &str) -> PathBuf {
        self.state_dir().join(name)
    }

    pub(crate) fn boot(&self) -> PathBuf {
        self.state(state::BOOT)
    }

    /// A fresh boot record, named by `suffix`, which only its writer uses.
    pub(crate) fn boot_staging(&self, suffix: &str) -> PathBuf {
        self.state(&format!("{}{suffix}", state::BOOT_STAGING))
    }

    pub(crate) fn count(&self) -> PathBuf {
        self.state(state::COUNT)
    }

    /// A fresh count file, named by `suffix`, which only its writer uses.
    pub(crate) fn count_staging(&self, suffix: &str) -> PathBuf {
        self.state(&format!("{}{suffix}", state::COUNT_STAGING))
    }

    pub(crate) fn owner(&self) -> PathBuf {
        self.state(state::OWNER)
    }

    /// A fresh ownership record, named by `suffix`, which only its writer uses.
    pub(crate) fn owner_staging(&self, suffix: &str) -> PathBuf {
        self.state(&format!("{}{suffix}", state::OWNER_STAGING))
    }

    /// A fresh generation, named by `suffix`, which only its writer uses.
    pub(crate) fn generation_staging(&self, suffix: &str) -> PathBuf {
        self.state(&format!("{}{suffix}", state::GENERATION_STAGING))
    }

    pub(crate) fn fence(&self) -> PathBuf {
        self.state(state::FENCE)
    }

    pub(crate) fn generation(&self) -> PathBuf {
        self.state(state::GENERATION)
    }

    pub(crate) fn registry_dir(&self) -> PathBuf {
        self.state(state::REGISTRY)
    }

    /// The data of registry entry `entry`.
    pub(crate) fn registry_entry(&self, entry: usize) -> PathBuf {
        self.registry_dir().join(entry.to_string())
    }

    pub(crate) fn registry_overflow(&self) -> PathBuf {
        self.state(state::REGISTRY_OVERFLOW)
    }

    /// A fresh overflow record, named by `suffix`, which only its writer uses.
    pub(crate) fn registry_overflow_staging(&self, suffix: &str) -> PathBuf {
        self.state(&format!("{}{suffix}", state::REGISTRY_OVERFLOW_STAGING))
    }

    pub(crate) fn exclusion_lock(&self) -> PathBuf {
        self.state(state::EXCLUSION_LOCK)
    }

    pub(crate) fn gate_lock(&self) -> PathBuf {
        self.state(state::GATE_LOCK)
    }

    pub(crate) fn tiers_lock(&self) -> PathBuf {
        self.state(state::TIERS_LOCK)
    }

    pub(crate) fn registry_lock(&self) -> PathBuf {
        self.state(state::REGISTRY_LOCK)
    }

    pub(crate) fn reader_tier() -> LockSpan {
        byte(spans::READER_TIER)
    }

    pub(crate) fn writer_tier() -> LockSpan {
        byte(spans::WRITER_TIER)
    }

    /// Registry entry `entry`'s claim lock, in the registry lock file.
    pub(crate) fn claim(entry: usize) -> LockSpan {
        byte(entry as u64 * spans::REGISTRY_STRIDE + spans::CLAIM)
    }

    /// Registry entry `entry`'s liveness lock, in the registry lock file.
    pub(crate) fn liveness(entry: usize) -> LockSpan {
        byte(entry as u64 * spans::REGISTRY_STRIDE + spans::LIVENESS)
    }

    /// Every lock file, each locked whole or in the spans above.
    #[cfg(test)]
    pub(crate) fn lock_files(&self) -> Vec<PathBuf> {
        vec![
            self.creation_lock(),
            self.exclusion_lock(),
            self.gate_lock(),
            self.tiers_lock(),
            self.registry_lock(),
        ]
    }

    /// Every data file and directory, none ever locked.
    #[cfg(test)]
    pub(crate) fn data_files(&self) -> Vec<PathBuf> {
        let mut files = vec![
            self.reservation(),
            self.state_dir(),
            self.boot(),
            self.count(),
            self.owner(),
            self.fence(),
            self.generation(),
            self.registry_dir(),
            self.registry_overflow(),
        ];
        files.extend((0..REGISTRY_ENTRIES).map(|entry| self.registry_entry(entry)));
        files
    }
}

fn byte(offset: u64) -> LockSpan {
    LockSpan::Range { offset, len: 1 }
}
