// Copyright (c) Michael Grier.

//! A MemFs directory sync reconciles only the entries changed since that directory was last
//! synced. Every operation sequence is replayed against a filesystem that reconciles every
//! entry on every sync, and the two must agree on every result and on what survives every
//! crash; the cost tests show what a sync examines is bounded by what changed.

use std::collections::BTreeMap;

use super::*;

fn p(path: &str) -> &Path {
    Path::new(path)
}

/// SplitMix64, so every sequence is reproducible from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// The names every path is built from, so operations collide often.
const NAMES: [&str; 3] = ["a", "b", "c"];

/// A path of one to three components.
fn random_path(rng: &mut Rng) -> PathBuf {
    let depth = 1 + rng.below(3);
    let mut path = PathBuf::from("/");
    for _ in 0..depth {
        path.push(NAMES[rng.below(NAMES.len() as u64) as usize]);
    }
    path
}

#[derive(Debug, Clone)]
enum Op {
    Write(PathBuf, Vec<u8>),
    Append(PathBuf, Vec<u8>),
    Truncate(PathBuf),
    RemoveFile(PathBuf),
    CreateDir(PathBuf),
    CreateDirAll(PathBuf),
    RemoveDir(PathBuf),
    RemoveDirAll(PathBuf),
    Rename(PathBuf, PathBuf),
    Fsync(PathBuf),
    Crash,
}

fn random_op(rng: &mut Rng, step: u64) -> Op {
    let bytes = step.to_le_bytes().to_vec();
    match rng.below(20) {
        0..=2 => Op::Write(random_path(rng), bytes),
        3 => Op::Append(random_path(rng), bytes),
        4 => Op::Truncate(random_path(rng)),
        5 => Op::RemoveFile(random_path(rng)),
        6..=7 => Op::CreateDir(random_path(rng)),
        8 => Op::CreateDirAll(random_path(rng)),
        9 => Op::RemoveDir(random_path(rng)),
        10 => Op::RemoveDirAll(random_path(rng)),
        11..=12 => Op::Rename(random_path(rng), random_path(rng)),
        13..=18 => Op::Fsync(random_path(rng)),
        _ => Op::Crash,
    }
}

fn apply(fs: &MemFs, op: &Op) -> Result<(), VfsErrorKind> {
    let result = match op {
        Op::Write(path, bytes) => fs.write(path, bytes),
        Op::Append(path, bytes) => fs.append(path, bytes),
        Op::Truncate(path) => fs.truncate(path, 1),
        Op::RemoveFile(path) => fs.remove_file(path),
        Op::CreateDir(path) => fs.create_dir(path),
        Op::CreateDirAll(path) => fs.create_dir_all(path),
        Op::RemoveDir(path) => fs.remove_dir(path),
        Op::RemoveDirAll(path) => fs.remove_dir_all(path),
        Op::Rename(from, to) => fs.rename(from, to),
        Op::Fsync(path) => fs.fsync(path),
        Op::Crash => {
            fs.simulate_crash();
            Ok(())
        }
    };
    result.map_err(|error| error.kind())
}

/// Every path in the tree, mapped to its bytes, or `None` for a directory.
fn tree(fs: &MemFs) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(fs: &MemFs, dir: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs.read_dir(dir).unwrap() {
            let path = dir.join(&entry.file_name);
            if entry.file_type == FileType::Dir {
                out.insert(path.clone(), None);
                walk(fs, &path, out);
            } else {
                out.insert(path.clone(), Some(fs.read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    // The root cannot be listed, so the walk starts from every name a path can begin with.
    for top in NAMES {
        let path = Path::new("/").join(top);
        match fs.metadata(&path) {
            Ok(meta) if meta.file_type == FileType::Dir => {
                out.insert(path.clone(), None);
                walk(fs, &path, &mut out);
            }
            Ok(_) => {
                out.insert(path.clone(), Some(fs.read(&path).unwrap()));
            }
            Err(error) => assert_eq!(error.kind(), VfsErrorKind::NotFound),
        }
    }
    out
}

#[test]
fn mem_a_tracked_directory_sync_agrees_with_reconciling_every_entry() {
    for seed in 0..64 {
        let mut rng = Rng(seed);
        let tracked = MemFs::new();
        let oracle = MemFs::with_full_directory_sync();
        for step in 0..400 {
            let op = random_op(&mut rng, step);
            let got = apply(&tracked, &op);
            let want = apply(&oracle, &op);
            assert_eq!(got, want, "seed {seed} step {step}: {op:?}");
            if matches!(op, Op::Crash) {
                assert_eq!(
                    tree(&tracked),
                    tree(&oracle),
                    "seed {seed} step {step}: the survivors of a crash differ"
                );
                tracked.audit_refs();
            }
        }
        tracked.simulate_crash();
        oracle.simulate_crash();
        assert_eq!(tree(&tracked), tree(&oracle), "seed {seed}: final crash");
        tracked.audit_refs();
    }
}

/// Builds `count` shards under `/p` the way a store does: each shard's file is synced, then
/// the shard, then `/p` to name it. Returns the entries the `/p` syncs examined.
fn grow_shards(fs: &MemFs, count: usize) -> usize {
    fs.create_dir(p("/p")).unwrap();
    let mut examined = 0;
    for shard in 0..count {
        let dir = PathBuf::from(format!("/p/s{shard}"));
        fs.create_dir(&dir).unwrap();
        fs.write(&dir.join("f"), b"x").unwrap();
        fs.fsync(&dir.join("f")).unwrap();
        fs.fsync(&dir).unwrap();
        let before = fs.examined_entries();
        fs.fsync(p("/p")).unwrap();
        examined += fs.examined_entries() - before;
    }
    examined
}

#[test]
fn mem_a_directory_sync_examines_only_what_changed_since_its_last_sync() {
    let fs = MemFs::new();
    let examined = grow_shards(&fs, 300);
    assert_eq!(
        examined, 300,
        "each sync of /p examines only the one new shard"
    );
    fs.simulate_crash();
    assert_eq!(fs.read_dir(p("/p")).unwrap().len(), 300);
    assert_eq!(fs.read(p("/p/s299/f")).unwrap(), b"x");
    fs.audit_refs();
}

#[test]
fn mem_reconciling_every_entry_examines_the_whole_directory_each_time() {
    let fs = MemFs::with_full_directory_sync();
    assert_eq!(grow_shards(&fs, 300), (1..=300).sum::<usize>());
}

#[test]
fn mem_a_directory_sync_with_nothing_changed_examines_nothing() {
    let fs = MemFs::new();
    grow_shards(&fs, 10);
    let before = fs.examined_entries();
    fs.fsync(p("/p")).unwrap();
    assert_eq!(fs.examined_entries(), before);
}

#[test]
fn mem_a_directory_new_to_the_durable_tree_reconciles_every_entry() {
    let fs = MemFs::new();
    fs.create_dir_all(p("/d/x")).unwrap();
    for name in ["f", "g", "h"] {
        fs.write(&p("/d/x").join(name), b"v").unwrap();
        fs.fsync(&p("/d/x").join(name)).unwrap();
    }
    let before = fs.examined_entries();
    fs.fsync(p("/d/x")).unwrap();
    assert_eq!(fs.examined_entries() - before, 3);
    fs.fsync(p("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read_dir(p("/d/x")).unwrap().len(), 3);
}

#[test]
fn mem_changes_inside_a_renamed_directory_follow_it() {
    let fs = MemFs::new();
    fs.create_dir_all(p("/d/x")).unwrap();
    fs.fsync(p("/d/x")).unwrap();
    fs.fsync(p("/d")).unwrap();
    // A change recorded in /d/x, unsynced, then carried to /e/y by the rename.
    fs.write(p("/d/x/f"), b"v").unwrap();
    fs.fsync(p("/d/x/f")).unwrap();
    fs.create_dir(p("/e")).unwrap();
    fs.rename(p("/d/x"), p("/e/y")).unwrap();
    fs.fsync(p("/e/y")).unwrap();
    fs.fsync(p("/e")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(p("/e/y/f")).unwrap(), b"v");
    assert!(!fs.exists(p("/d/x")).unwrap());
    fs.audit_refs();
}

#[test]
fn mem_a_directory_recreated_after_removal_does_not_keep_stale_records() {
    let fs = MemFs::new();
    fs.create_dir_all(p("/d/x")).unwrap();
    fs.write(p("/d/x/f"), b"v").unwrap();
    fs.remove_dir_all(p("/d/x")).unwrap();
    fs.create_dir(p("/d/x")).unwrap();
    fs.fsync(p("/d/x")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.simulate_crash();
    assert!(fs.read_dir(p("/d/x")).unwrap().is_empty());
    fs.audit_refs();
}
