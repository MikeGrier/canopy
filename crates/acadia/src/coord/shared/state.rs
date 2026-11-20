// Copyright (c) Michael Grier.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::coord::{CoordError, CoordResult, Trust, Untrusted};
use crate::{LockMode, LockSpan, Vfs, VfsError, VfsErrorKind, VfsLock};

mod exclusion;
#[cfg(any(test, feature = "test-support"))]
mod hook;
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub use hook::{SharedStep, SharedWords, StepHook, inspect};

const DIRECTORY: &str = ".coord-shared";
const WORD_COUNT: usize = 16 * 1024;
const BYTE_COUNT: usize = WORD_COUNT * size_of::<AtomicU64>();
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// Changing these indices or values breaks the shared state's format.
#[repr(usize)]
#[derive(Clone, Copy)]
enum Word {
    Ready,
    Vouched,
    Reason,
    Fence,
    Generation,
    Owner,
    Count,
    RegistrySequence,
    RegistryOverflow,
    Epoch,
    GenerationLimit,
}

/// Changing these values breaks the shared-state format.
mod format {
    pub const INSTANCE: [u8; 4] = *b"ACSI";
    pub const BOOT: [u8; 4] = *b"ACSB";
    pub const EPOCH: [u8; 4] = *b"ACSE";
    pub const OVERFLOW: [u8; 4] = *b"ACSO";
    pub const GENERATIONS: [u8; 4] = *b"ACSG";
    pub const GENERATION_WIDTH: usize = size_of::<u64>();
    pub const GENERATION_BLOCK: u64 = 1 << u32::BITS;
    pub const READY: u64 = 0x4143_5348_0000_0001;
    pub const FALSE: u64 = 0;
    pub const TRUE: u64 = 1;
    pub const CREATED: u64 = 0;
    pub const RESTARTED: u64 = 1;
    pub const CANNOT_TELL: u64 = 2;
    pub const UNSET_COUNT: u64 = 1;
}

type ModelKey = (u64, Vec<u8>, PathBuf);
type Models = Mutex<HashMap<ModelKey, Weak<Words>>>;
static MODELS: OnceLock<Models> = OnceLock::new();

enum Words {
    Native(NativeWords),
    Model(Box<[AtomicU64]>),
}

/// The owned mapping keeps the aligned pointer live; every access through it is atomic.
struct NativeWords {
    _mapping: memmap2::MmapMut,
    base: NonNull<u64>,
}

unsafe impl Send for NativeWords {}
unsafe impl Sync for NativeWords {}

impl Words {
    fn get(&self, index: usize) -> &AtomicU64 {
        assert!(index < WORD_COUNT);
        match self {
            Self::Model(words) => &words[index],
            Self::Native(words) => unsafe { AtomicU64::from_ptr(words.base.as_ptr().add(index)) },
        }
    }

    fn word(&self, word: Word) -> &AtomicU64 {
        self.get(word as usize)
    }

    fn reset(&self, reason: u64) {
        self.word(Word::Ready)
            .store(format::FALSE, Ordering::Release);
        for index in 1..WORD_COUNT {
            self.get(index).store(format::FALSE, Ordering::Release);
        }
        self.word(Word::Reason).store(reason, Ordering::Release);
        self.word(Word::Fence)
            .store(format::TRUE, Ordering::Release);
        self.word(Word::Count)
            .store(format::UNSET_COUNT, Ordering::Release);
        self.word(Word::Ready)
            .store(format::READY, Ordering::Release);
    }
}

/// What lets a participant be abandoned while it opens its state: `abandon` sets `abandoned` and
/// then takes `lock`, which an opening holds while it makes the state directory and the setup lock,
/// and again from the end of its wait for the setup lock until it returns. An abandonment therefore
/// never waits for an opening blocked on another participant, and once it returns the opening
/// changes nothing more.
pub(crate) struct OpenGate<'a> {
    pub(crate) lock: &'a Mutex<()>,
    pub(crate) abandoned: Arc<AtomicBool>,
}

impl OpenGate<'_> {
    fn enter(&self) -> CoordResult<std::sync::MutexGuard<'_, ()>> {
        let guard = match self.lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                self.abandoned.store(true, Ordering::Release);
                return Err(CoordError::Abandoned);
            }
        };
        if self.abandoned.load(Ordering::Acquire) {
            return Err(CoordError::Abandoned);
        }
        Ok(guard)
    }
}

/// A diagnostic view of a shared-state epoch. It grants no document access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateSnapshot {
    pub trust: Trust,
    pub fenced: bool,
    pub generation: u64,
    pub owner: u64,
    pub modification_count: u64,
}

/// One live participant's shared-state storage.
///
/// Opening storage is an explicit initialization operation, not
/// [`CoordinationFactory::open`](crate::coord::CoordinationFactory::open). Coordination artifacts
/// belong exclusively to this implementation and must not be truncated or removed while live.
/// A file-backed mapping avoids Windows session-local names and global mapping privileges.
pub struct SharedState {
    release: Arc<crate::coord::ReleaseGate>,
    vfs: Arc<dyn Vfs>,
    directory: PathBuf,
    live: Arc<Mutex<Option<Box<dyn VfsLock>>>>,
    words: Arc<Words>,
    boot: Option<Vec<u8>>,
    abandoned: Arc<AtomicBool>,
    holds: Arc<Mutex<HashMap<u64, Box<dyn VfsLock>>>>,
    creation: Weak<Mutex<HashMap<u64, Box<dyn VfsLock>>>>,
    #[cfg(any(test, feature = "test-support"))]
    hook: Option<Arc<dyn StepHook>>,
}

impl std::fmt::Debug for SharedState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedState")
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

fn failure(message: &str) -> CoordError {
    VfsError::new(VfsErrorKind::InvalidInput, "shared_state", message).into()
}

fn boot(vfs: &dyn Vfs) -> CoordResult<Option<Vec<u8>>> {
    match vfs.boot_identity() {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == VfsErrorKind::Unsupported => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The in-process model's name for the state directory `directory`, the same however it is
/// spelled from the filesystem's root: normalized as `MemFs` resolves a path, so every spelling
/// that reaches one directory there names one model.
fn model_root(directory: &Path) -> PathBuf {
    use std::path::Component;
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for part in directory.components() {
        match part {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(name) => parts.push(name),
        }
    }
    parts.into_iter().collect()
}

fn read_optional(vfs: &dyn Vfs, path: &Path) -> CoordResult<Option<Vec<u8>>> {
    match vfs.read(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == VfsErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl SharedState {
    fn reserve_generations(
        vfs: &dyn Vfs,
        directory: &Path,
        renewing: Option<u64>,
    ) -> CoordResult<(u64, u64)> {
        let path = directory.join("generation-range");
        let previous = match read_optional(vfs, &path)? {
            None if renewing.is_some() => {
                return Err(CoordError::Malformed {
                    what: "generation reservation",
                });
            }
            None => 0,
            Some(record) => {
                let bytes =
                    record
                        .strip_prefix(&format::GENERATIONS)
                        .ok_or(CoordError::Malformed {
                            what: "generation reservation",
                        })?;
                if bytes.len() != format::GENERATION_WIDTH * 2 {
                    return Err(CoordError::Malformed {
                        what: "generation reservation",
                    });
                }
                let (value, inverse) = bytes.split_at(format::GENERATION_WIDTH);
                let value = u64::from_le_bytes(value.try_into().expect("checked width"));
                if u64::from_le_bytes(inverse.try_into().expect("checked width")) != !value {
                    return Err(CoordError::Malformed {
                        what: "generation reservation",
                    });
                }
                value
            }
        };
        if let Some(expected) = renewing
            && previous != expected
        {
            if expected.saturating_add(format::GENERATION_BLOCK) != previous {
                return Err(CoordError::Malformed {
                    what: "generation reservation",
                });
            }
            vfs.fsync(directory)?;
            return Ok((expected, previous));
        }
        if previous == u64::MAX {
            return Err(CoordError::Exhausted { what: "generation" });
        }
        let limit = previous.saturating_add(format::GENERATION_BLOCK);
        let mut record = format::GENERATIONS.to_vec();
        record.extend_from_slice(&limit.to_le_bytes());
        record.extend_from_slice(&(!limit).to_le_bytes());
        let staging = directory.join("generation-range.staging");
        vfs.write(&staging, &record)?;
        vfs.fsync(&staging)?;
        vfs.rename(&staging, &path)?;
        vfs.fsync(directory)?;
        Ok((previous, limit))
    }

    /// Opens an epoch, serializing initialization before taking its shared liveness hold.
    /// A last-participant exit, host restart or interrupted initialization requires recovery.
    /// Reached only through a participant, which first refuses a root another implementation
    /// reserved.
    #[cfg(test)]
    pub(crate) fn open(vfs: Arc<dyn Vfs>, root: &Path) -> CoordResult<Self> {
        Self::open_gated(vfs, root, None, |state| state)
    }

    /// As [`open`](Self::open), for a participant that `gate` can abandon while it waits, handing
    /// the state to `handoff` before the final opening guard is released.
    pub(crate) fn open_gated<T>(
        vfs: Arc<dyn Vfs>,
        root: &Path,
        gate: Option<&OpenGate<'_>>,
        handoff: impl FnOnce(Self) -> T,
    ) -> CoordResult<T> {
        let mut handoff = Some(handoff);
        loop {
            if let Some(handoffed) = Self::open_once(Arc::clone(&vfs), root, gate, &mut handoff)? {
                return Ok(handoffed);
            }
        }
    }

    /// One attempt at [`open`](Self::open), or `None` when the epoch it would join ended between
    /// finding a live participant and joining it, so the attempt must decide again.
    fn open_once<T>(
        vfs: Arc<dyn Vfs>,
        root: &Path,
        gate: Option<&OpenGate<'_>>,
        handoff: &mut Option<impl FnOnce(Self) -> T>,
    ) -> CoordResult<Option<T>> {
        let directory = root.join(DIRECTORY);
        let setup = directory.join("setup.lock");
        {
            let _bootstrap = gate.map(OpenGate::enter).transpose()?;
            vfs.read_dir(root)?;
            match vfs.metadata(&directory) {
                Ok(_) => {}
                Err(error) if error.kind() == VfsErrorKind::NotFound => {
                    vfs.create_dir_all(&directory)?;
                }
                Err(error) => return Err(error.into()),
            }
            // Whoever made the directory may have died before its name was durable, and identities
            // are issued from what lies in it.
            vfs.fsync(root)?;
            // Made here, so the wait below opens the lock and changes nothing.
            if !vfs.exists(&setup)? {
                match vfs.try_lock(&setup, LockMode::Exclusive, LockSpan::File) {
                    Ok(_) => {}
                    Err(error) if error.kind() == VfsErrorKind::Locked => {}
                    Err(error) => return Err(error.into()),
                }
                vfs.fsync(&directory)?;
            }
        }
        let _setup = vfs.lock(&setup, LockMode::Exclusive, LockSpan::File)?;
        let _opening = gate.map(OpenGate::enter).transpose()?;
        let last = match vfs.try_lock(
            &directory.join("live.lock"),
            LockMode::Exclusive,
            LockSpan::File,
        ) {
            Ok(held) => Some(held),
            Err(error) if error.kind() == VfsErrorKind::Locked => None,
            Err(error) => return Err(error.into()),
        };
        let first = last.is_some();
        let current_boot = boot(vfs.as_ref())?;
        let old_boot = read_optional(vfs.as_ref(), &directory.join("boot"))?;
        let mut reason = match (&current_boot, &old_boot) {
            (None, _) => format::CANNOT_TELL,
            (Some(_), None) => format::CREATED,
            (Some(current), Some(previous)) => match previous.strip_prefix(&format::BOOT) {
                Some(recorded) if recorded == current.as_slice() => format::CREATED,
                Some(_) => format::RESTARTED,
                None => format::CANNOT_TELL,
            },
        };
        if !first && reason == format::RESTARTED {
            return Err(failure("live participants belong to another host boot"));
        }
        let instance_path = directory.join("instance");
        let encoded_instance = read_optional(vfs.as_ref(), &instance_path)?;
        let decoded_instance = encoded_instance
            .as_ref()
            .and_then(|bytes| bytes.strip_prefix(&format::INSTANCE))
            .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
            .map(u64::from_le_bytes);
        let instance = match decoded_instance {
            Some(instance) if !first => instance,
            _ if first => {
                if encoded_instance.is_some() && decoded_instance.is_none() {
                    reason = format::CANNOT_TELL;
                }
                let value = NEXT_INSTANCE
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                        value.checked_add(1)
                    })
                    .map_err(|_| failure("shared-state instance identity exhausted"))?;
                let mut bytes = format::INSTANCE.to_vec();
                bytes.extend_from_slice(&value.to_le_bytes());
                if encoded_instance.is_some() {
                    vfs.write(&instance_path, &bytes)?;
                    vfs.fsync(&instance_path)?;
                } else {
                    vfs.publish_new(&instance_path, &bytes)?;
                }
                vfs.fsync(&directory)?;
                value
            }
            _ => return Err(failure("invalid live shared-state instance")),
        };
        let cells = directory.join("cells");
        let epoch = if first {
            for entry in vfs.read_dir(&directory)? {
                if entry.file_name.starts_with("registry-") && entry.file_name.ends_with(".payload")
                {
                    vfs.remove_file(&directory.join(entry.file_name))?;
                }
            }
            let epoch_path = directory.join("epoch");
            let previous = read_optional(vfs.as_ref(), &epoch_path)?;
            let decoded = previous
                .as_ref()
                .and_then(|bytes| bytes.strip_prefix(&format::EPOCH))
                .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
                .map(u64::from_le_bytes);
            let next = match decoded {
                Some(value) => value.checked_add(1).ok_or(CoordError::Exhausted {
                    what: "shared-state epoch",
                })?,
                None if previous.is_some() => {
                    return Err(CoordError::Malformed {
                        what: "shared-state epoch",
                    });
                }
                None => 1,
            };
            let mut bytes = format::EPOCH.to_vec();
            bytes.extend_from_slice(&next.to_le_bytes());
            // Replaced whole: the record has no predecessor to fall back on if torn.
            let staging = directory.join("epoch.staging");
            vfs.write(&staging, &bytes)?;
            vfs.fsync(&staging)?;
            vfs.rename(&staging, &epoch_path)?;
            vfs.fsync(&directory)?;
            Some(next)
        } else {
            None
        };
        match vfs.metadata(&cells) {
            Ok(metadata) if metadata.len == BYTE_COUNT as u64 => {}
            Ok(_) if first => {
                reason = format::CANNOT_TELL;
                vfs.write(&cells, &vec![0; BYTE_COUNT])?;
                vfs.fsync(&cells)?;
            }
            Ok(_) => return Err(failure("invalid live shared-state mapping length")),
            Err(error) if error.kind() == VfsErrorKind::NotFound && first => {
                vfs.publish_new(&cells, &vec![0; BYTE_COUNT])?;
                vfs.fsync(&directory)?;
            }
            Err(error) => return Err(error.into()),
        }
        let generations = if first {
            Some(Self::reserve_generations(vfs.as_ref(), &directory, None)?)
        } else {
            None
        };
        let words = if vfs.is_os_filesystem() {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&cells)
                .map_err(|error| {
                    VfsError::new(VfsErrorKind::Other, "shared_state_map", error.to_string())
                        .with_path(&cells)
                })?;
            let mut mapping = unsafe { memmap2::MmapOptions::new().len(BYTE_COUNT).map_mut(&file) }
                .map_err(|error| {
                    VfsError::new(VfsErrorKind::Other, "shared_state_map", error.to_string())
                        .with_path(&cells)
                })?;
            if !(mapping.as_ptr() as usize).is_multiple_of(align_of::<AtomicU64>()) {
                return Err(failure("unaligned shared-state mapping"));
            }
            let base = NonNull::new(mapping.as_mut_ptr().cast::<u64>())
                .ok_or_else(|| failure("null shared-state mapping"))?;
            Arc::new(Words::Native(NativeWords {
                _mapping: mapping,
                base,
            }))
        } else {
            let normalized_root = model_root(&directory);
            let key = (
                instance,
                current_boot.clone().unwrap_or_default(),
                normalized_root,
            );
            // This cache is process-wide, and retaining a surviving weak map is safe after a panic.
            let mut models = MODELS
                .get_or_init(Default::default)
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            models.retain(|_, words| words.strong_count() > 0);
            if let Some(words) = models.get(&key).and_then(Weak::upgrade) {
                words
            } else if !first {
                // The model's words live only as long as a participant does: decide again only if
                // the liveness lock, probed under the setup lock still held, shows none remains.
                drop(models);
                return match vfs.try_lock(
                    &directory.join("live.lock"),
                    LockMode::Exclusive,
                    LockSpan::File,
                ) {
                    Ok(_) => Ok(None),
                    Err(error) if error.kind() == VfsErrorKind::Locked => {
                        Err(failure("the live epoch's state cannot be found"))
                    }
                    Err(error) => Err(error.into()),
                };
            } else {
                let words = Arc::new(Words::Model(
                    (0..WORD_COUNT).map(|_| AtomicU64::new(0)).collect(),
                ));
                models.insert(key, Arc::downgrade(&words));
                words
            }
        };
        if first {
            words.reset(reason);
            words.word(Word::Epoch).store(
                epoch.expect("an initializer reserved its epoch"),
                Ordering::Release,
            );
            let (generation, limit) = generations.expect("an initializer reserved generations");
            words
                .word(Word::Generation)
                .store(generation, Ordering::Release);
            words
                .word(Word::GenerationLimit)
                .store(limit, Ordering::Release);
            let overflow = read_optional(vfs.as_ref(), &directory.join("registry-overflow"))?;
            let incomplete = current_boot.is_none()
                || overflow.as_ref().is_some_and(|record| {
                    record
                        .strip_prefix(&format::OVERFLOW)
                        .is_none_or(|recorded| {
                            current_boot
                                .as_ref()
                                .is_none_or(|current| current.as_slice() == recorded)
                        })
                });
            words
                .word(Word::RegistryOverflow)
                .store(u64::from(incomplete), Ordering::Release);
            if let Some(current) = &current_boot {
                let mut recorded = format::BOOT.to_vec();
                recorded.extend_from_slice(current);
                vfs.write(&directory.join("boot"), &recorded)?;
                vfs.fsync(&directory.join("boot"))?;
                vfs.fsync(&directory)?;
            }
        } else if words.word(Word::Ready).load(Ordering::Acquire) != format::READY {
            return Err(failure("shared-state initialization is incomplete"));
        }
        drop(last);
        let live = vfs.lock(
            &directory.join("live.lock"),
            LockMode::Shared,
            LockSpan::File,
        )?;
        let state = Self {
            release: Arc::default(),
            vfs,
            directory,
            words,
            boot: current_boot,
            live: Arc::new(Mutex::new(Some(live))),
            abandoned: gate.map_or_else(
                || Arc::new(AtomicBool::new(false)),
                |gate| Arc::clone(&gate.abandoned),
            ),
            holds: Arc::new(Mutex::new(HashMap::new())),
            creation: Weak::new(),
            #[cfg(any(test, feature = "test-support"))]
            hook: None,
        };
        let handoff = handoff.take().expect("the opening handoff runs once");
        Ok(Some(handoff(state)))
    }

    /// This participant, announcing each later step that changes the shared words to `hook`.
    pub(super) fn with_release(mut self, release: Arc<crate::coord::ReleaseGate>) -> Self {
        self.release = release;
        self
    }

    pub(super) fn with_creation(
        mut self,
        creation: Weak<Mutex<HashMap<u64, Box<dyn VfsLock>>>>,
    ) -> Self {
        self.creation = creation;
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn with_hook(mut self, hook: Option<Arc<dyn StepHook>>) -> Self {
        self.hook = hook;
        self
    }

    fn live_guard(&self) -> CoordResult<std::sync::MutexGuard<'_, Option<Box<dyn VfsLock>>>> {
        match self.live.lock() {
            Ok(guard) => Ok(guard),
            Err(poisoned) => {
                drop(poisoned);
                self.abandon();
                Err(CoordError::Abandoned)
            }
        }
    }

    fn holds_guard(
        &self,
    ) -> CoordResult<std::sync::MutexGuard<'_, HashMap<u64, Box<dyn VfsLock>>>> {
        match self.holds.lock() {
            Ok(guard) => Ok(guard),
            Err(poisoned) => {
                drop(poisoned);
                self.abandon();
                Err(CoordError::Abandoned)
            }
        }
    }

    /// Whether the fence word is set, which is what `RecoveryNeed::fenced` and
    /// `Acquired::fenced` report; [`snapshot`](Self::snapshot) also counts untrusted state.
    pub(crate) fn fence_set(&self) -> bool {
        self.words.word(Word::Fence).load(Ordering::Acquire) != format::FALSE
    }

    /// Returns the epoch's current diagnostic state. Untrusted epochs are always fenced.
    pub fn snapshot(&self) -> CoordResult<StateSnapshot> {
        if self.abandoned.load(Ordering::Acquire) {
            return Err(CoordError::Abandoned);
        }
        self.snapshot_at(&boot(self.vfs.as_ref())?)
    }

    /// As [`snapshot`](Self::snapshot), with the boot already read, so a caller holding one of the
    /// participant's mutexes makes no filesystem call under it.
    fn snapshot_at(&self, current_boot: &Option<Vec<u8>>) -> CoordResult<StateSnapshot> {
        let live = self.live_guard()?;
        if live.is_none() {
            return Err(CoordError::Abandoned);
        }
        if self.abandoned.load(Ordering::Acquire) {
            return Err(CoordError::Abandoned);
        }
        let trust = if current_boot.is_none() {
            Trust::Untrusted(Untrusted::CannotTell)
        } else if *current_boot != self.boot {
            Trust::Untrusted(Untrusted::HostRestarted)
        } else if self.words.word(Word::Ready).load(Ordering::Acquire) != format::READY {
            Trust::Untrusted(Untrusted::CannotTell)
        } else if self.words.word(Word::Vouched).load(Ordering::Acquire) == format::TRUE {
            Trust::Vouched
        } else {
            Trust::Untrusted(
                match self.words.word(Word::Reason).load(Ordering::Acquire) {
                    format::CREATED => Untrusted::Created,
                    format::RESTARTED => Untrusted::HostRestarted,
                    _ => Untrusted::CannotTell,
                },
            )
        };
        Ok(StateSnapshot {
            trust,
            fenced: trust != Trust::Vouched
                || self.words.word(Word::Fence).load(Ordering::Acquire) != format::FALSE,
            generation: self.words.word(Word::Generation).load(Ordering::Acquire),
            owner: self.words.word(Word::Owner).load(Ordering::Acquire),
            modification_count: self.words.word(Word::Count).load(Ordering::Acquire),
        })
    }

    /// Releases this participant's liveness without clearing ownership or other shared records.
    pub fn abandon(&self) {
        let _turn = self.release.enter();
        self.abandoned.store(true, Ordering::Release);
        let creation = self.creation.upgrade().map(|creation| {
            let mut creation = creation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *creation)
        });
        let holds = {
            let mut holds = self
                .holds
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *holds)
        };
        let live = {
            let mut live = self
                .live
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            live.take()
        };
        if let Some(creation) = creation {
            crate::coord::discard(creation.into_values());
        }
        crate::coord::discard(holds.into_values());
        crate::coord::discard(live);
    }
}
