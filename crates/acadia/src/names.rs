// Copyright (c) Michael Grier.
//! Exact UTF-8 logical component names above the native VFS namespace.

use crate::{
    DirEntry, FileType, LockMode, LockSpan, Vfs, VfsError, VfsErrorKind, VfsLock, VfsResult,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const DIGEST_PREFIX: &str = ".acadia-h1-";
const DIRECT_LIMIT: usize = 212;
const MAP: &str = ".names.csv";
const LOCK: &str = ".names.lock";
const STAGE: &str = ".names.stage";
const OWNER: &str = ".names.owner";
const OWNER_BYTES: &[u8] = b"acadia_names_owner_v1\n";
const ARTIFACT_ATTEMPT_PREFIX: &str = ".names-attempt-";
const ARTIFACT_LEAF: &str = "artifact";
const HEADER: &str = "acadia_names_v1,logical_utf8_hex\n";
pub(crate) type Bindings = BTreeMap<String, String>;

fn invalid(message: &'static str) -> VfsError {
    VfsError::new(VfsErrorKind::InvalidInput, "component_names", message)
}

fn decode(bytes: &[u8]) -> VfsResult<Bindings> {
    let rows = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.strip_prefix(HEADER))
        .ok_or_else(|| invalid("invalid name map header or encoding"))?;
    let mut bindings = Bindings::new();
    for record in rows.split_inclusive('\n') {
        let (physical, hex) = record
            .strip_suffix('\n')
            .and_then(|row| row.split_once(','))
            .ok_or_else(|| invalid("invalid name map record"))?;
        if !hex.len().is_multiple_of(2)
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(invalid("noncanonical logical name encoding"));
        }
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|pos| u8::from_str_radix(&hex[pos..pos + 2], 16).expect("validated hexadecimal"))
            .collect();
        let logical = String::from_utf8(bytes).map_err(|_| invalid("invalid logical UTF-8"))?;
        if spelling(&logical) != physical
            || bindings
                .last_key_value()
                .is_some_and(|(last, _)| last.as_str() >= physical)
        {
            return Err(invalid("mismatched, duplicate or unsorted physical name"));
        }
        bindings.insert(physical.to_owned(), logical);
    }
    Ok(bindings)
}

fn encode(bindings: &Bindings) -> String {
    let mut csv = String::from(HEADER);
    for (physical, logical) in bindings {
        csv.push_str(physical);
        csv.push(',');
        append_hex(&mut csv, logical.as_bytes());
        csv.push('\n');
    }
    csv
}

/// Exclusive access to the extended-name namespace of one existing parent directory.
/// Keep the guard through operations on resolved paths. Do not lock the same parent
/// recursively, or modify its namespace outside this protocol while it is managed.
pub struct ComponentDirectory<'fs> {
    fs: &'fs dyn Vfs,
    parent: PathBuf,
    _lock: Option<Box<dyn VfsLock>>,
}

type Entries = BTreeMap<String, Option<DirEntry>>;

fn snapshot(fs: &dyn Vfs, parent: &Path) -> VfsResult<Entries> {
    let mut entries = Entries::new();
    for child in fs.read_dir(parent)? {
        entries
            .entry(child.file_name.to_ascii_lowercase())
            .and_modify(|group| *group = None)
            .or_insert(Some(child));
    }
    Ok(entries)
}

fn snapshot_entry(parent: &Path, entries: &Entries, name: &str) -> VfsResult<Option<FileType>> {
    match entries.get(name) {
        None => Ok(None),
        Some(Some(child)) if child.file_name == name => Ok(Some(child.file_type)),
        Some(_) => Err(invalid("noncanonical case alias").with_path(parent.join(name))),
    }
}

fn entry(
    fs: &dyn Vfs,
    parent: &Path,
    entries: &Entries,
    name: &str,
) -> VfsResult<Option<FileType>> {
    if let Some(group) = entries.get(name) {
        return match group {
            Some(child) if child.file_name == name => Ok(Some(child.file_type)),
            _ => Err(invalid("noncanonical case alias").with_path(parent.join(name))),
        };
    }
    if fs.exists(&parent.join(name))? {
        return Err(
            invalid("native alias without an exact directory entry").with_path(parent.join(name))
        );
    }
    Ok(None)
}

impl<'fs> ComponentDirectory<'fs> {
    pub(crate) fn stable_parent(fs: &dyn Vfs, parent: &Path) -> VfsResult<PathBuf> {
        if fs.is_os_filesystem() && parent.is_relative() {
            std::path::absolute(parent).map_err(|error| {
                VfsError::new(
                    VfsErrorKind::InvalidInput,
                    "absolute_path",
                    error.to_string(),
                )
                .with_path(parent)
            })
        } else {
            Ok(parent.to_owned())
        }
    }

    /// Locks the parent's permanent metadata lock and validates the authoritative CSV.
    pub fn open(fs: &'fs dyn Vfs, parent: &Path) -> VfsResult<Self> {
        Self::open_with_lock(fs, parent, true)
    }

    pub(crate) fn open_held(fs: &'fs dyn Vfs, parent: &Path) -> VfsResult<Self> {
        Self::open_with_lock(fs, parent, false)
    }

    fn open_with_lock(fs: &'fs dyn Vfs, parent: &Path, acquire: bool) -> VfsResult<Self> {
        let parent = Self::stable_parent(fs, parent)?;
        validate_control_entries(fs, &parent)?;
        let lock_path = lock_path(&parent);
        let initial = fs.read_dir(&parent)?;
        if initial.iter().any(|child| {
            child.file_name.eq_ignore_ascii_case(LOCK)
                && (child.file_name != LOCK || child.file_type != FileType::File)
        }) {
            return Err(invalid("invalid resolver lock entry").with_path(&lock_path));
        }
        let lock = if acquire {
            Some(fs.lock(&lock_path, LockMode::Exclusive, LockSpan::File)?)
        } else {
            None
        };
        let directory = Self {
            fs,
            parent: parent.to_owned(),
            _lock: lock,
        };
        validate_control_entries(fs, &parent)?;
        let entries = snapshot(fs, &parent)?;
        if entry(fs, &parent, &entries, LOCK)? != Some(FileType::File) {
            return Err(invalid("resolver lock is not a regular file").with_path(lock_path));
        }
        directory.bindings(&entries)?;
        Ok(directory)
    }

    pub(crate) fn claim_ownership(&self) -> VfsResult<()> {
        let entries = snapshot(self.fs, &self.parent)?;
        match entry(self.fs, &self.parent, &entries, OWNER)? {
            None => {
                self.fs.publish_new(&self.parent.join(OWNER), OWNER_BYTES)?;
                self.fs.fsync(&self.parent)
            }
            Some(FileType::File) if self.fs.read(&self.parent.join(OWNER))? == OWNER_BYTES => {
                self.fs.fsync(&self.parent)
            }
            Some(_) => {
                Err(invalid("invalid managed ownership record").with_path(self.parent.join(OWNER)))
            }
        }
    }

    fn bindings(&self, entries: &Entries) -> VfsResult<Bindings> {
        let path = self.parent.join(MAP);
        match entry(self.fs, &self.parent, entries, MAP)? {
            None => Ok(Bindings::new()),
            Some(FileType::File) => {
                decode(&self.fs.read(&path)?).map_err(|error| error.with_path(path))
            }
            Some(_) => Err(invalid("name map is not a regular file").with_path(path)),
        }
    }

    fn save(&self, entries: &Entries, bindings: &Bindings) -> VfsResult<()> {
        self.claim_ownership()?;
        let map = self.parent.join(MAP);
        let stage = self.parent.join(STAGE);
        match entry(self.fs, &self.parent, entries, MAP)? {
            None => {
                if entry(self.fs, &self.parent, entries, STAGE)?.is_some() {
                    return Err(invalid("unowned name map stage").with_path(stage));
                }
                self.fs.publish_new(&map, HEADER.as_bytes())?;
                self.fs.fsync(&self.parent)?;
            }
            Some(FileType::File) => {
                self.fs.fsync(&self.parent)?;
                match entry(self.fs, &self.parent, entries, STAGE)? {
                    Some(FileType::File) => self.fs.remove_file(&stage)?,
                    None => (),
                    Some(_) => {
                        return Err(invalid("invalid name map stage").with_path(stage));
                    }
                }
            }
            Some(_) => return Err(invalid("name map is not a regular file").with_path(map)),
        }
        self.fs.write(&stage, encode(bindings).as_bytes())?;
        self.fs.fsync(&stage)?;
        self.fs.rename(&stage, &map)?;
        self.fs.fsync(&self.parent)
    }

    fn child(
        &self,
        entries: &Entries,
        bindings: &Bindings,
        physical: &str,
        logical: &str,
    ) -> VfsResult<Option<FileType>> {
        let owner = bindings.get(physical);
        let kind = entry(self.fs, &self.parent, entries, physical)?;
        if owner.is_some_and(|name| name != logical)
            || (kind.is_some() && owner.is_none())
            || kind == Some(FileType::Other)
        {
            return Err(invalid("unowned, colliding or unsupported child")
                .with_path(self.parent.join(physical)));
        }
        Ok(kind)
    }

    /// Durably claims the spelling before returning the physical path for child publication.
    /// Repeating the operation resumes the same binding, even after an uncertain directory flush.
    pub fn reserve(&mut self, logical: &str) -> VfsResult<PathBuf> {
        let entries = snapshot(self.fs, &self.parent)?;
        let mut bindings = self.bindings(&entries)?;
        let physical = spelling(logical);
        self.child(&entries, &bindings, &physical, logical)?;
        if bindings.contains_key(&physical) {
            self.fs.fsync(&self.parent)?;
        } else {
            bindings.insert(physical.clone(), logical.to_owned());
            self.save(&entries, &bindings)?;
        }
        Ok(self.parent.join(physical))
    }

    /// Resolves an existing, owned child. An unpublished reservation is absent.
    pub fn lookup(&self, logical: &str) -> VfsResult<Option<PathBuf>> {
        let entries = snapshot(self.fs, &self.parent)?;
        let bindings = self.bindings(&entries)?;
        let physical = spelling(logical);
        Ok(self
            .child(&entries, &bindings, &physical, logical)?
            .map(|_| self.parent.join(physical)))
    }

    pub(crate) fn registered(&self) -> VfsResult<BTreeMap<String, String>> {
        self.bindings(&snapshot(self.fs, &self.parent)?)
    }

    /// Lists only published, owned children, using their original logical names.
    pub fn list(&self) -> VfsResult<Vec<DirEntry>> {
        let entries = snapshot(self.fs, &self.parent)?;
        let bindings = self.bindings(&entries)?;
        let mut visible = Vec::new();
        for (physical, logical) in &bindings {
            if let Some(file_type) = self.child(&entries, &bindings, physical, logical)? {
                visible.push(DirEntry {
                    file_name: logical.clone(),
                    file_type,
                });
            }
        }
        Ok(visible)
    }

    /// Removes a file, empty directory or unpublished reservation. Returns false if unbound.
    pub fn remove(&mut self, logical: &str) -> VfsResult<bool> {
        let entries = snapshot(self.fs, &self.parent)?;
        let mut bindings = self.bindings(&entries)?;
        let physical = spelling(logical);
        let kind = self.child(&entries, &bindings, &physical, logical)?;
        if bindings.remove(&physical).is_none() {
            self.fs.fsync(&self.parent)?;
            return Ok(false);
        }
        let path = self.parent.join(physical);
        match kind {
            Some(FileType::File) => self.fs.remove_file(&path)?,
            Some(FileType::Dir) => self.fs.remove_dir(&path)?,
            Some(FileType::Other) => return Err(invalid("unsupported child").with_path(path)),
            None => (),
        }
        self.fs.fsync(&self.parent)?;
        self.save(&entries, &bindings)?;
        Ok(true)
    }
}

fn append_hex(output: &mut String, bytes: &[u8]) {
    for byte in bytes {
        write!(output, "{byte:02x}").expect("writing to String is infallible");
    }
}

fn spelling(logical: &str) -> String {
    let device = matches!(logical, "con" | "prn" | "aux" | "nul")
        || matches!(
            logical.as_bytes(),
            [b'c', b'o', b'm', b'0'..=b'9'] | [b'l', b'p', b't', b'0'..=b'9']
        );
    if logical == ".store"
        || (!device
            && (1..=DIRECT_LIMIT).contains(&logical.len())
            && logical.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            }))
    {
        return logical.to_owned();
    }
    let mut physical = DIGEST_PREFIX.to_owned();
    append_hex(&mut physical, &Sha256::digest(logical.as_bytes()));
    physical
}

pub(crate) fn is_control(name: &str) -> bool {
    matches!(name, LOCK | MAP | STAGE | OWNER)
}

pub(crate) fn has_map(entries: &[DirEntry]) -> bool {
    entries.iter().any(|entry| entry.file_name == MAP)
}

pub(crate) fn validate_control_entries(fs: &dyn Vfs, parent: &Path) -> VfsResult<(Bindings, bool)> {
    let entries = snapshot(fs, parent)?;
    let owned = match snapshot_entry(parent, &entries, OWNER)? {
        None => false,
        Some(FileType::File) => {
            let path = parent.join(OWNER);
            if fs.read(&path)? != OWNER_BYTES {
                return Err(invalid("invalid name ownership record").with_path(path));
            }
            true
        }
        Some(_) => {
            return Err(invalid("name owner is not a regular file").with_path(parent.join(OWNER)));
        }
    };
    let bindings = match snapshot_entry(parent, &entries, MAP)? {
        None => Bindings::new(),
        Some(FileType::File) => {
            let path = parent.join(MAP);
            decode(&fs.read(&path)?).map_err(|error| error.with_path(&path))?
        }
        Some(_) => {
            return Err(invalid("name map is not a regular file").with_path(parent.join(MAP)));
        }
    };
    if !owned
        && (entries
            .get(MAP)
            .is_some_and(|entry| entry.as_ref().is_some_and(|item| item.file_name == MAP))
            || snapshot_entry(parent, &entries, STAGE)?.is_some())
    {
        return Err(invalid("unowned name map or stage").with_path(parent.join(MAP)));
    }
    Ok((bindings, owned))
}

pub(crate) fn is_map_publication_staging(entry: &DirEntry) -> bool {
    entry.file_type == FileType::File && crate::is_publication_staging_of(&entry.file_name, MAP)
}

pub(crate) struct ArtifactAttempt {
    pub(crate) directory: PathBuf,
    pub(crate) artifact: PathBuf,
}

fn artifact_attempt_prefix(physical: &str) -> String {
    let mut prefix = ARTIFACT_ATTEMPT_PREFIX.to_owned();
    append_hex(&mut prefix, &Sha256::digest(physical.as_bytes()));
    prefix.push('-');
    prefix
}

fn claim_artifact_attempt_at(
    fs: &dyn Vfs,
    parent: &Path,
    target: &Path,
    sequence: u64,
) -> VfsResult<Option<ArtifactAttempt>> {
    let physical = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("managed target is not a UTF-8 component").with_path(target))?;
    let directory = parent.join(format!("{}{}", artifact_attempt_prefix(physical), sequence));
    match fs.create_dir(&directory) {
        Ok(()) => Ok(Some(ArtifactAttempt {
            artifact: directory.join(ARTIFACT_LEAF),
            directory,
        })),
        Err(error) if error.kind() == VfsErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error),
    }
}

impl ArtifactAttempt {
    pub(crate) fn entry_type(&self, fs: &dyn Vfs) -> VfsResult<Option<FileType>> {
        Ok(fs
            .read_dir(&self.directory)?
            .into_iter()
            .find(|entry| entry.file_name == ARTIFACT_LEAF)
            .map(|entry| entry.file_type))
    }
}

pub(crate) fn claim_artifact_attempt(
    fs: &dyn Vfs,
    parent: &Path,
    target: &Path,
) -> VfsResult<ArtifactAttempt> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let sequence = NEXT
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| invalid("artifact attempt identity exhausted").with_path(target))?;
        if let Some(attempt) = claim_artifact_attempt_at(fs, parent, target, sequence)? {
            return Ok(attempt);
        }
    }
}

pub(crate) fn is_artifact_staging(entry: &DirEntry, physical: &str) -> bool {
    let suffix = entry
        .file_name
        .strip_prefix(&artifact_attempt_prefix(physical));
    entry.file_type == FileType::Dir
        && suffix.is_some_and(|sequence| {
            !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
        })
}

pub(crate) fn lock_path(parent: &Path) -> PathBuf {
    parent.join(LOCK)
}

#[cfg(test)]
mod tests;
