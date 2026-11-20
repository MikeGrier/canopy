// Copyright (c) Michael Grier.

//! `acadia` -- the filesystem seam under Canopy (D55).
//!
//! Identifiers such as `D55` in these docs name design decisions in the project's design
//! records; they are kept for traceability and are not needed to use the API.
//!
//! This crate is the ground every other Canopy crate's storage stands on -- the one
//! place where filesystem intent is *specified*, so that the rest of the system
//! never touches `std::fs`/`tokio::fs` directly.
//!
//! # The seam
//!
//! [`Vfs`] is the complete filesystem operation surface a storage engine such as
//! `canopy` needs. It is the *specified contract*:
//! backends (a real OS passthrough, an in-memory mock with fault injection) are
//! chosen and written because they satisfy this trait, never the other way
//! around. Behavior is owned here, not inherited from whatever `std::fs` happens
//! to do on a given platform (design autonomy).
//!
//! Four properties are load-bearing and are part of the contract, not an
//! implementation accident:
//!
//! - **UTF-8 logical components, backend-native locators.** Every normal component of a path is
//!   valid UTF-8, regardless of backend; a directory listing returns each component as `String`.
//!   Prefixes and roots are physical locators owned by the backend or its caller, but they do not
//!   relax the component rule. A backend-facing path with a non-UTF-8 component, or a directory
//!   containing an entry that cannot cross this contract, is refused with
//!   [`VfsErrorKind::InvalidInput`], never lossy-converted. Acadia does not impose Canopy's narrower
//!   lowercase character grammar; it owns only the portable UTF-8 component boundary.
//! - **Path-oriented, object-safe.** Every operation names a path and returns an
//!   owned result, and there are no associated types, so a `Vfs` is usable as
//!   `Arc<dyn Vfs>` and shareable across the async daemon and the single-writer
//!   monitor. [`Vfs::lock_exclusive`] is the one deliberate exception to
//!   "owned result": it returns a *guard* whose lifetime is the lock, because a
//!   lock that is not tied to an owner's lifetime cannot be released when that
//!   owner dies (`D116`). It is still object-safe.
//! - **Errors are values, never panics (D56).** Every fallible operation returns
//!   [`VfsResult`]; a missing file, a permission problem, or an injected I/O
//!   fault is a [`VfsError`], never an `unwrap`/`expect`/`panic!`. The error
//!   taxonomy ([`VfsErrorKind`]) is Canopy-owned.
//! - **Durability is explicit.** [`Vfs::write`], [`Vfs::append`], and
//!   [`Vfs::truncate`] change file *contents* but make no durability promise;
//!   bytes reach stable storage only when [`Vfs::fsync`] returns. This is what
//!   lets the store place the single commit point of its log protocol (D53)
//!   exactly where it wants it.
//! - **Rename is atomic, not durable.** [`Vfs::rename`] replaces the destination
//!   atomically on the same filesystem. The new name is durable only after its parent is
//!   [`fsync`](Vfs::fsync)'d; a cross-directory move also requires the source parent to be
//!   synced to make removal of the old name durable.
//!
//! This file defines the contract. The real OS backend ([`OsFs`]) and the in-memory
//! mock ([`MemFs`]) build on top of it; fault injection ([`FaultyFs`]) layers over any
//! backend, and the conformance suite exercises both backends and the fault decorator
//! against this contract.
//!
//! # Coordination
//!
//! [`coord`] is the crate's second layer, above the seam: the contract through
//! which the participants on one store root -- threads and processes alike --
//! decide who may act and when. It depends on the seam and on [`signal`]; nothing
//! in them depends on it.

use std::fmt;
use std::path::{Component, Path, PathBuf};

pub mod coord;
pub mod fault;
pub mod mem;
pub mod names;
pub mod os;
pub mod read_root;
pub mod signal;

#[cfg(test)]
mod tests;

pub use fault::{
    Corruption, Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, PathPredicate, When,
};
pub use mem::MemFs;
pub use os::OsFs;
#[cfg(canopy_loom)]
pub use signal::LoomSignal;
pub use signal::{CondvarSignal, PlatformSignal, Signal, Watch};
#[cfg(target_os = "linux")]
pub use signal::{EventFdSignal, SealedWaitObject, SealedWaitOnlyFd, wait_native, wait_native_any};
#[cfg(windows)]
pub use signal::{EventSignal, NativeEvent, SealedWaitOnlyEvent, wait_native, wait_sealed};
/// The handle [`EventSignal::wait_only`] lends, named here so that a crate lending it on does not
/// depend on `win-sync-sys` for the name.
#[cfg(windows)]
pub use win_sync_sys::WaitOnlyEvent;

/// What separates a file's name from the suffix of the sibling [`Vfs::publish_new`] stages it in.
/// Changing it changes which leftovers of a publication are recognised as the publication's.
pub const PUBLISHING_INFIX: &str = ".publishing.";

/// Whether `name` is a sibling that a publication of a file named `target` stages its bytes in:
/// `target`, [`PUBLISHING_INFIX`] and a non-empty suffix. A crash can leave one behind, and it
/// carries no meaning ([`Vfs::publish_new`]).
pub fn is_publication_staging_of(name: &str, target: &str) -> bool {
    name.as_bytes()
        .strip_prefix(target.as_bytes())
        .and_then(|rest| rest.strip_prefix(PUBLISHING_INFIX.as_bytes()))
        .is_some_and(|suffix| !suffix.is_empty())
}

/// The result of a [`Vfs`] operation: success, or a [`VfsError`] value.
pub type VfsResult<T> = Result<T, VfsError>;

/// The category of a [`VfsError`].
///
/// A Canopy-owned taxonomy of the filesystem failures the store must reason
/// about. It is deliberately coarser than a raw OS errno list: it names the
/// distinctions callers act on (does the path exist? is it the wrong type? did a
/// write fault?) and folds everything else into [`VfsErrorKind::Other`]. It is
/// `#[non_exhaustive]` so new categories can be added without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VfsErrorKind {
    /// The named path does not exist.
    NotFound,
    /// The path already exists and the operation required it not to.
    AlreadyExists,
    /// A directory operation was attempted on a non-directory (or a component
    /// of the path was not a directory).
    NotADirectory,
    /// A file operation was attempted on a directory.
    IsADirectory,
    /// A directory could not be removed because it is not empty.
    DirectoryNotEmpty,
    /// The operation was refused for lack of permission.
    PermissionDenied,
    /// An argument was malformed (an empty or otherwise invalid path, a cross-
    /// filesystem rename, and similar caller errors).
    InvalidInput,
    /// End of file was reached before the requested bytes were available.
    UnexpectedEof,
    /// A write returned having persisted fewer bytes than requested (a short
    /// write) and could make no further progress.
    WriteZero,
    /// The underlying storage is full.
    StorageFull,
    /// The operation was interrupted and may be retried.
    Interrupted,
    /// The resource is held exclusively by another owner (`D116`).
    ///
    /// Distinct from [`PermissionDenied`](VfsErrorKind::PermissionDenied)
    /// because the caller acts on it differently: nothing is misconfigured, and
    /// retrying later may well succeed.
    Locked,
    /// The process view that issued the operation has ended and issues nothing more.
    ProcessEnded,
    /// The backend does not support the requested operation.
    Unsupported,
    /// Any failure that does not map onto a more specific category above.
    Other,
}

impl fmt::Display for VfsErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            VfsErrorKind::NotFound => "not found",
            VfsErrorKind::AlreadyExists => "already exists",
            VfsErrorKind::NotADirectory => "not a directory",
            VfsErrorKind::IsADirectory => "is a directory",
            VfsErrorKind::DirectoryNotEmpty => "directory not empty",
            VfsErrorKind::PermissionDenied => "permission denied",
            VfsErrorKind::InvalidInput => "invalid input",
            VfsErrorKind::UnexpectedEof => "unexpected end of file",
            VfsErrorKind::WriteZero => "write returned zero (short write)",
            VfsErrorKind::StorageFull => "storage full",
            VfsErrorKind::Interrupted => "interrupted",
            VfsErrorKind::Locked => "held by another owner",
            VfsErrorKind::ProcessEnded => "process ended",
            VfsErrorKind::Unsupported => "unsupported operation",
            VfsErrorKind::Other => "other filesystem error",
        };
        f.write_str(s)
    }
}

/// A filesystem operation failure.
///
/// Carries the [`VfsErrorKind`] category, the name of the operation that failed,
/// the path it was acting on (when there is a single relevant path), and a
/// human-readable message. It is a plain value: constructing or returning one
/// never touches the filesystem and never panics.
#[derive(Debug, Clone)]
pub struct VfsError {
    kind: VfsErrorKind,
    op: &'static str,
    path: Option<PathBuf>,
    message: String,
}

impl VfsError {
    /// Builds an error of `kind` for operation `op` (a short static verb such as
    /// `"read"` or `"rename"`) with a human-readable `message`. Attach the
    /// offending path with [`VfsError::with_path`].
    pub fn new(kind: VfsErrorKind, op: &'static str, message: impl Into<String>) -> Self {
        VfsError {
            kind,
            op,
            path: None,
            message: message.into(),
        }
    }

    /// Returns `self` with `path` recorded as the path the operation was acting
    /// on, for diagnostics.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// The failure category.
    pub fn kind(&self) -> VfsErrorKind {
        self.kind
    }

    /// The name of the operation that failed.
    pub fn op(&self) -> &'static str {
        self.op
    }

    /// The path the operation was acting on, if one was recorded.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The human-readable detail message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(
                f,
                "{} failed at {}: {}: {}",
                self.op,
                path.display(),
                self.kind,
                self.message
            ),
            None => write!(f, "{} failed: {}: {}", self.op, self.kind, self.message),
        }
    }
}

impl std::error::Error for VfsError {}

/// Whether a filesystem entry is a file, a directory, or something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// Anything else (symlink, device, socket, ...); the store treats these as
    /// opaque and does not act on them.
    Other,
}

/// The result of [`Vfs::metadata`]: what a path is and, for a file, how large.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    /// What kind of entry the path names.
    pub file_type: FileType,
    /// The file's length in bytes. Meaningful for [`FileType::File`]; zero for
    /// directories.
    pub len: u64,
}

impl Metadata {
    /// Whether the path names a regular file.
    pub fn is_file(&self) -> bool {
        self.file_type == FileType::File
    }

    /// Whether the path names a directory.
    pub fn is_dir(&self) -> bool {
        self.file_type == FileType::Dir
    }
}

/// One entry returned by [`Vfs::read_dir`].
///
/// Names the child's file name (the final path component, without the parent)
/// and its type. `.` and `..` are never reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// The entry's UTF-8 file name -- the final path component only.
    pub file_name: String,
    /// What kind of entry it is.
    pub file_type: FileType,
}

pub(crate) fn validate_path_components(path: &Path, op: &'static str) -> VfsResult<()> {
    for component in path.components() {
        if let Component::Normal(name) = component {
            let Some(name) = name.to_str() else {
                return Err(VfsError::new(
                    VfsErrorKind::InvalidInput,
                    op,
                    "a path component is not valid UTF-8",
                )
                .with_path(path));
            };
            if name.as_bytes().contains(&0) {
                return Err(VfsError::new(
                    VfsErrorKind::InvalidInput,
                    op,
                    "a path component contains NUL",
                )
                .with_path(path));
            }
        }
    }
    Ok(())
}

/// A held exclusive lock, released when it is dropped (`D116`).
///
/// The trait carries no methods: a lock's entire behaviour is its lifetime.
/// Holding the value is holding the lock, and there is deliberately no explicit
/// `release` -- an owner that must remember to call one is an owner that can
/// forget, and the case the lock exists for is the owner that never runs any
/// cleanup at all because its process died.
pub trait VfsLock: Send + Sync + fmt::Debug {}

/// Whether a lock taken by [`Vfs::try_lock`] admits other holders of the bytes it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockMode {
    /// Any number of shared holders at once; no exclusive holder alongside them.
    Shared,
    /// One holder, alongside no other of either mode.
    Exclusive,
}

/// The bytes of a file a lock taken by [`Vfs::try_lock`] covers.
///
/// Two locks on one file conflict only where their spans overlap, and a span may lie wholly
/// past the end of the file: a lock covers byte positions, not data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockSpan {
    /// Every position the file has or could have, overlapping every range.
    File,
    /// `len` positions from `offset`. `len` is at least one and the range ends at or before
    /// [`LockSpan::MAX_END`]; anything else is refused with [`VfsErrorKind::InvalidInput`].
    Range {
        /// The first position covered.
        offset: u64,
        /// How many positions are covered.
        len: u64,
    },
}

impl LockSpan {
    /// The position no range may extend past: the largest file offset Linux locks can name.
    pub const MAX_END: u64 = i64::MAX as u64;

    /// The half-open positions `[start, end)` this span covers, or `None` if it is invalid.
    /// [`LockSpan::File`] covers `[0, u64::MAX)`.
    #[must_use]
    pub fn bounds(self) -> Option<(u64, u64)> {
        match self {
            LockSpan::File => Some((0, u64::MAX)),
            LockSpan::Range { offset, len } => {
                let end = offset.checked_add(len)?;
                (len > 0 && end <= LockSpan::MAX_END).then_some((offset, end))
            }
        }
    }
}

/// The filesystem seam: the complete set of operations Canopy storage performs.
///
/// See the [crate documentation](crate) for the contract's load-bearing
/// properties (path-oriented and object-safe, errors-as-values, explicit
/// durability, atomic-but-not-durable rename).
///
/// Implementors are shareable across threads (`Send + Sync`) so a single backend
/// can be handed to the async daemon and the single-writer monitor at once;
/// interior mutability, where a backend needs it, is the backend's concern. A process view that
/// has ended issues nothing more: once its end returns, its waits end and every operation through
/// it fails with [`VfsErrorKind::ProcessEnded`]. A host restart ends every process view.
///
/// Unless stated otherwise, every operation requires the parent directory of
/// `path` to already exist and fails with [`VfsErrorKind::NotFound`] if it does
/// not; directories are created only by [`create_dir`](Vfs::create_dir) and
/// [`create_dir_all`](Vfs::create_dir_all).
///
/// Every normal path component must be valid UTF-8 and contain no NUL. Decorators validate this
/// before applying their own behavior, and a listing returns each final component as `String`.
/// A foreign native entry is therefore an [`InvalidInput`](VfsErrorKind::InvalidInput) refusal of
/// the listing, not a lossy filename conversion.
pub trait Vfs: Send + Sync {
    /// Refuses an operation through a process view that has ended.
    ///
    /// Backends whose process identity is meaningful override this; decorators call the wrapped
    /// backend before selecting any behavior of their own. Other backends have no extra process
    /// view state to check.
    fn check_process(&self, op: &'static str, path: &Path) -> VfsResult<()> {
        validate_path_components(path, op)
    }

    /// Runs `commit` as one process-liveness decision: a backend that can end process views keeps
    /// restart ordered before or after the call. Arbitrary matching and user callbacks belong
    /// outside this hook; only the state commit named by the operation belongs inside it.
    fn commit_process(
        &self,
        op: &'static str,
        path: &Path,
        commit: &mut dyn FnMut(),
    ) -> VfsResult<()> {
        self.check_process(op, path)?;
        commit();
        Ok(())
    }

    /// Reads the entire contents of the file at `path`.
    ///
    /// Fails with [`VfsErrorKind::NotFound`] if the file does not exist and
    /// [`VfsErrorKind::IsADirectory`] if `path` names a directory.
    fn read(&self, path: &Path) -> VfsResult<Vec<u8>>;

    /// Reads up to `len` bytes of the file at `path`, starting at byte `offset`.
    ///
    /// Returns fewer than `len` bytes when the file ends first, and an empty
    /// vector when `offset` is at or past the end: reaching the end is not an
    /// error. It never allocates for bytes the file does not have, so a caller may
    /// pass a `len` larger than the file. [`OsFs`] refuses a result larger than 256 MiB with
    /// [`VfsErrorKind::StorageFull`] before reserving memory; larger reads must be made in chunks.
    ///
    /// This is the read of a file that is **still being written**: a reader
    /// following a growing output file asks for the next slice from where it left
    /// off, and sees whatever the writer has put there so far. It takes no lock
    /// and holds nothing open afterwards, so it never stands between a writer and
    /// its file, and a writer never stands between it and the bytes.
    ///
    /// Fails with [`VfsErrorKind::NotFound`] if the file does not exist and
    /// [`VfsErrorKind::IsADirectory`] if `path` names a directory.
    fn read_at(&self, path: &Path, offset: u64, len: usize) -> VfsResult<Vec<u8>>;

    /// Writes `contents` as the entire contents of the file at `path`, creating
    /// the file if absent and truncating it if present.
    ///
    /// This changes file *contents* only; the bytes are not guaranteed durable
    /// until a subsequent [`fsync`](Vfs::fsync) of `path` returns.
    fn write(&self, path: &Path, contents: &[u8]) -> VfsResult<()>;

    /// Appends `contents` to the end of the file at `path`, creating the file if
    /// absent.
    ///
    /// Like [`write`](Vfs::write), this is not durable until `path` is
    /// [`fsync`](Vfs::fsync)'d. This is the primitive the append-only commit log
    /// (D53) is built on.
    fn append(&self, path: &Path, contents: &[u8]) -> VfsResult<()>;

    /// Sets the length of the file at `path` to `len` bytes, growing it with
    /// zero bytes or discarding the tail as needed.
    ///
    /// `len == 0` empties the file, which is how the log is reset at quiescence
    /// (D53). Not durable until `path` is [`fsync`](Vfs::fsync)'d.
    fn truncate(&self, path: &Path, len: u64) -> VfsResult<()>;

    /// Flushes the file or directory at `path` to durable storage.
    ///
    /// After this returns, all prior [`write`](Vfs::write) /
    /// [`append`](Vfs::append) / [`truncate`](Vfs::truncate) to the file at
    /// `path` are persisted. When `path` names a directory, prior name changes
    /// within it -- [`rename`](Vfs::rename), file creation, removal -- are made
    /// durable. This is the store's explicit commit point (D53).
    fn fsync(&self, path: &Path) -> VfsResult<()>;

    /// Atomically renames `from` to `to`, replacing `to` if it already exists.
    ///
    /// `from` and `to` must be on the same filesystem (a cross-filesystem
    /// rename fails with [`VfsErrorKind::InvalidInput`]). The rename is atomic --
    /// an observer sees either the old or the new `to`, never a partial state --
    /// but its effect is durable only after the affected parent directories are
    /// [`fsync`](Vfs::fsync)'d: the destination parent names the new entry, and a different source
    /// parent must also be synced to make removal of the old name durable. This is the store's
    /// stage-then-rename primitive.
    fn rename(&self, from: &Path, to: &Path) -> VfsResult<()>;

    /// Removes the file at `path`.
    ///
    /// Fails with [`VfsErrorKind::NotFound`] if it does not exist and
    /// [`VfsErrorKind::IsADirectory`] if `path` names a directory (use
    /// [`remove_dir`](Vfs::remove_dir)).
    fn remove_file(&self, path: &Path) -> VfsResult<()>;

    /// Creates a single directory at `path`.
    ///
    /// The parent must already exist. Fails with [`VfsErrorKind::AlreadyExists`]
    /// if `path` already exists.
    fn create_dir(&self, path: &Path) -> VfsResult<()>;

    /// Creates the directory at `path` and any missing parents.
    ///
    /// Succeeds if `path` already exists as a directory.
    fn create_dir_all(&self, path: &Path) -> VfsResult<()>;

    /// Removes the empty directory at `path`.
    ///
    /// Fails with [`VfsErrorKind::DirectoryNotEmpty`] if it still has children
    /// (use [`remove_dir_all`](Vfs::remove_dir_all)).
    fn remove_dir(&self, path: &Path) -> VfsResult<()>;

    /// Removes the directory at `path` and everything under it.
    ///
    /// This is how the store discards a staging directory after a transaction.
    fn remove_dir_all(&self, path: &Path) -> VfsResult<()>;

    /// Lists the entries directly under the directory at `path`.
    ///
    /// The order is unspecified and `.`/`..` are excluded. Fails with
    /// [`VfsErrorKind::NotADirectory`] if `path` is not a directory.
    fn read_dir(&self, path: &Path) -> VfsResult<Vec<DirEntry>>;

    /// Returns the [`Metadata`] of the referent of `path`, following a final symbolic link.
    /// [`read_dir`](Vfs::read_dir) reports a link entry itself as [`FileType::Other`]. On Unix,
    /// [`rename`](Vfs::rename) moves or replaces the final entry, and [`remove_file`](Vfs::remove_file)
    /// unlinks it; neither operation mutates the final link's referent.
    ///
    /// Fails with [`VfsErrorKind::NotFound`] if `path` names nothing, including a link that leads
    /// nowhere.
    fn metadata(&self, path: &Path) -> VfsResult<Metadata>;

    /// Whether `path` names anything, following symbolic links as [`metadata`](Vfs::metadata) does.
    ///
    /// A convenience over [`metadata`](Vfs::metadata): a
    /// [`VfsErrorKind::NotFound`] becomes `Ok(false)`; any other error still
    /// propagates, because "does it exist?" is genuinely unanswerable when, for
    /// example, permission is denied.
    fn exists(&self, path: &Path) -> VfsResult<bool> {
        match self.metadata(path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == VfsErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Takes an exclusive lock named by `path`, creating the regular file if absent, and
    /// returns the guard that holds it (`D116`). An existing final component must be a regular
    /// file; [`OsFs`] does not follow a final symlink or reparse point while opening it.
    ///
    /// The contract is **exclusion among openers of this filesystem**, not "a
    /// kernel lock". Each backend honours it at its own scope: [`OsFs`] excludes
    /// every process on the machine, while a [`MemFs`] instance's whole universe
    /// is one process, so an in-process lock is the faithful implementation there
    /// rather than a stub. A lock taken over `MemFs` is therefore *not* evidence
    /// of cross-process exclusion, and a test that needs that property must use
    /// `OsFs` and a real second process.
    ///
    /// The call **never blocks**: a lock held by another owner fails immediately
    /// with [`VfsErrorKind::Locked`] rather than waiting. Waiting would turn a
    /// misconfiguration -- two daemons pointed at one root -- into a hang, which
    /// is harder to diagnose than a refusal.
    ///
    /// The lock is released when the returned guard is dropped. The operating system also releases
    /// it when the owning process dies and no other process retains an inherited descriptor for the
    /// same open file description. On Unix a process must not `fork` while retaining this guard
    /// unless every child closes or `exec`s away the inherited description; the kernel lock follows
    /// the open description, not the Rust guard. This is the property the whole
    /// operation exists for. It follows that the file at `path` may outlive the
    /// lock: its existence carries no meaning and it must never be treated as a
    /// stale lock to reap. Only a live holder means anything.
    ///
    /// The default implementation reports [`VfsErrorKind::Unsupported`], so a
    /// backend that offers no exclusion says so rather than pretending. **A
    /// decorator that wraps another `Vfs` must forward this explicitly**; the
    /// default would otherwise silently downgrade a backend that does support it.
    fn lock_exclusive(&self, path: &Path) -> VfsResult<Box<dyn VfsLock>> {
        Err(VfsError::new(VfsErrorKind::Unsupported, "lock_exclusive", {
            "this backend offers no exclusion"
        })
        .with_path(path))
    }

    /// Takes a lock of `mode` on the `span` of the regular file at `path`, creating it if absent
    /// and never truncating it, and returns the guard that holds it. [`OsFs`] does not follow a
    /// final symlink or reparse point while opening the path.
    ///
    /// The call **never waits**: a span that overlaps one another holder has in a conflicting
    /// mode -- either of the two exclusive -- fails at once with [`VfsErrorKind::Locked`].
    /// **Every guard is its own holder**, so two guards taken in one process, or on one thread,
    /// conflict exactly as guards in two processes do.
    ///
    /// The lock is released when the guard is dropped. After process death the operating system
    /// releases it once no retained inherited description keeps it alive; on Unix a forked child
    /// must close or `exec` away inherited descriptions. A Windows process-death release may lag
    /// the
    /// process's exit by a moment; a successor that must not fail on that lag retries.
    ///
    /// # A locked span holds no data
    ///
    /// What a read or write through the seam does to bytes inside a span another holder has
    /// locked is the platform's: Windows refuses a read of an exclusively locked byte and a
    /// write of any locked byte -- the holder's own included, since every seam operation opens
    /// its own handle -- while Linux does neither. [`MemFs`] refuses as Windows does, with
    /// [`VfsErrorKind::Locked`]. So data a caller reads or writes lives outside every span
    /// anyone locks, and a lock that needs a place of its own takes a range past the data, or a
    /// file of its own with [`LockSpan::File`].
    ///
    /// # Not to be mixed with `lock_exclusive`
    ///
    /// A file is locked through this or through [`lock_exclusive`](Vfs::lock_exclusive), never
    /// both: whether the two exclude each other differs between platforms.
    ///
    /// [`OsFs`] takes `LockFileEx` locks on Windows and open-file-description locks on Linux,
    /// each on a handle of its own; on another Unix it takes `flock` for [`LockSpan::File`] and
    /// reports a range as [`VfsErrorKind::Unsupported`]. An invalid span is
    /// [`VfsErrorKind::InvalidInput`]. The default implementation reports
    /// [`VfsErrorKind::Unsupported`], and **a decorator must forward this explicitly**, for the
    /// reason given at [`lock_exclusive`](Vfs::lock_exclusive).
    fn try_lock(&self, path: &Path, mode: LockMode, span: LockSpan) -> VfsResult<Box<dyn VfsLock>> {
        let _ = (mode, span);
        Err(VfsError::new(
            VfsErrorKind::Unsupported,
            "try_lock",
            "this backend offers no locks",
        )
        .with_path(path))
    }

    /// Takes a lock of `mode` on the `span` of the file at `path` as
    /// [`try_lock`](Vfs::try_lock) does, but **waits** while another holder's lock conflicts,
    /// and returns once it is granted.
    ///
    /// Everything [`try_lock`](Vfs::try_lock) states holds here too, and two more things:
    ///
    /// - **A waiter is woken by any release that lets it in**: a guard dropped, or its holder's
    ///   process dying. No order among waiters is promised, and a waiter may be passed over by
    ///   one that arrives later.
    /// - **The wait cannot be bounded or cancelled.** A caller that holds a conflicting guard on
    ///   the same thread waits for ever, as two conflicting guards would across processes.
    ///
    /// [`OsFs`] waits in `LockFileEx` on Windows and in an open-file-description lock's wait on
    /// Linux; on another Unix it waits in `flock` for [`LockSpan::File`] and reports a range as
    /// [`VfsErrorKind::Unsupported`]. The default implementation reports
    /// [`VfsErrorKind::Unsupported`], and **a decorator must forward this explicitly**, for the
    /// reason given at [`lock_exclusive`](Vfs::lock_exclusive).
    fn lock(&self, path: &Path, mode: LockMode, span: LockSpan) -> VfsResult<Box<dyn VfsLock>> {
        let _ = (mode, span);
        Err(VfsError::new(
            VfsErrorKind::Unsupported,
            "lock",
            "this backend offers no locks",
        )
        .with_path(path))
    }

    /// Waits for `mode` on an existing regular file. Unlike [`lock`](Vfs::lock), this never
    /// creates `path`; a missing final entry is [`VfsErrorKind::NotFound`].
    fn lock_existing(
        &self,
        path: &Path,
        mode: LockMode,
        span: LockSpan,
    ) -> VfsResult<Box<dyn VfsLock>> {
        let _ = (mode, span);
        Err(VfsError::new(
            VfsErrorKind::Unsupported,
            "lock_existing",
            "this backend cannot lock an existing file without creating it",
        )
        .with_path(path))
    }

    /// Creates the file at `path` holding exactly `contents`, if nothing is there.
    ///
    /// Anything already at `path` -- a file or a directory -- fails it with
    /// [`VfsErrorKind::AlreadyExists`] and changes nothing, so of any number of publishers
    /// racing for one path exactly one succeeds. Any other failure may leave the file published
    /// whole, as a crash after its publication would.
    ///
    /// **The file is never visible incomplete, and a power loss leaves it complete or absent.**
    /// Its bytes are durable before its name appears. The name itself is durable once the
    /// parent directory is [`fsync`](Vfs::fsync)'d, as for a [`rename`](Vfs::rename); until
    /// then a crash may lose it, but never leaves it holding less than `contents`.
    ///
    /// The bytes are staged in a sibling whose name is the file's name followed by
    /// [`PUBLISHING_INFIX`] and a suffix ([`is_publication_staging_of`]), removed whether the
    /// publication succeeds or fails. A crash can leave one behind; it carries no meaning, may be
    /// removed, and does not keep a later publication from succeeding.
    ///
    /// The published file has one name, so a [`rename`](Vfs::rename) may replace it as soon as
    /// this returns.
    ///
    /// The default implementation reports [`VfsErrorKind::Unsupported`], and **a decorator must
    /// forward this explicitly**, for the reason given at [`lock_exclusive`](Vfs::lock_exclusive).
    fn publish_new(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        let _ = contents;
        Err(VfsError::new(
            VfsErrorKind::Unsupported,
            "publish_new",
            "this backend cannot publish",
        )
        .with_path(path))
    }

    /// Opens `path` as an operating-system file handle for something outside the
    /// seam to write into, creating the file if absent and **truncating** it if
    /// present.
    ///
    /// This exists for one caller shape: a child process that is handed the
    /// handle as its standard output or error and writes straight into it, while
    /// the opener never sees a byte. The seam's other operations take and return
    /// bytes and so cannot express that, and leaving the open off the seam would
    /// make the one place a handle is created the one place a test cannot make
    /// fail. The handle is the caller's from here on: nothing the seam does
    /// afterwards, including a [`FaultyFs`] rule, reaches a write made through it.
    ///
    /// Readers must be able to open the same file concurrently, because the
    /// output is tailed while it is written.
    ///
    /// The default implementation reports [`VfsErrorKind::Unsupported`]: a backend
    /// with no operating-system files, such as [`MemFs`], cannot hand one out and
    /// says so. **A decorator that wraps another `Vfs` must forward this
    /// explicitly**, for the reason given at [`lock_exclusive`](Vfs::lock_exclusive).
    fn open_output(&self, path: &Path) -> VfsResult<std::fs::File> {
        self.check_process("open_output", path)?;
        Err(VfsError::new(VfsErrorKind::Unsupported, "open_output", {
            "this backend has no operating-system files to hand out"
        })
        .with_path(path))
    }

    /// Whether the paths this filesystem names are paths in the operating
    /// system's own, so that something outside the seam -- a directory watch, a
    /// child process -- sees the same files the seam does.
    ///
    /// A caller that wants an operating-system facility over a directory asks
    /// this first, and does without when it is `false`. Defaults to `false`: a
    /// backend says it is the real filesystem or it is not, and the in-memory one
    /// is not. **A decorator must forward this**, since a wrapper does not change
    /// what the wrapped filesystem is.
    fn is_os_filesystem(&self) -> bool {
        false
    }

    /// An identity of the current boot of the host this filesystem lives on: the same on every
    /// call, from every process, while the host runs, and different once it has restarted.
    ///
    /// It is what lets state kept only in memory, or in files never synced, tell whether it has
    /// survived: a restart loses such state, and the identity says one happened. [`OsFs`] reads
    /// the identity the kernel generates at boot on Linux and the boot sequence number Windows
    /// keeps in the data it shares with every process; [`MemFs`] counts its simulated crashes,
    /// since [`MemFs::simulate_crash`] is its power loss.
    ///
    /// The default implementation reports [`VfsErrorKind::Unsupported`], as does [`OsFs`] on a
    /// platform with no such identity, and **a decorator must forward this explicitly**, for the
    /// reason given at [`lock_exclusive`](Vfs::lock_exclusive).
    fn boot_identity(&self) -> VfsResult<Vec<u8>> {
        Err(VfsError::new(
            VfsErrorKind::Unsupported,
            "boot_identity",
            "this backend cannot identify a boot",
        ))
    }
}

impl From<VfsError> for std::io::Error {
    /// The `io::Error` whose kind is the nearest of the standard ones, with the
    /// [`VfsError`] kept as its payload, for callers whose own contract is
    /// `io::Result`.
    fn from(error: VfsError) -> std::io::Error {
        use std::io::ErrorKind;
        let kind = match error.kind() {
            VfsErrorKind::NotFound => ErrorKind::NotFound,
            VfsErrorKind::AlreadyExists => ErrorKind::AlreadyExists,
            VfsErrorKind::NotADirectory => ErrorKind::NotADirectory,
            VfsErrorKind::IsADirectory => ErrorKind::IsADirectory,
            VfsErrorKind::DirectoryNotEmpty => ErrorKind::DirectoryNotEmpty,
            VfsErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
            VfsErrorKind::InvalidInput => ErrorKind::InvalidInput,
            VfsErrorKind::UnexpectedEof => ErrorKind::UnexpectedEof,
            VfsErrorKind::WriteZero => ErrorKind::WriteZero,
            VfsErrorKind::StorageFull => ErrorKind::StorageFull,
            VfsErrorKind::Interrupted => ErrorKind::Interrupted,
            VfsErrorKind::Unsupported => ErrorKind::Unsupported,
            _ => ErrorKind::Other,
        };
        std::io::Error::new(kind, error)
    }
}
