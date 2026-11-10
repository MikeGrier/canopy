// Copyright (c) Michael Grier.

//! `acadia` -- the filesystem seam under Canopy (D55).
//!
//! Identifiers such as `D55` or `VFS-2` in these docs name design decisions and work
//! items in the project's design records; they are kept for traceability and are not
//! needed to use the API.
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
//!   atomically on the same filesystem; the *effect* of the rename is durable
//!   only after the containing directory is [`fsync`](Vfs::fsync)'d.
//!
//! This file defines the contract (VFS-1). The real OS backend ([`OsFs`],
//! VFS-2) and the in-memory mock ([`MemFs`], VFS-3) build on top of it; fault
//! injection ([`FaultyFs`], VFS-4) layers over any backend, and the conformance
//! suite (VFS-5) exercises both backends and the fault decorator against this
//! contract.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

pub mod fault;
pub mod mem;
pub mod os;
pub mod read_root;

#[cfg(test)]
mod tests;

pub use fault::{
    Corruption, Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, PathPredicate, When,
};
pub use mem::MemFs;
pub use os::OsFs;

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
    /// The entry's file name -- the final path component only.
    pub file_name: OsString,
    /// What kind of entry it is.
    pub file_type: FileType,
}

/// A held exclusive lock, released when it is dropped (`D116`).
///
/// The trait carries no methods: a lock's entire behaviour is its lifetime.
/// Holding the value is holding the lock, and there is deliberately no explicit
/// `release` -- an owner that must remember to call one is an owner that can
/// forget, and the case the lock exists for is the owner that never runs any
/// cleanup at all because its process died.
pub trait VfsLock: Send + Sync + fmt::Debug {}

/// The filesystem seam: the complete set of operations Canopy storage performs.
///
/// See the [crate documentation](crate) for the contract's load-bearing
/// properties (path-oriented and object-safe, errors-as-values, explicit
/// durability, atomic-but-not-durable rename).
///
/// Implementors are shareable across threads (`Send + Sync`) so a single backend
/// can be handed to the async daemon and the single-writer monitor at once;
/// interior mutability, where a backend needs it, is the backend's concern.
///
/// Unless stated otherwise, every operation requires the parent directory of
/// `path` to already exist and fails with [`VfsErrorKind::NotFound`] if it does
/// not; directories are created only by [`create_dir`](Vfs::create_dir) and
/// [`create_dir_all`](Vfs::create_dir_all).
pub trait Vfs: Send + Sync {
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
    /// pass a `len` larger than the file.
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
    /// but its effect is durable only after the containing directory is
    /// [`fsync`](Vfs::fsync)'d. This is the store's stage-then-rename primitive.
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

    /// Returns the [`Metadata`] for the entry at `path`.
    ///
    /// Fails with [`VfsErrorKind::NotFound`] if nothing exists at `path`.
    fn metadata(&self, path: &Path) -> VfsResult<Metadata>;

    /// Whether anything exists at `path`.
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

    /// Takes an exclusive lock named by `path`, creating the file if absent, and
    /// returns the guard that holds it (`D116`).
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
    /// The lock is released when the returned guard is dropped, **and when the
    /// owning process dies by any means**, which is the property the whole
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
