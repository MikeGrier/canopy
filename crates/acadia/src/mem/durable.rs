// Copyright (c) Michael Grier.
// Split from mem.rs at 9f8f908.

use super::*;

mod fsync;

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
/// Returns whether a durable directory at `parts` was replaced, so nothing that was beneath it
/// is there any more.
fn renew(
    durable: &mut BTreeMap<OsString, Node>,
    carried: &mut HashMap<Vec<OsString>, Node>,
    replaced: &mut HashSet<Vec<OsString>>,
    dirty: &mut HashMap<Vec<OsString>, Option<HashSet<OsString>>>,
    refs: &mut Refs<'_>,
    parts: &[OsString],
    full: &Path,
) -> VfsResult<bool> {
    let held_directory = matches!(node_at(durable, parts, "fsync", full), Ok(Node::Dir(_)));
    let refreshed = refresh(durable, replaced, refs, parts);
    if refreshed {
        mark_all(dirty, parts);
    }
    let materialized = materialize(durable, carried, refs, parts, "fsync", full)?;
    if materialized && let Ok(node) = node_at(durable, parts, "fsync", full) {
        mark_subtree_all(dirty, &mut parts.to_vec(), node);
    }
    Ok(held_directory && (refreshed || materialized))
}

/// Brings the unnamed marks beneath `parts` up to date once a sync has renewed the durable
/// directory there: the marks of a directory it `replaced` name nothing any more, and a carried
/// snapshot it materialized brings the marks it carried.
fn adopt_marks(
    unnamed: &mut HashSet<Vec<OsString>>,
    carried_unnamed: &mut HashMap<Vec<OsString>, Vec<Vec<OsString>>>,
    carried: &HashMap<Vec<OsString>, Node>,
    parts: &[OsString],
    replaced: bool,
) {
    if replaced {
        unnamed.retain(|marked| marked.len() <= parts.len() || !marked.starts_with(parts));
    }
    if !carried.contains_key(parts)
        && let Some(marks) = carried_unnamed.remove(parts)
    {
        unnamed.extend(
            marks
                .into_iter()
                .map(|relative| [parts, &relative[..]].concat()),
        );
    }
}

/// Marks the directory `node`, at `path`, and every directory beneath it for a full sync.
fn mark_subtree_all(
    dirty: &mut HashMap<Vec<OsString>, Option<HashSet<OsString>>>,
    path: &mut Vec<OsString>,
    node: &Node,
) {
    for_each_dir(path, node, &mut |dir_parts| mark_all(dirty, dir_parts));
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
        let snapshot = carried.get(prefix);
        let over = anchor.and(snapshot);
        view = match (over, below) {
            (None, None) if snapshot.is_some_and(|node| matches!(node, Node::Dir(_))) => {
                anchor = Some(k);
                snapshot.and_then(|node| match node {
                    Node::Dir(map) => Some(map),
                    Node::File(_) => None,
                })
            }
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
    carried_unnamed: &'a mut HashMap<Vec<OsString>, Vec<Vec<OsString>>>,
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
        let mut emptied =
            books.replaced.remove(prefix) && reset_within(snapshot, relative, books.refs);
        if books.carried.contains_key(prefix) {
            let Node::Dir(map) = &mut *snapshot else {
                return Ok(());
            };
            let (name, between) = relative.split_last().expect("below the anchor");
            let dir = ensure_dir(map, between, "fsync", full)?;
            if let Some(nested) = books.carried.remove(prefix)
                && let Some(displaced) = dir.insert(name.clone(), nested)
            {
                emptied |= matches!(displaced, Node::Dir(_));
                books.refs.release_node(&displaced);
            }
        }
        adopt_marks(
            books.unnamed,
            books.carried_unnamed,
            books.carried,
            prefix,
            emptied,
        );
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
            || matches!(live_children.get(&marked[parts.len()]), Some(Node::Dir(_)))
    });
    retain_replaced_marks(books.replaced, books.carried, parts, live_children);
    Ok(())
}

/// Drops the replacement marks at and beneath each child of `parts` that a sync has just made
/// absent in what it reconciled: no directory is left there for a mark to start from empty,
/// unless a carried snapshot is.
pub(super) fn retain_replaced_marks(
    replaced: &mut HashSet<Vec<OsString>>,
    carried: &HashMap<Vec<OsString>, Node>,
    parts: &[OsString],
    live_children: &BTreeMap<OsString, Node>,
) {
    replaced.retain(|marked| {
        marked.len() <= parts.len()
            || !marked.starts_with(parts)
            || live_children.contains_key(&marked[parts.len()])
            || carried
                .keys()
                .any(|key| key.starts_with(&marked[..=parts.len()]))
    });
}

/// Empties the directory at `relative` inside `snapshot`, releasing what it held. Returns
/// whether there was a directory there to empty.
fn reset_within(snapshot: &mut Node, relative: &[OsString], refs: &mut Refs<'_>) -> bool {
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
        return true;
    }
    false
}
