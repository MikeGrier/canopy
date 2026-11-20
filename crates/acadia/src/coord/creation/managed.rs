// Copyright (c) Michael Grier.
use super::*;
use crate::{FileType, Vfs, names::ComponentDirectory};
use std::collections::BTreeSet;

pub(crate) trait ManagedFence {
    fn alive(&self) -> CoordResult<()>;

    fn fenced<T>(&self, operation: impl FnOnce() -> crate::VfsResult<T>) -> CoordResult<T>;
}

/// A maker for the exact UTF-8 namespace. Dispatch on `logical` and use `target` for I/O.
/// The parent resolver is locked during callbacks; do not recursively enter it.
pub trait ManagedArtifactMaker {
    /// Creates the artifact at an absent attempt-private `target`, and flushes it and every
    /// descendant before returning. The target is never the final child path; coordination
    /// places it only after the participant is proven live.
    fn make(&mut self, logical: &str, target: &Path, inputs: &[u8]) -> CoordResult<()>;

    /// Supplies marker bytes. Only the coordination publishes the marker at its resolved path.
    fn marker(&mut self, logical: &str, inputs: &[u8]) -> CoordResult<Vec<u8>>;
}

fn discard_attempt(
    fs: &dyn Vfs,
    root: &Path,
    attempt: &crate::names::ArtifactAttempt,
) -> CoordResult<()> {
    match fs.remove_dir_all(&attempt.directory) {
        Ok(()) => {}
        Err(error) if error.kind() == crate::VfsErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(fs.fsync(root)?)
}

pub(crate) struct ManagedPlan<'a> {
    fs: &'a dyn Vfs,
    root: std::path::PathBuf,
    marker: &'a str,
    others: &'a [&'a str],
    inputs: &'a [u8],
    names: BTreeSet<&'a str>,
}

impl<'a> ManagedPlan<'a> {
    pub(crate) fn new(plan: &CreationPlan<'a>, fs: &'a dyn Vfs, root: &Path) -> CoordResult<Self> {
        let (marker, others) = plan.artifacts.split_last().ok_or(CoordError::InvalidPlan {
            why: "a creation makes at least its marker",
        })?;
        let names = plan.artifacts.iter().copied().collect::<BTreeSet<_>>();
        if names.len() != plan.artifacts.len() {
            return Err(CoordError::InvalidPlan {
                why: "an artifact is named twice",
            });
        }
        let root = crate::names::ComponentDirectory::stable_parent(fs, root)?;
        Ok(Self {
            marker,
            others,
            inputs: plan.inputs,
            names,
            fs,
            root,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn preflight(&self, reservation: Option<bool>) -> CoordResult<()> {
        if reservation == Some(false) {
            return Err(CoordError::Reserved);
        }
        let (registered, owned) = crate::names::validate_control_entries(self.fs, &self.root)?;
        if reservation == Some(true) && !owned {
            return Err(CoordError::Occupied {
                name: ".names.owner".to_owned(),
            });
        }
        let entries = self.fs.read_dir(&self.root)?;
        let has_map = crate::names::has_map(&entries);
        if has_map && reservation != Some(true) {
            return Err(CoordError::Occupied {
                name: ".names.csv".to_owned(),
            });
        }
        if let Some(stray) = entries.iter().find(|entry| {
            let name = entry.file_name.as_str();
            if name.starts_with(RESERVED_PREFIX)
                || (crate::names::is_control(name) && entry.file_type == FileType::File)
                || (registered.contains_key(name) && entry.file_type != FileType::Other)
            {
                return false;
            }
            if reservation == Some(true) && crate::names::is_map_publication_staging(entry) {
                return false;
            }
            if reservation == Some(true)
                && registered.keys().any(|physical| {
                    crate::names::is_artifact_staging(entry, physical)
                        || (crate::is_publication_staging_of(name, physical)
                            && entry.file_type == FileType::File)
                })
            {
                return false;
            }
            true
        }) {
            return Err(CoordError::Occupied {
                name: stray.file_name.clone(),
            });
        }
        Ok(())
    }

    pub(crate) fn run_held<F: ManagedFence>(
        &self,
        dir: &mut ComponentDirectory<'_>,
        fence: &F,
        maker: &mut dyn ManagedArtifactMaker,
        reservation: Option<bool>,
        settle: impl FnOnce() -> CoordResult<Created>,
        publish: impl Fn(&Path, &[u8]) -> CoordResult<()>,
    ) -> CoordResult<Created> {
        let (fs, root) = (self.fs, self.root.as_path());
        fence.alive()?;
        if let Some(path) = fence.fenced(|| dir.lookup(self.marker))? {
            if !fence.fenced(|| fs.metadata(&path))?.is_file() {
                return Err(CoordError::Occupied {
                    name: self.marker.to_owned(),
                });
            }
            if reservation != Some(true) {
                return Err(CoordError::AlreadyCreated);
            }
            fence.fenced(|| fs.fsync(&path))?;
            fence.fenced(|| fs.fsync(root))?;
            return Ok(Created::Existing);
        }
        if reservation == Some(false) {
            return Err(CoordError::Reserved);
        }
        let registered = fence.fenced(|| dir.registered())?;
        for logical in registered.values() {
            if reservation.is_none() || !self.names.contains(logical.as_str()) {
                return Err(CoordError::Occupied {
                    name: logical.clone(),
                });
            }
            fence.fenced(|| dir.lookup(logical))?;
        }
        for entry in fence.fenced(|| fs.read_dir(root))? {
            let name = &entry.file_name;
            if name.starts_with(RESERVED_PREFIX)
                || registered.contains_key(name)
                || (reservation == Some(true) && crate::names::is_map_publication_staging(&entry))
                || (crate::names::is_control(name) && entry.file_type == FileType::File)
                || registered.keys().any(|target| {
                    crate::names::is_artifact_staging(&entry, target)
                        || (crate::is_publication_staging_of(name, target)
                            && entry.file_type == FileType::File)
                })
            {
                continue;
            }
            return Err(CoordError::Occupied {
                name: entry.file_name,
            });
        }
        if reservation.is_none() {
            fence.fenced(|| dir.claim_ownership())?;
        }
        let created = settle()?;
        for logical in self.others {
            fence.alive()?;
            let target = fence.fenced(|| dir.reserve(logical))?;
            if fence.fenced(|| dir.lookup(logical))?.is_none() {
                let attempt =
                    fence.fenced(|| crate::names::claim_artifact_attempt(fs, root, &target))?;
                fence.alive()?;
                if let Err(error) = maker.make(logical, &attempt.artifact, self.inputs) {
                    discard_attempt(fs, root, &attempt)?;
                    return Err(error);
                }
                fence.alive()?;
                match fence.fenced(|| attempt.entry_type(fs))? {
                    Some(FileType::File | FileType::Dir) => {
                        fence.fenced(|| fs.fsync(&attempt.directory))?;
                        fence.fenced(|| fs.rename(&attempt.artifact, &target))?;
                        fence.fenced(|| fs.fsync(&attempt.directory))?;
                        fence.fenced(|| fs.fsync(root))?;
                        fence.fenced(|| fs.remove_dir_all(&attempt.directory))?;
                    }
                    Some(FileType::Other) => {
                        discard_attempt(fs, root, &attempt)?;
                        return Err(CoordError::Malformed {
                            what: "managed artifact staging kind",
                        });
                    }
                    None => {
                        discard_attempt(fs, root, &attempt)?;
                        return Err(CoordError::Malformed {
                            what: "missing managed artifact staging",
                        });
                    }
                }
            } else {
                fence.fenced(|| fs.fsync(&target))?;
            }
            fence.fenced(|| fs.fsync(root))?;
        }
        fence.alive()?;
        let target = fence.fenced(|| dir.reserve(self.marker))?;
        fence.alive()?;
        let bytes = maker.marker(self.marker, self.inputs)?;
        publish(&target, &bytes)?;
        fence.alive()?;
        fence.fenced(|| fs.fsync(root))?;
        Ok(created)
    }
}

pub(crate) fn confirm_managed_held<F: ManagedFence>(
    fs: &dyn Vfs,
    root: &Path,
    logical: &str,
    dir: &ComponentDirectory<'_>,
    fence: &F,
) -> CoordResult<()> {
    let path = fence
        .fenced(|| dir.lookup(logical))?
        .ok_or(CoordError::NotCreated)?;
    if !fence.fenced(|| fs.metadata(&path))?.is_file() {
        return Err(CoordError::NotCreated);
    }
    fence.fenced(|| fs.fsync(&path))?;
    fence.fenced(|| fs.fsync(root))?;
    Ok(())
}
