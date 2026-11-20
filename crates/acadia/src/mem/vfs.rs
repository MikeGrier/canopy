// Copyright (c) Michael Grier.
// Split from mem.rs at 2367be3.

use super::*;

impl Vfs for MemFs {
    fn check_process(&self, op: &'static str, path: &Path) -> VfsResult<()> {
        validate_path_components(path, op)?;
        let table = self.table();
        table.running(self.process.load(Ordering::Acquire), op, path)
    }

    fn commit_process(
        &self,
        op: &'static str,
        path: &Path,
        commit: &mut dyn FnMut(),
    ) -> VfsResult<()> {
        validate_path_components(path, op)?;
        let table = self.table();
        table.running(self.process.load(Ordering::Acquire), op, path)?;
        #[cfg(test)]
        self.run_after_check();
        commit();
        Ok(())
    }

    fn read(&self, path: &Path) -> VfsResult<Vec<u8>> {
        self.check_process("read", path)?;
        let parts = split_path(path, "read")?;
        let holds = self.holds("read", path)?;
        let guard = self.tree();
        match node_at(&guard.live, &parts, "read", path)? {
            Node::File(id) => {
                let bytes = guard
                    .live_data
                    .get(id)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                holds.check(*id, Access::Read, 0, bytes.len() as u64, "read", path)?;
                Ok(bytes.to_vec())
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
        self.check_process("read_at", path)?;
        let parts = split_path(path, "read_at")?;
        let holds = self.holds("read_at", path)?;
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
                holds.check(*id, Access::Read, start as u64, end as u64, "read_at", path)?;
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
        self.check_process("write", path)?;
        let parts = split_path(path, "write")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(VfsErrorKind::InvalidInput, "write", path, "empty path"));
        };
        let holds = self.holds("write", path)?;
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
                holds.check(id, Access::Write, 0, touched, "write", path)?;
                live_data.insert(id, contents.to_vec());
                Ok(())
            }
            None => {
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
        self.check_process("append", path)?;
        let parts = split_path(path, "append")?;
        let Some((name, parents)) = parts.split_last() else {
            return Err(err(
                VfsErrorKind::InvalidInput,
                "append",
                path,
                "empty path",
            ));
        };
        let holds = self.holds("append", path)?;
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
                holds.check(id, Access::Write, old_len, new_len, "append", path)?;
                bytes.extend_from_slice(contents);
                Ok(())
            }
            None => {
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
        self.check_process("truncate", path)?;
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
        let holds = self.holds("truncate", path)?;
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
                holds.check(id, Access::Write, low as u64, high as u64, "truncate", path)?;
                bytes
                    .try_reserve(new_len.saturating_sub(bytes.len()))
                    .map_err(|_| {
                        err(
                            VfsErrorKind::InvalidInput,
                            "truncate",
                            path,
                            "length too large",
                        )
                    })?;
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
        let _process = self.holds("fsync", path)?;
        self.sync_path(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> VfsResult<()> {
        let _process = self.holds("rename", from)?;
        let from_parts = split_path(from, "rename")?;
        let to_parts = split_path(to, "rename")?;
        if from_parts == to_parts {
            // Renaming a path onto itself changes nothing, but only a path that exists.
            return node_at(&self.tree().live, &from_parts, "rename", from).map(|_| ());
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
            carried_unnamed,
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
        let (src_kind, src_file) = {
            let sdir = dir_of(live, from_parents, "rename", from)?;
            match sdir.get(from_name) {
                None => return Err(err(VfsErrorKind::NotFound, "rename", from, "no such path")),
                Some(Node::File(id)) => (Kind::File, Some(*id)),
                Some(Node::Dir(_)) => (Kind::Dir, None),
            }
        };
        let dst_info = {
            let ddir = dir_of(live, to_parents, "rename", to)?;
            if let (Some(Node::File(id)), Some(source)) = (ddir.get(to_name), src_file)
                && *id == source
            {
                // Two names of one file, which a crash can leave: renaming one over the other
                // changes nothing, as POSIX `rename` does.
                return Ok(());
            }
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
            // The unnamed marks beneath the directory travel with its snapshot. Those beneath its
            // old path stay too, for whatever the durable tree still holds there.
            let durable_holds_a_file = matches!(
                node_at(durable, &from_parts, "rename", from),
                Ok(Node::File(_))
            );
            let snapshot = match carried.remove(&from_parts) {
                Some(node) => {
                    let mut marks = carried_unnamed.remove(&from_parts).unwrap_or_default();
                    // Beneath a durable file, the marks there are this snapshot's own.
                    if durable_holds_a_file {
                        marks.extend(marks_beneath(unnamed, &from_parts));
                    }
                    Some((node, marks))
                }
                None => {
                    extract_carried(carried, carried_unnamed, replaced, &from_parts).or_else(|| {
                        durable_is_ours
                            .then(|| node_at(durable, &from_parts, "rename", from).ok())
                            .flatten()
                            .filter(|node| matches!(node, Node::Dir(_)))
                            .cloned()
                            .inspect(|copy| refs.retain_node(copy))
                            .map(|copy| (copy, marks_beneath(unnamed, &from_parts)))
                    })
                }
            };
            // The destination's own history goes with it: an empty directory renamed over
            // carries nothing, and must not inherit a snapshot the replaced one was carrying.
            for key in carried.keys().cloned().collect::<Vec<_>>() {
                if key.starts_with(&to_parts)
                    && let Some(old) = carried.remove(&key)
                {
                    refs.release_node(&old);
                }
            }
            carried_unnamed.retain(|key, _| !key.starts_with(&to_parts));
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
                    if let Some(marks) = carried_unnamed.remove(&key) {
                        carried_unnamed.insert(rekeyed.clone(), marks);
                    }
                    if replaced.remove(&key) {
                        replaced.insert(rekeyed);
                    }
                }
            }
            // Whatever is at either path from now on is not what the durable tree holds there.
            replaced.insert(from_parts.clone());
            replaced.insert(to_parts.clone());
            if let Some((snapshot, marks)) = snapshot {
                if !marks.is_empty() {
                    carried_unnamed.insert(to_parts.clone(), marks);
                }
                if let Some(old) = carried.insert(to_parts, snapshot) {
                    refs.release_node(&old);
                }
            }
        }
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> VfsResult<()> {
        let _process = self.holds("remove_file", path)?;
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
        let _process = self.holds("create_dir", path)?;
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
        let _process = self.holds("create_dir_all", path)?;
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
        let _process = self.holds("remove_dir", path)?;
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
            carried_unnamed,
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
                    drop_carried_under(carried, carried_unnamed, &mut refs, &parts);
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
        let _process = self.holds("remove_dir_all", path)?;
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
            carried_unnamed,
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
        drop_carried_under(carried, carried_unnamed, &mut refs, &parts);
        replaced.insert(parts);
        Ok(())
    }

    fn read_dir(&self, path: &Path) -> VfsResult<Vec<DirEntry>> {
        let _process = self.holds("read_dir", path)?;
        let parts = split_path(path, "read_dir")?;
        let guard = self.tree();
        match node_at(&guard.live, &parts, "read_dir", path)? {
            Node::Dir(children) => children
                .iter()
                .map(|(k, n)| {
                    let file_name = k.to_str().ok_or_else(|| {
                        err(
                            VfsErrorKind::InvalidInput,
                            "read_dir",
                            path,
                            "a directory entry is not valid UTF-8",
                        )
                    })?;
                    Ok(DirEntry {
                        file_name: file_name.to_owned(),
                        file_type: match n {
                            Node::File(_) => FileType::File,
                            Node::Dir(_) => FileType::Dir,
                        },
                    })
                })
                .collect(),
            Node::File(_) => Err(err(
                VfsErrorKind::NotADirectory,
                "read_dir",
                path,
                "not a directory",
            )),
        }
    }

    fn metadata(&self, path: &Path) -> VfsResult<Metadata> {
        let _process = self.holds("metadata", path)?;
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
        let _process = self.holds("publish_new", path)?;
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
        let _process = self.holds("boot_identity", Path::new(""))?;
        Ok(self.tree().boots.to_le_bytes().to_vec())
    }
}
