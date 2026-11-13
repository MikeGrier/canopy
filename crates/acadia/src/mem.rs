// Copyright (c) Michael Grier.

//! The in-memory mock [`Vfs`] backend (VFS-3, D55).
//!
//! [`MemFs`] is a complete filesystem held in memory: a tree of directories and
//! named files that lets store logic run disk-free and deterministically. It
//! satisfies the same [`Vfs`] contract as the real backend ([`OsFs`](crate::os::OsFs)),
//! byte-for-byte, so a store exercised against it behaves as it would on disk.
//!
//! # Durability is modeled, not faked
//!
//! The mock's reason to exist beyond a plain hash map is that it models the one
//! property the store's crash-safety depends on: **the split between what has
//! been written and what is durable** (D55). It keeps two states:
//!
//! - a **live** tree -- what reads observe, updated by every mutation; and
//! - a **durable** tree -- what would survive a power loss, advanced only by
//!   [`fsync`](Vfs::fsync).
//!
//! [`fsync`](Vfs::fsync) of a *file* makes that file's current bytes durable.
//! [`fsync`](Vfs::fsync) of a *directory* makes that directory's set of entries
//! (the names created, removed, or renamed within it) durable, but not the
//! contents of the files those names point at -- exactly the real
//! data-vs-metadata distinction. Nor does it make the directory's *own* name
//! durable: that takes an fsync of its parent, so a directory created under a
//! parent that is never fsync'd is lost on a crash with everything in it. (The
//! implicit root cannot be fsync'd; its direct entries count as named.) [`MemFs::simulate_crash`] discards everything
//! not yet durable, reverting live to durable, so tests can assert what a
//! recovering store would find after a crash at any point.
//!
//! Files carry a stable inode identity, so a [`rename`](Vfs::rename) moves the
//! name while the durable bytes stay attached to the data: the store's
//! stage-file / `fsync` / rename / `fsync`-dir protocol therefore recovers
//! correctly in the model, and a crash *between* those steps loses exactly what
//! it would lose on a real disk.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{
    DirEntry, FileType, LockMode, LockSpan, Metadata, Vfs, VfsError, VfsErrorKind, VfsLock,
    VfsResult,
};

mod locks;

use locks::{Access, LockShared};

/// A node in the filesystem tree: a file (by inode id) or a directory.
#[derive(Debug, Clone)]
enum Node {
    /// A regular file; the `u64` is its inode id, keying its bytes in the
    /// live/durable data maps.
    File(u64),
    /// A directory: its entries, ordered by name for deterministic listing.
    Dir(BTreeMap<OsString, Node>),
}

/// The two-state store behind a [`MemFs`].
#[derive(Debug, Default, Clone)]
struct Inner {
    /// The visible tree: what reads observe.
    live: BTreeMap<OsString, Node>,
    /// The persisted tree: what a crash would leave behind.
    durable: BTreeMap<OsString, Node>,
    /// Live file contents by inode id.
    live_data: HashMap<u64, Vec<u8>>,
    /// Durable (fsync'd) file contents by inode id.
    durable_data: HashMap<u64, Vec<u8>>,
    /// Directories present in `durable` only because their own contents were
    /// fsync'd: their *name* in the parent has not been made durable yet, so a
    /// crash loses them. A parent's fsync clears the mark.
    unnamed: HashSet<Vec<OsString>>,
    /// Live directories renamed since their new name was last made durable, mapped to the
    /// durable contents they carried away from their old name, captured at the rename.
    /// Directories are tracked by path, not identity, so this is what lets a renamed
    /// directory keep its durable contents whatever order its old and new parents are synced
    /// in, as the inode would on a real filesystem.
    carried: HashMap<Vec<OsString>, Node>,
    /// Paths whose live directory is no longer the one the durable tree holds there -- it was
    /// removed or renamed away, and anything now at the path is a new directory. The next sync
    /// that names the path starts it from empty rather than from the old durable contents.
    replaced: HashSet<Vec<OsString>>,
    /// How many places name each inode: entries in the live tree, the durable tree, and the
    /// carried snapshots. A file's durable bytes are dropped when its count reaches zero, so
    /// reclaiming the data an operation made unreachable costs that operation only what it
    /// discarded, never a walk of the whole filesystem.
    refs: HashMap<u64, usize>,
    /// How many live-tree entries name each inode. A crash can leave two names for one file
    /// (a rename made durable in its destination but not its source), so live bytes go only
    /// with the last live name.
    live_refs: HashMap<u64, usize>,
    /// The names in each directory whose live entry may differ from its durable one, so a
    /// directory sync costs what changed rather than the size of the directory. `None` means
    /// the durable directory was created, reset or replaced wholesale and every entry must be
    /// reconciled; a directory with no record has no changes. Recording a name that did not
    /// change only costs a no-op reconcile, while missing one would lose it on a crash -- so
    /// every operation that changes a live directory's entries records here.
    dirty: HashMap<Vec<OsString>, Option<HashSet<OsString>>>,
    /// Reconcile every entry on every directory sync, as an oracle for the change tracking.
    #[cfg(test)]
    full_directory_sync: bool,
    /// How many entries directory syncs have examined, so a test can see what a sync cost.
    #[cfg(test)]
    examined_entries: usize,
    /// Next inode id to hand out.
    next_inode: u64,
}

/// An in-memory [`Vfs`] that models write-vs-durable ordering.
///
/// See the [module documentation](self) for the durability model. Cheap to
/// share as `Arc<dyn Vfs>`; all state is behind a single lock, so operations are
/// serialized and deterministic.
///
/// A `MemFs` value is one simulated process's view of its filesystem:
/// [`another_process`](MemFs::another_process) gives a second process over the same tree, and
/// [`abandon`](MemFs::abandon) is a process's death.
#[derive(Debug, Default)]
pub struct MemFs {
    inner: Arc<Mutex<Inner>>,
    /// The locks held in this filesystem, by every process over it.
    ///
    /// Held outside [`Inner`] deliberately: a lock is a property of its *holder*,
    /// not of the stored tree, so [`simulate_crash`](MemFs::simulate_crash) --
    /// which models losing unfsync'd storage, not losing the process -- must not
    /// disturb it. `Arc` so a guard can release without borrowing the filesystem.
    locks: Arc<LockShared>,
    /// The simulated process this view belongs to, whose locks [`abandon`](MemFs::abandon)
    /// releases.
    process: u64,
}

impl MemFs {
    /// Creates an empty in-memory filesystem.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A filesystem whose directory syncs reconcile every entry, ignoring the change tracking.
    #[cfg(test)]
    pub(crate) fn with_full_directory_sync() -> Self {
        let fs = Self::default();
        fs.tree().full_directory_sync = true;
        fs
    }

    /// How many entries directory syncs have examined so far.
    #[cfg(test)]
    pub(crate) fn examined_entries(&self) -> usize {
        self.tree().examined_entries
    }

    /// An independent filesystem starting from this one's whole state, what is durable and
    /// what is not yet alike, so an expensive fixture can be built once and copied per test.
    ///
    /// Locks are not copied: a lock belongs to its holder, not to the stored tree.
    #[must_use]
    pub fn fork(&self) -> Self {
        Self {
            inner: Arc::new(Mutex::new(self.tree().clone())),
            locks: Arc::default(),
            process: 0,
        }
    }

    /// Discards every change that has not been made durable by an
    /// [`fsync`](Vfs::fsync), reverting the live tree to the durable tree.
    ///
    /// This is the crash / power-loss simulation the recovery-stress harness
    /// drives: after it returns, reads observe exactly what a store recovering
    /// from a crash at this instant would find -- fsync'd names, with each
    /// surviving file's last fsync'd bytes (or empty, if its data was never
    /// fsync'd).
    pub fn simulate_crash(&self) {
        let mut guard = self.tree();
        let inner = &mut *guard;
        let mut unnamed: Vec<Vec<OsString>> = inner.unnamed.drain().collect();
        unnamed.sort();
        for parts in unnamed {
            if let Some((name, parents)) = parts.split_last()
                && let Ok(dir) = dir_of_mut(&mut inner.durable, parents, "crash", Path::new(""))
            {
                dir.remove(name);
            }
        }
        inner.carried.clear();
        inner.replaced.clear();
        inner.dirty.clear();
        inner.live = inner.durable.clone();
        let mut ids = Vec::new();
        collect_ids(&inner.live, &mut ids);
        let mut rebuilt = HashMap::with_capacity(ids.len());
        for id in ids {
            let bytes = inner.durable_data.get(&id).cloned().unwrap_or_default();
            rebuilt.insert(id, bytes);
        }
        inner.live_data = rebuilt;
        recount(inner);
    }

    /// How many files' live and durable bytes are held, so a test can see data reclaimed.
    #[cfg(test)]
    pub(crate) fn held_file_data(&self) -> (usize, usize) {
        let guard = self.tree();
        (guard.live_data.len(), guard.durable_data.len())
    }

    /// Recounts every reference from scratch and panics if the incremental counts, or the
    /// data maps they govern, disagree with it.
    #[cfg(test)]
    pub(crate) fn audit_refs(&self) {
        let guard = self.tree();
        let mut expected: HashMap<u64, usize> = HashMap::new();
        let mut live = Vec::new();
        collect_ids(&guard.live, &mut live);
        let mut all = live.clone();
        collect_ids(&guard.durable, &mut all);
        for snapshot in guard.carried.values() {
            walk_ids(snapshot, &mut |id| all.push(id));
        }
        for id in all {
            *expected.entry(id).or_default() += 1;
        }
        assert_eq!(guard.refs, expected, "incremental reference counts drifted");
        let mut live_expected: HashMap<u64, usize> = HashMap::new();
        for id in &live {
            *live_expected.entry(*id).or_default() += 1;
        }
        assert_eq!(guard.live_refs, live_expected, "live name counts drifted");
        let live: HashSet<u64> = live.into_iter().collect();
        let held: HashSet<u64> = guard.live_data.keys().copied().collect();
        assert_eq!(held, live, "live bytes held for exactly the live files");
        for id in guard.durable_data.keys() {
            assert!(
                expected.contains_key(id),
                "durable bytes of unreachable inode {id}"
            );
        }
    }

    /// Locks the inner state, recovering the guard if a prior holder panicked
    /// (poisoning is not treated as a fault to propagate; D56).
    fn tree(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Builds a [`VfsError`] of `kind` for operation `op` on `path`.
fn err(kind: VfsErrorKind, op: &'static str, path: &Path, message: &'static str) -> VfsError {
    VfsError::new(kind, op, message).with_path(path)
}

/// Normalizes `path` into the sequence of named components under the mock's
/// single implicit root, collapsing `.`/root/prefix and applying `..` by
/// popping. An empty result (the root itself) is rejected as invalid input,
/// because every operation names at least one entry.
fn split_path(path: &Path, op: &'static str) -> VfsResult<Vec<OsString>> {
    let mut parts: Vec<OsString> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(s) => parts.push(s.to_os_string()),
        }
    }
    if parts.is_empty() {
        return Err(err(
            VfsErrorKind::InvalidInput,
            op,
            path,
            "empty or root path",
        ));
    }
    Ok(parts)
}

/// Hands out the next inode id, or an error if the 64-bit id space is somehow
/// exhausted (checked so an overflow is a value, never a panic; D56).
fn next_id(next: &mut u64, op: &'static str, path: &Path) -> VfsResult<u64> {
    let id = *next;
    *next = next
        .checked_add(1)
        .ok_or_else(|| err(VfsErrorKind::Other, op, path, "inode id space exhausted"))?;
    Ok(id)
}

/// Resolves the directory reached by walking `parents` from `root` (immutable).
fn dir_of<'a>(
    mut cur: &'a BTreeMap<OsString, Node>,
    parents: &[OsString],
    op: &'static str,
    full: &Path,
) -> VfsResult<&'a BTreeMap<OsString, Node>> {
    for p in parents {
        match cur.get(p) {
            Some(Node::Dir(m)) => cur = m,
            Some(Node::File(_)) => {
                return Err(err(
                    VfsErrorKind::NotADirectory,
                    op,
                    full,
                    "path component is not a directory",
                ));
            }
            None => return Err(err(VfsErrorKind::NotFound, op, full, "no such directory")),
        }
    }
    Ok(cur)
}

/// Resolves the directory reached by walking `parents` from `cur` (mutable).
fn dir_of_mut<'a>(
    cur: &'a mut BTreeMap<OsString, Node>,
    parents: &[OsString],
    op: &'static str,
    full: &Path,
) -> VfsResult<&'a mut BTreeMap<OsString, Node>> {
    match parents.split_first() {
        None => Ok(cur),
        Some((head, rest)) => match cur.get_mut(head) {
            Some(Node::Dir(m)) => dir_of_mut(m, rest, op, full),
            Some(Node::File(_)) => Err(err(
                VfsErrorKind::NotADirectory,
                op,
                full,
                "path component is not a directory",
            )),
            None => Err(err(VfsErrorKind::NotFound, op, full, "no such directory")),
        },
    }
}

/// Resolves the node at the full `parts` path (immutable).
fn node_at<'a>(
    root: &'a BTreeMap<OsString, Node>,
    parts: &[OsString],
    op: &'static str,
    full: &Path,
) -> VfsResult<&'a Node> {
    let Some((name, parents)) = parts.split_last() else {
        return Err(err(VfsErrorKind::InvalidInput, op, full, "empty path"));
    };
    let dir = dir_of(root, parents, op, full)?;
    dir.get(name)
        .ok_or_else(|| err(VfsErrorKind::NotFound, op, full, "no such path"))
}

/// Ensures every component of `parts` exists as a directory under `cur`,
/// creating empty directories as needed, and returns the deepest directory.
fn ensure_dir<'a>(
    cur: &'a mut BTreeMap<OsString, Node>,
    parts: &[OsString],
    op: &'static str,
    full: &Path,
) -> VfsResult<&'a mut BTreeMap<OsString, Node>> {
    match parts.split_first() {
        None => Ok(cur),
        Some((head, rest)) => {
            let child = cur
                .entry(head.clone())
                .or_insert_with(|| Node::Dir(BTreeMap::new()));
            match child {
                Node::Dir(m) => ensure_dir(m, rest, op, full),
                Node::File(_) => Err(err(
                    VfsErrorKind::NotADirectory,
                    op,
                    full,
                    "path component is not a directory",
                )),
            }
        }
    }
}

/// Records that the entry `name` in the live directory `parent` changed.
///
/// The implicit root is never synced, so its entries are not tracked.
fn mark(
    dirty: &mut HashMap<Vec<OsString>, Option<HashSet<OsString>>>,
    parent: &[OsString],
    name: &OsString,
) {
    if parent.is_empty() {
        return;
    }
    match dirty.get_mut(parent) {
        Some(Some(names)) => {
            names.insert(name.clone());
        }
        Some(None) => {}
        None => {
            dirty.insert(parent.to_vec(), Some(HashSet::from([name.clone()])));
        }
    }
}

/// Records that the durable directory at `parts` no longer matches its live one except by
/// recorded changes, so its next sync reconciles every entry.
fn mark_all(dirty: &mut HashMap<Vec<OsString>, Option<HashSet<OsString>>>, parts: &[OsString]) {
    if !parts.is_empty() {
        dirty.insert(parts.to_vec(), None);
    }
}

/// Moves `carried`'s snapshot for `parts`, if any, into `durable` at `parts`.
///
/// Run before a sync makes anything at or beneath a renamed directory durable, so what the
/// directory carried is the base that sync builds on rather than being replaced by it.
/// Returns whether a snapshot was moved.
fn materialize(
    durable: &mut BTreeMap<OsString, Node>,
    carried: &mut HashMap<Vec<OsString>, Node>,
    refs: &mut Refs<'_>,
    parts: &[OsString],
    op: &'static str,
    full: &Path,
) -> VfsResult<bool> {
    let Some((name, parents)) = parts.split_last() else {
        return Ok(false);
    };
    if !carried.contains_key(parts) {
        return Ok(false);
    }
    // Only taken once its destination exists, so a failure leaves the snapshot carried.
    let dir = ensure_dir(durable, parents, op, full)?;
    // A move from the snapshots into the durable tree: the snapshot's own counts carry over,
    // and only what it displaces is released.
    if let Some(snapshot) = carried.remove(parts)
        && let Some(displaced) = dir.insert(name.clone(), snapshot)
    {
        refs.release_node(&displaced);
    }
    Ok(true)
}

/// Starts the durable directory at `parts` from empty if the path was [`Inner::replaced`]:
/// the directory there now is a new one, and must not inherit its predecessor's entries.
/// Returns whether the path was replaced.
fn refresh(
    durable: &mut BTreeMap<OsString, Node>,
    replaced: &mut HashSet<Vec<OsString>>,
    refs: &mut Refs<'_>,
    parts: &[OsString],
) -> bool {
    if !replaced.remove(parts) {
        return false;
    }
    if let Some((name, parents)) = parts.split_last()
        && let Ok(dir) = dir_of_mut(durable, parents, "fsync", Path::new(""))
        && let Some(node @ Node::Dir(_)) = dir.get_mut(name)
    {
        let old = std::mem::replace(node, Node::Dir(BTreeMap::new()));
        refs.release_node(&old);
    }
    true
}

/// Refreshes and materializes the durable directory at `parts` before a sync builds on it.
///
/// Either one replaces durable directories wholesale, after which their recorded changes no
/// longer describe how they differ from the live ones, so each is marked for a full sync.
fn renew(
    durable: &mut BTreeMap<OsString, Node>,
    carried: &mut HashMap<Vec<OsString>, Node>,
    replaced: &mut HashSet<Vec<OsString>>,
    dirty: &mut HashMap<Vec<OsString>, Option<HashSet<OsString>>>,
    refs: &mut Refs<'_>,
    parts: &[OsString],
    full: &Path,
) -> VfsResult<()> {
    if refresh(durable, replaced, refs, parts) {
        mark_all(dirty, parts);
    }
    if materialize(durable, carried, refs, parts, "fsync", full)?
        && let Ok(node) = node_at(durable, parts, "fsync", full)
    {
        mark_subtree_all(dirty, &mut parts.to_vec(), node);
    }
    Ok(())
}

/// Marks the directory `node`, at `path`, and every directory beneath it for a full sync.
fn mark_subtree_all(
    dirty: &mut HashMap<Vec<OsString>, Option<HashSet<OsString>>>,
    path: &mut Vec<OsString>,
    node: &Node,
) {
    for_each_dir(path, node, &mut |dir_parts| mark_all(dirty, dir_parts));
}

/// Calls `f` with the path of `node`, at `path`, and of every directory beneath it.
fn for_each_dir(path: &mut Vec<OsString>, node: &Node, f: &mut impl FnMut(&[OsString])) {
    if let Node::Dir(children) = node {
        f(path);
        for (name, child) in children {
            path.push(name.clone());
            for_each_dir(path, child, f);
            path.pop();
        }
    }
}

/// Takes the subtree at `parts` out of the snapshot a renamed ancestor carries, if one does.
///
/// A directory moved out of a renamed directory before either was synced exists durably
/// only inside that ancestor's snapshot. Nothing is extracted if a path between the
/// ancestor and `parts` was replaced since: the directory there now is a new one, and the
/// snapshot's copy belongs to its predecessor.
fn extract_carried(
    carried: &mut HashMap<Vec<OsString>, Node>,
    replaced: &HashSet<Vec<OsString>>,
    parts: &[OsString],
) -> Option<Node> {
    let ancestor = (1..parts.len())
        .rev()
        .map(|depth| &parts[..depth])
        .find(|prefix| carried.contains_key(*prefix))?
        .to_vec();
    if (ancestor.len() + 1..=parts.len()).any(|depth| replaced.contains(&parts[..depth])) {
        return None;
    }
    let Node::Dir(cur) = carried.get_mut(&ancestor)? else {
        return None;
    };
    let mut cur = cur;
    let rest = &parts[ancestor.len()..];
    let (name, between) = rest.split_last()?;
    for component in between {
        match cur.get_mut(component) {
            Some(Node::Dir(next)) => cur = next,
            _ => return None,
        }
    }
    cur.remove(name)
}

/// Collects every inode id reachable in `map` (recursively) into `out`.
fn collect_ids(map: &BTreeMap<OsString, Node>, out: &mut Vec<u64>) {
    for node in map.values() {
        walk_ids(node, &mut |id| out.push(id));
    }
}

/// Calls `f` with every inode id in `node`'s subtree.
fn walk_ids(node: &Node, f: &mut impl FnMut(u64)) {
    #[cfg(test)]
    NODES_WALKED.with(|walked| walked.set(walked.get() + 1));
    match node {
        Node::File(id) => f(*id),
        Node::Dir(map) => {
            for child in map.values() {
                walk_ids(child, f);
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Nodes this thread's operations have walked, so a test can bound an operation's cost.
    static NODES_WALKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Nodes walked on this thread so far (see [`walk_ids`]).
#[cfg(test)]
pub(crate) fn nodes_walked() -> u64 {
    NODES_WALKED.with(std::cell::Cell::get)
}

/// The reference counts and the durable bytes they govern, borrowed together.
struct Refs<'a> {
    counts: &'a mut HashMap<u64, usize>,
    durable_data: &'a mut HashMap<u64, Vec<u8>>,
}

impl Refs<'_> {
    fn retain(&mut self, id: u64) {
        *self.counts.entry(id).or_default() += 1;
    }

    fn retain_node(&mut self, node: &Node) {
        walk_ids(node, &mut |id| self.retain(id));
    }

    /// Drops one reference to `id`, and its durable bytes with the last one.
    fn release(&mut self, id: u64) {
        if let std::collections::hash_map::Entry::Occupied(mut count) = self.counts.entry(id) {
            *count.get_mut() -= 1;
            if *count.get() == 0 {
                count.remove();
                self.durable_data.remove(&id);
            }
        }
    }

    fn release_node(&mut self, node: &Node) {
        walk_ids(node, &mut |id| self.release(id));
    }
}

/// The kind of an existing node, used for [`Vfs::rename`] type checks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
}

/// Advances `ddir` (a durable directory) to have the same direct entries as
/// `live_children`, without recursing into subdirectories or copying file
/// bytes. This is what a directory [`fsync`](Vfs::fsync) persists: names, not
/// contents.
///
/// Every durable entry it drops or replaces is released, which is the only way durable file
/// data becomes unreachable. Returns the names given a new, empty durable directory.
fn reconcile_children(
    ddir: &mut BTreeMap<OsString, Node>,
    live_children: &BTreeMap<OsString, Node>,
    refs: &mut Refs<'_>,
) -> Vec<OsString> {
    let stale: Vec<OsString> = ddir
        .keys()
        .filter(|k| !live_children.contains_key(*k))
        .cloned()
        .collect();
    for k in stale {
        reconcile_entry(ddir, &k, None, refs);
    }
    live_children
        .iter()
        .filter(|(name, node)| reconcile_entry(ddir, name, Some(node), refs))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Advances the durable entry `name` in `ddir` to `live`, the live entry (or its absence).
/// Returns whether the name was given a new, empty durable directory.
fn reconcile_entry(
    ddir: &mut BTreeMap<OsString, Node>,
    name: &OsString,
    live: Option<&Node>,
    refs: &mut Refs<'_>,
) -> bool {
    match live {
        None => {
            if let Some(old) = ddir.remove(name) {
                refs.release_node(&old);
            }
            false
        }
        Some(Node::File(id)) => {
            refs.retain(*id);
            if let Some(old) = ddir.insert(name.clone(), Node::File(*id)) {
                refs.release_node(&old);
            }
            false
        }
        Some(Node::Dir(_)) => {
            if matches!(ddir.get(name), Some(Node::Dir(_))) {
                return false;
            }
            if let Some(old) = ddir.insert(name.clone(), Node::Dir(BTreeMap::new())) {
                refs.release_node(&old);
            }
            true
        }
    }
}

/// The depth at which a sync of the directory at `parts` must be carried rather than made
/// durable in place, if any: the deepest prefix holding a file, in the durable tree or, below
/// the first such file, in the snapshots carried over it.
///
/// Above the first durable file the durable path is all directories, and an ordinary sync
/// materializes carried snapshots into it as it goes.
fn carried_anchor(
    durable: &BTreeMap<OsString, Node>,
    carried: &HashMap<Vec<OsString>, Node>,
    parts: &[OsString],
) -> Option<usize> {
    let mut anchor = None;
    let mut view = Some(durable);
    for k in 1..=parts.len() {
        let prefix = &parts[..k];
        let below = view.and_then(|map| map.get(&parts[k - 1]));
        let over = anchor.and(carried.get(prefix));
        view = match (over, below) {
            (Some(Node::Dir(map)), _) => Some(map),
            (_, Some(Node::File(_))) => {
                anchor = Some(k);
                match carried.get(prefix) {
                    Some(Node::Dir(map)) => Some(map),
                    _ => None,
                }
            }
            (_, Some(Node::Dir(map))) => Some(map),
            _ => None,
        };
    }
    anchor
}

/// Syncs the live directory at `parts` into the carried snapshot of `parts[..depth]`, a new
/// directory standing where a file is still durable (see [`carried_anchor`]).
///
/// Mirrors a durable sync -- replaced directories start empty, carried snapshots below join
/// in, newly created directories are unnamed until their parent syncs, and the target's
/// entries are reconciled -- but against the snapshot, which the anchor's parent's sync later
/// materializes over the file. The snapshot goes back into `carried` whatever happens, so a
/// failure loses nothing it held.
fn sync_into_carried(
    books: &mut Bookkeeping<'_, '_>,
    parts: &[OsString],
    depth: usize,
    live_children: &BTreeMap<OsString, Node>,
    full: &Path,
) -> VfsResult<()> {
    let base = &parts[..depth];
    // A snapshot carried at the anchor is newer than any replacement mark there: a rename
    // sets both at once, and a removal drops the snapshot with the mark.
    books.replaced.remove(base);
    let mut snapshot = books
        .carried
        .remove(base)
        .unwrap_or_else(|| Node::Dir(BTreeMap::new()));
    let result = sync_snapshot(&mut snapshot, books, parts, depth, live_children, full);
    books.carried.insert(base.to_vec(), snapshot);
    result
}

/// The path-keyed state a carried sync updates alongside the snapshot itself.
struct Bookkeeping<'a, 'r> {
    carried: &'a mut HashMap<Vec<OsString>, Node>,
    replaced: &'a mut HashSet<Vec<OsString>>,
    unnamed: &'a mut HashSet<Vec<OsString>>,
    refs: &'a mut Refs<'r>,
}

/// The body of [`sync_into_carried`], against a snapshot the caller puts back.
fn sync_snapshot(
    snapshot: &mut Node,
    books: &mut Bookkeeping<'_, '_>,
    parts: &[OsString],
    depth: usize,
    live_children: &BTreeMap<OsString, Node>,
    full: &Path,
) -> VfsResult<()> {
    let mut below: Vec<Vec<OsString>> = (depth + 1..=parts.len())
        .map(|k| parts[..k].to_vec())
        .collect();
    for (child, node) in live_children {
        if matches!(node, Node::Dir(_)) {
            let mut child_parts = parts.to_vec();
            child_parts.push(child.clone());
            below.push(child_parts);
        }
    }
    for prefix in &below {
        let relative = &prefix[depth..];
        if books.replaced.remove(prefix) {
            reset_within(snapshot, relative, books.refs);
        }
        if books.carried.contains_key(prefix) {
            let Node::Dir(map) = &mut *snapshot else {
                return Ok(());
            };
            let (name, between) = relative.split_last().expect("below the anchor");
            let dir = ensure_dir(map, between, "fsync", full)?;
            if let Some(nested) = books.carried.remove(prefix)
                && let Some(displaced) = dir.insert(name.clone(), nested)
            {
                books.refs.release_node(&displaced);
            }
        }
    }
    let Node::Dir(map) = snapshot else {
        return Ok(());
    };
    // What this sync creates inside the snapshot has no durable name in its parent yet.
    let mut present = Some(&*map);
    for k in depth + 1..=parts.len() {
        present = match present.and_then(|dir| dir.get(&parts[k - 1])) {
            Some(Node::Dir(next)) => Some(next),
            _ => {
                books.unnamed.insert(parts[..k].to_vec());
                None
            }
        };
    }
    let ddir = ensure_dir(map, &parts[depth..], "fsync", full)?;
    reconcile_children(ddir, live_children, books.refs);
    for (child, node) in live_children {
        if matches!(node, Node::Dir(_)) {
            let mut child_parts = parts.to_vec();
            child_parts.push(child.clone());
            books.unnamed.remove(&child_parts);
        }
    }
    books.unnamed.retain(|marked| {
        marked.len() <= parts.len()
            || !marked.starts_with(parts)
            || live_children.contains_key(&marked[parts.len()])
    });
    Ok(())
}

/// Empties the directory at `relative` inside `snapshot`, releasing what it held.
fn reset_within(snapshot: &mut Node, relative: &[OsString], refs: &mut Refs<'_>) {
    let target = match relative.split_last() {
        None => Some(snapshot),
        Some((name, between)) => match snapshot {
            Node::Dir(map) => dir_of_mut(map, between, "fsync", Path::new(""))
                .ok()
                .and_then(|dir| dir.get_mut(name)),
            Node::File(_) => None,
        },
    };
    if let Some(node @ Node::Dir(_)) = target {
        let old = std::mem::replace(node, Node::Dir(BTreeMap::new()));
        refs.release_node(&old);
    }
}

/// Drops one live name for `id`, and its live bytes with the last one.
fn unlink_live(
    live_refs: &mut HashMap<u64, usize>,
    live_data: &mut HashMap<u64, Vec<u8>>,
    id: u64,
) {
    if let std::collections::hash_map::Entry::Occupied(mut count) = live_refs.entry(id) {
        *count.get_mut() -= 1;
        if *count.get() == 0 {
            count.remove();
            live_data.remove(&id);
        }
    }
}

/// Discards every carried snapshot at or beneath `parts`, releasing what each one named.
fn drop_carried_under(
    carried: &mut HashMap<Vec<OsString>, Node>,
    refs: &mut Refs<'_>,
    parts: &[OsString],
) {
    carried.retain(|key, snapshot| {
        let keep = !key.starts_with(parts);
        if !keep {
            refs.release_node(snapshot);
        }
        keep
    });
}

/// Rebuilds every reference count from the trees, and drops the bytes of every file no tree
/// names. A crash replaces the whole live tree at once, so it alone pays a full walk.
fn recount(inner: &mut Inner) {
    let mut live_ids = Vec::new();
    collect_ids(&inner.live, &mut live_ids);
    inner.live_refs.clear();
    for id in &live_ids {
        *inner.live_refs.entry(*id).or_default() += 1;
    }
    let live_refs = &inner.live_refs;
    inner.live_data.retain(|id, _| live_refs.contains_key(id));
    let mut all = live_ids;
    collect_ids(&inner.durable, &mut all);
    for snapshot in inner.carried.values() {
        walk_ids(snapshot, &mut |id| all.push(id));
    }
    inner.refs.clear();
    for id in all {
        *inner.refs.entry(id).or_default() += 1;
    }
    let refs = &inner.refs;
    inner.durable_data.retain(|id, _| refs.contains_key(id));
}

impl Vfs for MemFs {
    fn read(&self, path: &Path) -> VfsResult<Vec<u8>> {
        let parts = split_path(path, "read")?;
        let holds = self.holds_on(&parts);
        let guard = self.tree();
        match node_at(&guard.live, &parts, "read", path)? {
            Node::File(id) => {
                let bytes = guard.live_data.get(id).cloned().unwrap_or_default();
                holds.check(Access::Read, 0, bytes.len() as u64, "read", path)?;
                Ok(bytes)
            }
            Node::Dir(_) => Err(err(
                VfsErrorKind::IsADirectory,
                "read",
                path,
                "is a directory",
            )),
        }
    }

    fn read_at(&self, path: &Path, offset: u64, len: usize) -> VfsResult<Vec<u8>> {
        let parts = split_path(path, "read_at")?;
        let holds = self.holds_on(&parts);
        let guard = self.tree();
        match node_at(&guard.live, &parts, "read_at", path)? {
            Node::File(id) => {
                let bytes = guard
                    .live_data
                    .get(id)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                // An offset past what a `usize` holds is past the end of any file memory holds.
                let start = usize::try_from(offset)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                let end = start.saturating_add(len).min(bytes.len());
                holds.check(Access::Read, start as u64, end as u64, "read_at", path)?;
                Ok(bytes[start..end].to_vec())
            }
            Node::Dir(_) => Err(err(
                VfsErrorKind::IsADirectory,
                "read_at",
                path,
                "is a directory",
            )),
        }
    }

    fn write(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        let parts = split_path(path, "write")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(VfsErrorKind::InvalidInput, "write", path, "empty path"));
        };
        let holds = self.holds_on(&parts);
        let mut guard = self.tree();
        let Inner {
            live,
            live_data,
            refs,
            live_refs,
            next_inode,
            dirty,
            ..
        } = &mut *guard;
        let dir = dir_of_mut(live, parents, "write", path)?;
        match dir.get(name) {
            Some(Node::Dir(_)) => Err(err(
                VfsErrorKind::IsADirectory,
                "write",
                path,
                "is a directory",
            )),
            Some(Node::File(id)) => {
                let id = *id;
                let old_len = live_data.get(&id).map_or(0, Vec::len);
                let touched = old_len.max(contents.len()) as u64;
                holds.check(Access::Write, 0, touched, "write", path)?;
                live_data.insert(id, contents.to_vec());
                Ok(())
            }
            None => {
                holds.check(Access::Write, 0, contents.len() as u64, "write", path)?;
                let id = next_id(next_inode, "write", path)?;
                live_data.insert(id, contents.to_vec());
                dir.insert(name.clone(), Node::File(id));
                *refs.entry(id).or_default() += 1;
                *live_refs.entry(id).or_default() += 1;
                mark(dirty, parents, name);
                Ok(())
            }
        }
    }

    fn append(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        let parts = split_path(path, "append")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "append",
                path,
                "empty path",
            ));
        };
        let holds = self.holds_on(&parts);
        let mut guard = self.tree();
        let Inner {
            live,
            live_data,
            refs,
            live_refs,
            next_inode,
            dirty,
            ..
        } = &mut *guard;
        let dir = dir_of_mut(live, parents, "append", path)?;
        match dir.get(name) {
            Some(Node::Dir(_)) => Err(err(
                VfsErrorKind::IsADirectory,
                "append",
                path,
                "is a directory",
            )),
            Some(Node::File(id)) => {
                let id = *id;
                let bytes = live_data.entry(id).or_default();
                let old_len = bytes.len() as u64;
                let new_len = old_len + contents.len() as u64;
                holds.check(Access::Write, old_len, new_len, "append", path)?;
                bytes.extend_from_slice(contents);
                Ok(())
            }
            None => {
                holds.check(Access::Write, 0, contents.len() as u64, "append", path)?;
                let id = next_id(next_inode, "append", path)?;
                live_data.insert(id, contents.to_vec());
                dir.insert(name.clone(), Node::File(id));
                *refs.entry(id).or_default() += 1;
                *live_refs.entry(id).or_default() += 1;
                mark(dirty, parents, name);
                Ok(())
            }
        }
    }

    fn truncate(&self, path: &Path, len: u64) -> VfsResult<()> {
        let new_len = usize::try_from(len).map_err(|_| {
            err(
                VfsErrorKind::InvalidInput,
                "truncate",
                path,
                "length too large",
            )
        })?;
        let parts = split_path(path, "truncate")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "truncate",
                path,
                "empty path",
            ));
        };
        let holds = self.holds_on(&parts);
        let mut guard = self.tree();
        let Inner {
            live, live_data, ..
        } = &mut *guard;
        let dir = dir_of_mut(live, parents, "truncate", path)?;
        match dir.get(name) {
            Some(Node::Dir(_)) => Err(err(
                VfsErrorKind::IsADirectory,
                "truncate",
                path,
                "is a directory",
            )),
            Some(Node::File(id)) => {
                let id = *id;
                let bytes = live_data.entry(id).or_default();
                let (low, high) = (bytes.len().min(new_len), bytes.len().max(new_len));
                holds.check(Access::Write, low as u64, high as u64, "truncate", path)?;
                bytes.resize(new_len, 0);
                Ok(())
            }
            None => Err(err(
                VfsErrorKind::NotFound,
                "truncate",
                path,
                "no such file",
            )),
        }
    }

    fn fsync(&self, path: &Path) -> VfsResult<()> {
        let parts = split_path(path, "fsync")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(VfsErrorKind::InvalidInput, "fsync", path, "empty path"));
        };
        let mut guard = self.tree();
        #[cfg(test)]
        let full_sync = guard.full_directory_sync;
        #[cfg(not(test))]
        let full_sync = false;
        let Inner {
            live,
            durable,
            live_data,
            durable_data,
            unnamed,
            carried,
            replaced,
            refs,
            dirty,
            #[cfg(test)]
            examined_entries,
            ..
        } = &mut *guard;
        let mut refs = Refs {
            counts: refs,
            durable_data,
        };
        let resolved = {
            let dir = dir_of(live, parents, "fsync", path)?;
            match dir.get(name) {
                None => return Err(err(VfsErrorKind::NotFound, "fsync", path, "no such path")),
                Some(Node::File(id)) => Resolved::File(*id),
                Some(Node::Dir(_)) => Resolved::Dir,
            }
        };
        match resolved {
            Resolved::File(id) => {
                let bytes = live_data.get(&id).cloned().unwrap_or_default();
                refs.durable_data.insert(id, bytes);
            }
            Resolved::Dir => {
                let parent = dir_of(live, parents, "fsync", path)?;
                let Some(Node::Dir(live_children)) = parent.get(name) else {
                    return Err(err(VfsErrorKind::NotFound, "fsync", path, "no such path"));
                };
                // A directory standing where the durable tree still has a file is new: until
                // its parent's sync names it, its contents are carried, as a renamed
                // directory's are, and a crash drops them while the file survives.
                if let Some(depth) = carried_anchor(durable, carried, &parts) {
                    let mut books = Bookkeeping {
                        carried,
                        replaced,
                        unnamed,
                        refs: &mut refs,
                    };
                    sync_into_carried(&mut books, &parts, depth, live_children, path)?;
                    return Ok(());
                }
                // The implicit root cannot be fsync'd, so its direct entries count as named.
                for depth in 2..=parts.len() {
                    let prefix = &parts[..depth];
                    if node_at(durable, prefix, "fsync", path).is_err() {
                        unnamed.insert(prefix.to_vec());
                    }
                }
                // A directory the durable tree does not hold once its ancestors are renewed
                // starts empty there, so all of its entries are changes.
                for depth in 1..=parts.len() {
                    let prefix = &parts[..depth];
                    renew(durable, carried, replaced, dirty, &mut refs, prefix, path)?;
                    if !matches!(node_at(durable, prefix, "fsync", path), Ok(Node::Dir(_))) {
                        mark_all(dirty, prefix);
                    }
                }
                ensure_dir(durable, &parts, "fsync", path)?;
                // Only the recorded names can differ from the durable directory, unless it was
                // replaced wholesale, when every entry is reconciled.
                let changed: Option<Vec<OsString>> = if full_sync {
                    None
                } else {
                    match dirty.get(&parts) {
                        None => Some(Vec::new()),
                        Some(None) => None,
                        Some(Some(names)) => Some(names.iter().cloned().collect()),
                    }
                };
                let child_dirs: Vec<&OsString> = match &changed {
                    None => live_children
                        .iter()
                        .filter(|(_, node)| matches!(node, Node::Dir(_)))
                        .map(|(child, _)| child)
                        .collect(),
                    Some(names) => names
                        .iter()
                        .filter(|child| matches!(live_children.get(*child), Some(Node::Dir(_))))
                        .collect(),
                };
                // A renamed directory newly named here brings what it carried with it.
                for child in &child_dirs {
                    let mut child_parts = parts.clone();
                    child_parts.push((*child).clone());
                    renew(
                        durable,
                        carried,
                        replaced,
                        dirty,
                        &mut refs,
                        &child_parts,
                        path,
                    )?;
                }
                let ddir = dir_of_mut(durable, &parts, "fsync", path)?;
                #[cfg(test)]
                {
                    *examined_entries += changed
                        .as_ref()
                        .map_or(live_children.len().max(ddir.len()), Vec::len);
                }
                let fresh = match &changed {
                    None => reconcile_children(ddir, live_children, &mut refs),
                    Some(names) => names
                        .iter()
                        .filter(|child| {
                            reconcile_entry(ddir, child, live_children.get(*child), &mut refs)
                        })
                        .cloned()
                        .collect(),
                };
                for child in fresh {
                    let mut child_parts = parts.clone();
                    child_parts.push(child);
                    mark_all(dirty, &child_parts);
                }
                for child in &child_dirs {
                    let mut child_parts = parts.clone();
                    child_parts.push((*child).clone());
                    unnamed.remove(&child_parts);
                }
                dirty.remove(&parts);
                unnamed.retain(|marked| {
                    marked.len() <= parts.len()
                        || !marked.starts_with(&parts)
                        || live_children.contains_key(&marked[parts.len()])
                });
            }
        }
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> VfsResult<()> {
        let from_parts = split_path(from, "rename")?;
        let to_parts = split_path(to, "rename")?;
        if from_parts == to_parts {
            return Ok(());
        }
        // Reject renaming a directory into its own subtree.
        if from_parts.len() < to_parts.len() && to_parts.starts_with(&from_parts) {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "rename",
                to,
                "cannot rename a directory into its own subtree",
            ));
        }
        let Some((from_name, from_parents)) = from_parts.split_last() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "rename",
                from,
                "empty path",
            ));
        };
        let Some((to_name, to_parents)) = to_parts.split_last() else {
            return Err(err(VfsErrorKind::InvalidInput, "rename", to, "empty path"));
        };
        let mut guard = self.tree();
        let Inner {
            live,
            durable,
            live_data,
            durable_data,
            carried,
            replaced,
            refs,
            live_refs,
            dirty,
            ..
        } = &mut *guard;
        let mut refs = Refs {
            counts: refs,
            durable_data,
        };
        let src_kind = {
            let sdir = dir_of(live, from_parents, "rename", from)?;
            match sdir.get(from_name) {
                None => return Err(err(VfsErrorKind::NotFound, "rename", from, "no such path")),
                Some(Node::File(_)) => Kind::File,
                Some(Node::Dir(_)) => Kind::Dir,
            }
        };
        let dst_info = {
            let ddir = dir_of(live, to_parents, "rename", to)?;
            ddir.get(to_name).map(|n| match n {
                Node::File(_) => (Kind::File, true),
                Node::Dir(m) => (Kind::Dir, m.is_empty()),
            })
        };
        match (src_kind, dst_info) {
            (_, None) => {}
            (Kind::File, Some((Kind::File, _))) => {}
            (Kind::File, Some((Kind::Dir, _))) => {
                return Err(err(
                    VfsErrorKind::IsADirectory,
                    "rename",
                    to,
                    "destination is a directory",
                ));
            }
            (Kind::Dir, Some((Kind::File, _))) => {
                return Err(err(
                    VfsErrorKind::NotADirectory,
                    "rename",
                    to,
                    "destination is not a directory",
                ));
            }
            (Kind::Dir, Some((Kind::Dir, empty))) => {
                if !empty {
                    return Err(err(
                        VfsErrorKind::DirectoryNotEmpty,
                        "rename",
                        to,
                        "destination directory is not empty",
                    ));
                }
            }
        }
        let node = {
            let sdir = dir_of_mut(live, from_parents, "rename", from)?;
            sdir.remove(from_name)
        };
        let Some(node) = node else {
            return Err(err(VfsErrorKind::NotFound, "rename", from, "no such path"));
        };
        let ddir = dir_of_mut(live, to_parents, "rename", to)?;
        // Only a file can be displaced: a directory destination was checked empty.
        if let Some(Node::File(displaced)) = ddir.insert(to_name.clone(), node) {
            unlink_live(live_refs, live_data, displaced);
            refs.release(displaced);
        }
        mark(dirty, from_parents, from_name);
        mark(dirty, to_parents, to_name);
        if src_kind == Kind::Dir {
            // Both paths are replaced below, so the next sync naming either starts it, and
            // everything beneath it, from empty and reconciles every entry: the records under
            // either path describe nothing any more.
            dirty.remove(&to_parts);
            if let Ok(moved) = node_at(live, &to_parts, "rename", to) {
                let mut stale = Vec::new();
                for_each_dir(&mut from_parts.clone(), moved, &mut |old| {
                    stale.push(old.to_vec())
                });
                for old in stale {
                    dirty.remove(&old);
                }
            }
        }
        if src_kind == Kind::Dir {
            // What the directory carries is captured now, so syncing its old parent first --
            // which drops the old name from the durable tree -- cannot lose it.
            // The durable tree holds this directory's contents at its path only if nothing at
            // or above that path has been replaced since.
            let durable_is_ours =
                !(1..=from_parts.len()).any(|depth| replaced.contains(&from_parts[..depth]));
            let snapshot = carried
                .remove(&from_parts)
                .or_else(|| extract_carried(carried, replaced, &from_parts))
                .or_else(|| {
                    durable_is_ours
                        .then(|| node_at(durable, &from_parts, "rename", from).ok())
                        .flatten()
                        .filter(|node| matches!(node, Node::Dir(_)))
                        .cloned()
                        .inspect(|copy| refs.retain_node(copy))
                });
            // The destination's own history goes with it: an empty directory renamed over
            // carries nothing, and must not inherit a snapshot the replaced one was carrying.
            for key in carried.keys().cloned().collect::<Vec<_>>() {
                if key.starts_with(&to_parts)
                    && let Some(old) = carried.remove(&key)
                {
                    refs.release_node(&old);
                }
            }
            replaced.retain(|key| key.len() == to_parts.len() || !key.starts_with(&to_parts));
            for map_keys in [
                carried.keys().cloned().collect::<Vec<_>>(),
                replaced.iter().cloned().collect::<Vec<_>>(),
            ] {
                for key in map_keys {
                    if !key.starts_with(&from_parts) || key == from_parts {
                        continue;
                    }
                    let mut rekeyed = to_parts.clone();
                    rekeyed.extend_from_slice(&key[from_parts.len()..]);
                    if let Some(subtree) = carried.remove(&key) {
                        carried.insert(rekeyed.clone(), subtree);
                    }
                    if replaced.remove(&key) {
                        replaced.insert(rekeyed);
                    }
                }
            }
            // Whatever is at either path from now on is not what the durable tree holds there.
            replaced.insert(from_parts.clone());
            replaced.insert(to_parts.clone());
            if let Some(snapshot) = snapshot
                && let Some(old) = carried.insert(to_parts, snapshot)
            {
                refs.release_node(&old);
            }
        }
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> VfsResult<()> {
        let parts = split_path(path, "remove_file")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "remove_file",
                path,
                "empty path",
            ));
        };
        let mut guard = self.tree();
        let Inner {
            live,
            live_data,
            durable_data,
            refs,
            live_refs,
            dirty,
            ..
        } = &mut *guard;
        let mut refs = Refs {
            counts: refs,
            durable_data,
        };
        let dir = dir_of_mut(live, parents, "remove_file", path)?;
        match dir.get(name) {
            None => Err(err(
                VfsErrorKind::NotFound,
                "remove_file",
                path,
                "no such file",
            )),
            Some(Node::Dir(_)) => Err(err(
                VfsErrorKind::IsADirectory,
                "remove_file",
                path,
                "is a directory",
            )),
            Some(Node::File(_)) => {
                if let Some(Node::File(id)) = dir.remove(name) {
                    unlink_live(live_refs, live_data, id);
                    refs.release(id);
                }
                mark(dirty, parents, name);
                Ok(())
            }
        }
    }

    fn create_dir(&self, path: &Path) -> VfsResult<()> {
        let parts = split_path(path, "create_dir")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "create_dir",
                path,
                "empty path",
            ));
        };
        let mut guard = self.tree();
        let Inner { live, dirty, .. } = &mut *guard;
        let dir = dir_of_mut(live, parents, "create_dir", path)?;
        if dir.contains_key(name) {
            return Err(err(
                VfsErrorKind::AlreadyExists,
                "create_dir",
                path,
                "already exists",
            ));
        }
        dir.insert(name.clone(), Node::Dir(BTreeMap::new()));
        mark(dirty, parents, name);
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> VfsResult<()> {
        let parts = split_path(path, "create_dir_all")?;
        let mut guard = self.tree();
        let Inner { live, dirty, .. } = &mut *guard;
        // Every level this creates is a new entry in its parent.
        let mut cur = Some(&*live);
        for depth in 1..=parts.len() {
            cur = match cur.and_then(|dir| dir.get(&parts[depth - 1])) {
                Some(Node::Dir(next)) => Some(next),
                Some(Node::File(_)) => break,
                None => {
                    mark(dirty, &parts[..depth - 1], &parts[depth - 1]);
                    None
                }
            };
        }
        ensure_dir(live, &parts, "create_dir_all", path)?;
        Ok(())
    }

    fn remove_dir(&self, path: &Path) -> VfsResult<()> {
        let parts = split_path(path, "remove_dir")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "remove_dir",
                path,
                "empty path",
            ));
        };
        let mut guard = self.tree();
        let Inner {
            live,
            durable_data,
            carried,
            replaced,
            refs,
            dirty,
            ..
        } = &mut *guard;
        let mut refs = Refs {
            counts: refs,
            durable_data,
        };
        let dir = dir_of_mut(live, parents, "remove_dir", path)?;
        match dir.get(name) {
            None => Err(err(
                VfsErrorKind::NotFound,
                "remove_dir",
                path,
                "no such directory",
            )),
            Some(Node::File(_)) => Err(err(
                VfsErrorKind::NotADirectory,
                "remove_dir",
                path,
                "not a directory",
            )),
            Some(Node::Dir(children)) => {
                if children.is_empty() {
                    dir.remove(name);
                    drop_carried_under(carried, &mut refs, &parts);
                    mark(dirty, parents, name);
                    dirty.remove(&parts);
                    replaced.insert(parts.clone());
                    Ok(())
                } else {
                    Err(err(
                        VfsErrorKind::DirectoryNotEmpty,
                        "remove_dir",
                        path,
                        "directory not empty",
                    ))
                }
            }
        }
    }

    fn remove_dir_all(&self, path: &Path) -> VfsResult<()> {
        let parts = split_path(path, "remove_dir_all")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "remove_dir_all",
                path,
                "empty path",
            ));
        };
        let mut guard = self.tree();
        let Inner {
            live,
            live_data,
            durable_data,
            carried,
            replaced,
            refs,
            live_refs,
            dirty,
            ..
        } = &mut *guard;
        let mut refs = Refs {
            counts: refs,
            durable_data,
        };
        let dir = dir_of_mut(live, parents, "remove_dir_all", path)?;
        match dir.get(name) {
            None => {
                return Err(err(
                    VfsErrorKind::NotFound,
                    "remove_dir_all",
                    path,
                    "no such directory",
                ));
            }
            Some(Node::File(_)) => {
                return Err(err(
                    VfsErrorKind::NotADirectory,
                    "remove_dir_all",
                    path,
                    "not a directory",
                ));
            }
            Some(Node::Dir(_)) => {}
        }
        if let Some(Node::Dir(sub)) = dir.remove(name) {
            let mut ids = Vec::new();
            collect_ids(&sub, &mut ids);
            for id in ids {
                unlink_live(live_refs, live_data, id);
                refs.release(id);
            }
            // Records for directories that no longer exist only cost memory.
            for_each_dir(&mut parts.clone(), &Node::Dir(sub), &mut |dir_parts| {
                dirty.remove(dir_parts);
            });
        }
        mark(dirty, parents, name);
        drop_carried_under(carried, &mut refs, &parts);
        replaced.insert(parts);
        Ok(())
    }

    fn read_dir(&self, path: &Path) -> VfsResult<Vec<DirEntry>> {
        let parts = split_path(path, "read_dir")?;
        let guard = self.tree();
        match node_at(&guard.live, &parts, "read_dir", path)? {
            Node::Dir(children) => Ok(children
                .iter()
                .map(|(k, n)| DirEntry {
                    file_name: k.clone(),
                    file_type: match n {
                        Node::File(_) => FileType::File,
                        Node::Dir(_) => FileType::Dir,
                    },
                })
                .collect()),
            Node::File(_) => Err(err(
                VfsErrorKind::NotADirectory,
                "read_dir",
                path,
                "not a directory",
            )),
        }
    }

    fn metadata(&self, path: &Path) -> VfsResult<Metadata> {
        let parts = split_path(path, "metadata")?;
        let guard = self.tree();
        match node_at(&guard.live, &parts, "metadata", path)? {
            Node::File(id) => Ok(Metadata {
                file_type: FileType::File,
                len: guard.live_data.get(id).map_or(0, |v| v.len() as u64),
            }),
            Node::Dir(_) => Ok(Metadata {
                file_type: FileType::Dir,
                len: 0,
            }),
        }
    }

    // An exclusive hold on the whole file: it excludes every other lock on the path, which on
    // Windows its share mode does too, and nothing a reader of the contract may rely on.
    fn lock_exclusive(&self, path: &Path) -> VfsResult<Box<dyn VfsLock>> {
        self.take_lock(
            path,
            "lock_exclusive",
            LockMode::Exclusive,
            LockSpan::File,
            false,
        )
    }

    fn try_lock(&self, path: &Path, mode: LockMode, span: LockSpan) -> VfsResult<Box<dyn VfsLock>> {
        self.take_lock(path, "try_lock", mode, span, false)
    }

    fn lock(&self, path: &Path, mode: LockMode, span: LockSpan) -> VfsResult<Box<dyn VfsLock>> {
        self.take_lock(path, "lock", mode, span, true)
    }

    fn publish_new(&self, path: &Path, contents: &[u8]) -> VfsResult<()> {
        let parts = split_path(path, "publish_new")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "publish_new",
                path,
                "empty path",
            ));
        };
        let mut guard = self.tree();
        let Inner {
            live,
            live_data,
            durable_data,
            refs,
            live_refs,
            next_inode,
            dirty,
            ..
        } = &mut *guard;
        let dir = dir_of_mut(live, parents, "publish_new", path)?;
        if dir.contains_key(name) {
            return Err(err(
                VfsErrorKind::AlreadyExists,
                "publish_new",
                path,
                "already exists",
            ));
        }
        // Staged, synced and linked into place in one step: the bytes are durable and the name
        // is not, which is the state `OsFs` reaches before its staging sibling is removed.
        let id = next_id(next_inode, "publish_new", path)?;
        live_data.insert(id, contents.to_vec());
        durable_data.insert(id, contents.to_vec());
        dir.insert(name.clone(), Node::File(id));
        *refs.entry(id).or_default() += 1;
        *live_refs.entry(id).or_default() += 1;
        mark(dirty, parents, name);
        Ok(())
    }
}

/// What [`Vfs::fsync`] found at its path, captured so the live borrow can be
/// released before durable state is advanced.
enum Resolved {
    File(u64),
    Dir,
}
