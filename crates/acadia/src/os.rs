// Copyright (c) Michael Grier.

//! The real operating-system [`Vfs`] backend (VFS-2, D55).
//!
//! [`OsFs`] is a thin passthrough that implements the [`Vfs`] contract over
//! `std::fs`. It is the *only* place in the whole workspace that touches
//! `std::fs` directly; every other crate reaches the filesystem through the
//! [`Vfs`] seam so it can be mocked and fault-injected (D55/D56).
//!
//! Its single job beyond delegation is to translate `std::io::Error` into the
//! Canopy-owned [`VfsError`] taxonomy: the OS reports errno-flavored
//! [`io::ErrorKind`]s, and `map_kind` folds those onto the coarser
//! [`VfsErrorKind`] categories the store reasons about (design autonomy -- the
//! taxonomy is ours, `std` is merely the mechanism that satisfies it).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::{DirEntry, FileType, Metadata, Vfs, VfsError, VfsErrorKind, VfsLock, VfsResult};

#[cfg(not(any(unix, windows)))]
compile_error!("acadia's OsFs supports Unix and Windows targets only");

/// The real filesystem: a [`Vfs`] implemented as a thin passthrough over
/// `std::fs`.
///
/// `OsFs` is stateless and cheap to clone/share; hand it out as
/// `Arc<dyn Vfs>`. All durability and atomicity guarantees are exactly those of
/// the underlying platform as described by the [`Vfs`] contract:
/// [`write`](Vfs::write)/[`append`](Vfs::append)/[`truncate`](Vfs::truncate)
/// are durable only after [`fsync`](Vfs::fsync), and [`rename`](Vfs::rename) is
/// atomic but its effect is durable only after the containing directory is
/// fsync'd.
///
/// # fsync portability
///
/// On Unix a directory is fsync'd by opening it and calling `sync_all`, which is
/// what makes a prior [`rename`](Vfs::rename) durable. On Windows the directory is
/// opened with backup semantics and write access and flushed with
/// `FlushFileBuffers`; a failure to open or flush it (restrictive ACLs, a filesystem
/// that refuses directory handles) is reported rather than ignored. Only Unix and
/// Windows are supported targets.
///
/// File fsync works on every platform, but the syncing handle must be opened
/// differently: Windows `FlushFileBuffers` (which `sync_all` calls) requires a
/// write-capable handle, whereas Unix flushes happily from a read handle, so the
/// file is opened for write on Windows and for read elsewhere.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsFs;

impl OsFs {
    /// Creates a handle to the real filesystem.
    #[must_use]
    pub fn new() -> Self {
        OsFs
    }
}

/// Maps a `std::io::ErrorKind` onto the Canopy-owned [`VfsErrorKind`] taxonomy.
///
/// `io::ErrorKind` is `#[non_exhaustive]`; kinds without a dedicated category --
/// including the network kinds, which the local filesystem never produces -- are
/// folded into [`VfsErrorKind::Other`].
fn map_kind(kind: io::ErrorKind) -> VfsErrorKind {
    match kind {
        io::ErrorKind::NotFound => VfsErrorKind::NotFound,
        io::ErrorKind::AlreadyExists => VfsErrorKind::AlreadyExists,
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            VfsErrorKind::PermissionDenied
        }
        // A cross-filesystem rename is a caller error under our contract.
        io::ErrorKind::InvalidInput | io::ErrorKind::CrossesDevices => VfsErrorKind::InvalidInput,
        io::ErrorKind::UnexpectedEof => VfsErrorKind::UnexpectedEof,
        io::ErrorKind::WriteZero => VfsErrorKind::WriteZero,
        io::ErrorKind::StorageFull => VfsErrorKind::StorageFull,
        io::ErrorKind::Interrupted => VfsErrorKind::Interrupted,
        io::ErrorKind::Unsupported => VfsErrorKind::Unsupported,
        io::ErrorKind::NotADirectory => VfsErrorKind::NotADirectory,
        io::ErrorKind::IsADirectory => VfsErrorKind::IsADirectory,
        io::ErrorKind::DirectoryNotEmpty => VfsErrorKind::DirectoryNotEmpty,
        _ => VfsErrorKind::Other,
    }
}

/// Builds a [`VfsError`] for operation `op` on `path` from an `io::Error`.
fn err(op: &'static str, path: &Path, e: &io::Error) -> VfsError {
    VfsError::new(map_kind(e.kind()), op, e.to_string()).with_path(path)
}

/// Converts a `std::fs::FileType` into the seam's [`FileType`].
fn file_type_of(ft: fs::FileType) -> FileType {
    if ft.is_file() {
        FileType::File
    } else if ft.is_dir() {
        FileType::Dir
    } else {
        FileType::Other
    }
}

/// One positioned read from `file` at absolute `offset`, which does not move a shared seek cursor.
#[cfg(windows)]
fn read_positioned(file: &fs::File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

/// One positioned read from `file` at absolute `offset`.
#[cfg(unix)]
fn read_positioned(file: &fs::File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

/// A held [`OsFs`] lock (`D116`): the open handle *is* the lock.
///
/// The handle is never read or written. Dropping it -- or the process exiting by
/// any means, including a hard kill -- closes it, and the OS releases the lock at
/// that instant. That is the whole reason this is an OS lock rather than a marker
/// file recording a pid: the kernel cleans up after an owner that ran no
/// destructor, where a marker file would linger and wedge the store.
#[derive(Debug)]
struct OsLock {
    _handle: fs::File,
}

impl VfsLock for OsLock {}

/// Opens `path` such that the returned handle holds an exclusive lock on it.
///
/// Windows has no advisory-lock call to make here: the exclusion *is* the share
/// mode. Opening with `share_mode(0)` means a second opener is refused by the
/// object manager with a sharing violation, so acquisition and open are one
/// operation and cannot race.
#[cfg(windows)]
fn open_exclusive(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        // Never truncate: the handle is wanted for its exclusivity alone, and the
        // file may be held by an owner whose bytes are none of our business.
        .truncate(false)
        .share_mode(0)
        .open(path)
}

/// Opens `path` and takes a non-blocking exclusive `flock` on the handle.
///
/// Unlike Windows, the open itself excludes nothing on Unix, so the lock is a
/// second step. `LOCK_NB` keeps it from blocking: a contended root must fail, not
/// hang.
#[cfg(unix)]
fn open_exclusive(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::io::AsRawFd;

    let handle = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    // SAFETY: `handle` owns a valid open descriptor for the duration of the call.
    if unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

/// Opens a directory handle that `sync_all` can flush.
#[cfg(unix)]
fn open_dir_for_sync(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}

/// `FILE_FLAG_BACKUP_SEMANTICS` -- required to open a directory handle at all.
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

/// Opens a directory handle that `sync_all` (`FlushFileBuffers`) can flush.
///
/// `FlushFileBuffers` needs a write-capable handle, and a directory handle needs
/// backup semantics. Win32 does not document what flushing a directory handle
/// persists; this is the strongest request the platform offers for the directory's
/// entries, and any failure is surfaced to the caller.
#[cfg(windows)]
fn open_dir_for_sync(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// `ERROR_SHARING_VIOLATION` -- another handle holds the file with an
/// incompatible share mode.
#[cfg(windows)]
const ERROR_SHARING_VIOLATION: i32 = 32;

/// `ERROR_LOCK_VIOLATION` -- a byte-range lock refused the access.
#[cfg(windows)]
const ERROR_LOCK_VIOLATION: i32 = 33;

/// Whether `e` means "someone else holds this", as opposed to a real failure.
///
/// Contention arrives as a platform-specific errno that `io::ErrorKind` does not
/// name, so it must be recognised here rather than left to fold into
/// [`VfsErrorKind::Other`] -- the caller's whole decision turns on this
/// distinction.
fn is_contention(e: &io::Error) -> bool {
    #[cfg(windows)]
    {
        matches!(
            e.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION)
        )
    }
    #[cfg(unix)]
    {
        // `flock` reports contention as `EWOULDBLOCK` only. `EACCES` comes from the
        // open that precedes it, so it is a permission problem, not a holder.
        e.raw_os_error() == Some(libc::EWOULDBLOCK)
    }
}

impl Vfs for OsFs {
    fn read(&self, path: &Path) -> VfsResult<Vec<u8>> {
        fs::read(path).map_err(|e| err("read", path, &e))
    }

    fn read_at(&self, path: &Path, offset: u64, len: usize) -> VfsResult<Vec<u8>> {
        let file = fs::File::open(path).map_err(|e| err("read_at", path, &e))?;
        let metadata = file.metadata().map_err(|e| err("read_at", path, &e))?;
        // Unix opens a directory for reading; checked before the range so an empty or
        // past-the-end read cannot succeed on one.
        if metadata.is_dir() {
            return Err(
                VfsError::new(VfsErrorKind::IsADirectory, "read_at", "is a directory")
                    .with_path(path),
            );
        }
        let file_len = metadata.len();
        // Bounded by what the file holds, so a `len` larger than the file allocates nothing
        // for the difference.
        let wanted = file_len
            .saturating_sub(offset)
            .min(u64::try_from(len).unwrap_or(u64::MAX));
        let mut bytes = vec![0u8; usize::try_from(wanted).unwrap_or(0)];
        let mut filled = 0usize;
        while filled < bytes.len() {
            let read = read_positioned(&file, &mut bytes[filled..], offset + filled as u64)
                .map_err(|e| err("read_at", path, &e))?;
            if read == 0 {
                // The file shrank under the read; what was there is what there is.
                break;
            }
            filled += read;
        }
        bytes.truncate(filled);
        Ok(bytes)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        fs::write(path, contents).map_err(|e| err("write", path, &e))
    }

    fn append(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| err("append", path, &e))?;
        file.write_all(contents)
            .map_err(|e| err("append", path, &e))
    }

    fn truncate(&self, path: &Path, len: u64) -> VfsResult<()> {
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| err("truncate", path, &e))?;
        file.set_len(len).map_err(|e| err("truncate", path, &e))
    }

    fn fsync(&self, path: &Path) -> VfsResult<()> {
        let meta = fs::metadata(path).map_err(|e| err("fsync", path, &e))?;
        if meta.is_dir() {
            open_dir_for_sync(path)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| err("fsync", path, &e))
        } else {
            // Windows FlushFileBuffers needs a write-capable handle; Unix syncs
            // fine from a read handle (and read avoids failing on read-only
            // files that have no pending writes to flush).
            #[cfg(windows)]
            let opened = OpenOptions::new().write(true).open(path);
            #[cfg(not(windows))]
            let opened = OpenOptions::new().read(true).open(path);
            let file = opened.map_err(|e| err("fsync", path, &e))?;
            file.sync_all().map_err(|e| err("fsync", path, &e))
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> VfsResult<()> {
        fs::rename(from, to).map_err(|e| err("rename", from, &e))
    }

    fn remove_file(&self, path: &Path) -> VfsResult<()> {
        fs::remove_file(path).map_err(|e| err("remove_file", path, &e))
    }

    fn create_dir(&self, path: &Path) -> VfsResult<()> {
        fs::create_dir(path).map_err(|e| err("create_dir", path, &e))
    }

    fn create_dir_all(&self, path: &Path) -> VfsResult<()> {
        fs::create_dir_all(path).map_err(|e| err("create_dir_all", path, &e))
    }

    fn remove_dir(&self, path: &Path) -> VfsResult<()> {
        fs::remove_dir(path).map_err(|e| err("remove_dir", path, &e))
    }

    fn remove_dir_all(&self, path: &Path) -> VfsResult<()> {
        fs::remove_dir_all(path).map_err(|e| err("remove_dir_all", path, &e))
    }

    fn read_dir(&self, path: &Path) -> VfsResult<Vec<DirEntry>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path).map_err(|e| err("read_dir", path, &e))? {
            let entry = entry.map_err(|e| err("read_dir", path, &e))?;
            let file_type = file_type_of(entry.file_type().map_err(|e| err("read_dir", path, &e))?);
            entries.push(DirEntry {
                file_name: entry.file_name(),
                file_type,
            });
        }
        Ok(entries)
    }

    fn metadata(&self, path: &Path) -> VfsResult<Metadata> {
        let meta = fs::metadata(path).map_err(|e| err("metadata", path, &e))?;
        Ok(Metadata {
            file_type: file_type_of(meta.file_type()),
            len: meta.len(),
        })
    }

    fn lock_exclusive(&self, path: &Path) -> VfsResult<Box<dyn VfsLock>> {
        match open_exclusive(path) {
            Ok(handle) => Ok(Box::new(OsLock { _handle: handle })),
            Err(e) if is_contention(&e) => {
                Err(
                    VfsError::new(VfsErrorKind::Locked, "lock_exclusive", e.to_string())
                        .with_path(path),
                )
            }
            Err(e) => Err(err("lock_exclusive", path, &e)),
        }
    }

    fn is_os_filesystem(&self) -> bool {
        true
    }

    fn open_output(&self, path: &Path) -> VfsResult<fs::File> {
        // `std`'s default share mode on Windows already admits concurrent readers
        // and Unix imposes none, so a tailing reader can open the file meanwhile.
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|e| err("open_output", path, &e))
    }
}
