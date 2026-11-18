// Copyright (c) Michael Grier.
// Split from vfs.rs at 9f8f908.

use super::*;

impl MemFs {
    /// The body of [`Vfs::fsync`].
    pub(in crate::mem) fn sync_path(&self, path: &Path) -> VfsResult<()> {
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
            carried_unnamed,
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
                        carried_unnamed,
                        replaced,
                        unnamed,
                        refs: &mut refs,
                    };
                    sync_into_carried(&mut books, &parts, depth, live_children, path)?;
                    return Ok(());
                }
                // A directory the durable tree does not hold once its ancestors are renewed has
                // no durable name yet -- the implicit root cannot be fsync'd, so its direct
                // entries count as named -- and starts empty there, so all of its entries are
                // changes. One a renewed ancestor's carried snapshot holds is already named in it.
                for depth in 1..=parts.len() {
                    let prefix = &parts[..depth];
                    if depth >= 2 && node_at(durable, prefix, "fsync", path).is_err() {
                        unnamed.insert(prefix.to_vec());
                    }
                    let emptied =
                        renew(durable, carried, replaced, dirty, &mut refs, prefix, path)?;
                    adopt_marks(unnamed, carried_unnamed, carried, prefix, emptied);
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
                    let emptied = renew(
                        durable,
                        carried,
                        replaced,
                        dirty,
                        &mut refs,
                        &child_parts,
                        path,
                    )?;
                    adopt_marks(unnamed, carried_unnamed, carried, &child_parts, emptied);
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
                // A mark beneath a child that is no longer a directory names nothing durable.
                unnamed.retain(|marked| {
                    marked.len() <= parts.len()
                        || !marked.starts_with(&parts)
                        || matches!(live_children.get(&marked[parts.len()]), Some(Node::Dir(_)))
                });
                retain_replaced_marks(replaced, carried, &parts, live_children);
            }
        }
        Ok(())
    }
}

/// What [`Vfs::fsync`] found at its path, captured so the live borrow can be
/// released before durable state is advanced.
enum Resolved {
    File(u64),
    Dir,
}
