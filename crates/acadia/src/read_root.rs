// Copyright (c) Michael Grier.

//! Directory-handle-anchored read access for untrusted artifact paths (D300).

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};

/// An already anchored directory. Implementations reject every link/reparse
/// component and return a stream bound to the opened regular file, never a path
/// to reopen. Concurrent content writes can still change a stream; consumers
/// must hash and retain the exact bytes they read.
pub trait ReadRoot: Send + Sync {
    fn open_file(&self, relative: &Path) -> io::Result<Box<dyn Read + Send>>;
}

/// OS directory capability. Each component is opened relative to its parent's
/// handle with no-follow semantics; metadata is inspected on that same handle.
pub struct OsReadRoot {
    directory: Dir,
}

impl OsReadRoot {
    /// Anchors an absolute directory, rejecting links in every component below
    /// the filesystem root. Relative paths and parent traversal are refused.
    pub fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid("read root must be absolute"));
        }
        let mut anchor = PathBuf::new();
        let mut names = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => anchor.push(component.as_os_str()),
                Component::Normal(name) => names.push(name),
                _ => return Err(invalid("read root contains traversal")),
            }
        }
        let mut directory = Dir::open_ambient_dir(anchor, cap_std::ambient_authority())?;
        reject_reparse(&directory.dir_metadata()?)?;
        for name in names {
            directory = directory.open_dir_nofollow(name)?;
            reject_reparse(&directory.dir_metadata()?)?;
        }
        Ok(Self { directory })
    }
}

impl ReadRoot for OsReadRoot {
    fn open_file(&self, relative: &Path) -> io::Result<Box<dyn Read + Send>> {
        let mut components = relative.components().peekable();
        let mut directory = self.directory.try_clone()?;
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                return Err(invalid("non-relative read path"));
            };
            if components.peek().is_some() {
                directory = directory.open_dir_nofollow(name)?;
                reject_reparse(&directory.dir_metadata()?)?;
            } else {
                let mut options = OpenOptions::new();
                options.read(true).follow(FollowSymlinks::No);
                #[cfg(unix)]
                {
                    use cap_std::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NONBLOCK);
                }
                let file = directory.open_with(name, &options)?;
                let metadata = file.metadata()?;
                reject_reparse(&metadata)?;
                if !metadata.is_file() {
                    return Err(invalid("read target is not a regular file"));
                }
                return Ok(Box::new(file));
            }
        }
        Err(invalid("empty read path"))
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn reject_reparse(metadata: &cap_std::fs::Metadata) -> io::Result<()> {
    if metadata.file_type().is_symlink() {
        return Err(invalid("link traversal refused"));
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid("reparse traversal refused"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
