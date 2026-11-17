// Copyright (c) Michael Grier.
// Split from mem.rs at 2367be3.

use super::*;

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
                // A directory the durable tree does not hold once its ancestors are renewed has
                // no durable name yet -- the implicit root cannot be fsync'd, so its direct
                // entries count as named -- and starts empty there, so all of its entries are
                // changes. One a renewed ancestor's carried snapshot holds is already named in it.
                for depth in 1..=parts.len() {
                    let prefix = &parts[..depth];
                    if depth >= 2 && node_at(durable, prefix, "fsync", path).is_err() {
                        unnamed.insert(prefix.to_vec());
                    }
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
            unnamed,
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
            // A descendant whose name the moved directory has not made durable is just as
            // unnamed in the snapshot; the mark stays at the old path too, which the durable tree
            // keeps holding until the old parent syncs.
            let moved_unnamed: Vec<Vec<OsString>> = unnamed
                .iter()
                .filter(|key| key.len() > from_parts.len() && key.starts_with(&from_parts))
                .map(|key| {
                    let mut rekeyed = to_parts.clone();
                    rekeyed.extend_from_slice(&key[from_parts.len()..]);
                    rekeyed
                })
                .collect();
            unnamed.extend(moved_unnamed);
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

    fn boot_identity(&self) -> VfsResult<Vec<u8>> {
        Ok(self.tree().boots.to_le_bytes().to_vec())
    }
}

/// What [`Vfs::fsync`] found at its path, captured so the live borrow can be
/// released before durable state is advanced.
enum Resolved {
    File(u64),
    Dir,
}
