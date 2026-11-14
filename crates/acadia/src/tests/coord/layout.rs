// Copyright (c) Michael Grier.

//! The file-based implementation's layout: names that keep to the reserved prefix, lock files
//! that hold no data, and locks that never overlap.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::LockSpan;
use crate::coord::RESERVED_PREFIX;
use crate::coord::file::layout::{Layout, REGISTRY_ENTRIES};

fn root() -> PathBuf {
    PathBuf::from("/store")
}

fn layouts() -> [Layout; 2] {
    [Layout::new(&root()), Layout::alternate(&root())]
}

/// The name directly beneath the root that `path` lies in.
fn top_level(path: &Path) -> &OsStr {
    path.strip_prefix(root())
        .expect("beneath the root")
        .iter()
        .next()
        .expect("a name")
}

#[test]
fn every_name_beneath_the_root_has_the_reserved_prefix() {
    for layout in layouts() {
        for path in layout.lock_files().iter().chain(&layout.data_files()) {
            let name = top_level(path).to_str().expect("ASCII");
            assert!(name.starts_with(RESERVED_PREFIX), "{name}");
        }
    }
}

#[test]
fn no_lock_file_is_a_data_file() {
    for layout in layouts() {
        let locks: HashSet<_> = layout.lock_files().into_iter().collect();
        let data: HashSet<_> = layout.data_files().into_iter().collect();
        assert_eq!(
            locks.len(),
            layout.lock_files().len(),
            "lock files are distinct"
        );
        assert_eq!(
            data.len(),
            layout.data_files().len(),
            "data files are distinct"
        );
        assert!(locks.is_disjoint(&data));
    }
}

#[test]
fn every_lock_file_lies_beside_or_within_the_state() {
    for layout in layouts() {
        for lock in layout.lock_files() {
            let parent = lock.parent().expect("a parent");
            assert!(
                parent == layout.root() || parent == layout.state_dir(),
                "{}",
                lock.display()
            );
        }
    }
}

#[test]
fn the_tiers_are_distinct_bytes() {
    let (reader, writer) = (Layout::reader_tier(), Layout::writer_tier());
    assert!(!overlap(reader, writer));
}

#[test]
fn no_two_registry_locks_overlap() {
    let spans: Vec<_> = (0..REGISTRY_ENTRIES)
        .flat_map(|entry| [Layout::claim(entry), Layout::liveness(entry)])
        .collect();
    for (i, a) in spans.iter().enumerate() {
        for b in &spans[i + 1..] {
            assert!(!overlap(*a, *b), "{a:?} and {b:?}");
        }
    }
}

#[test]
fn every_registry_lock_is_a_valid_span() {
    for entry in 0..REGISTRY_ENTRIES {
        for span in [Layout::claim(entry), Layout::liveness(entry)] {
            assert!(span.bounds().is_some(), "{span:?}");
        }
    }
}

#[test]
fn every_registry_entry_has_a_file_of_its_own() {
    let layout = Layout::new(&root());
    let files: HashSet<_> = (0..REGISTRY_ENTRIES)
        .map(|entry| layout.registry_entry(entry))
        .collect();
    assert_eq!(files.len(), REGISTRY_ENTRIES);
    for file in files {
        assert_eq!(file.parent(), Some(layout.registry_dir().as_path()));
    }
}

#[test]
fn the_two_implementations_differ_only_in_their_creation_exclusion() {
    let [file, alternate] = layouts();
    assert_ne!(file.creation_lock(), alternate.creation_lock());
    let others = |layout: &Layout| {
        let mut locks = layout.lock_files();
        locks.retain(|lock| *lock != layout.creation_lock());
        (locks, layout.data_files())
    };
    assert_eq!(others(&file), others(&alternate));
}

#[test]
fn a_staged_count_is_a_name_of_its_own_beside_the_count() {
    let layout = Layout::new(&root());
    let staged = layout.count_staging("1234");
    assert_ne!(staged, layout.count());
    assert_eq!(staged.parent(), layout.count().parent());
    assert!(!layout.data_files().contains(&staged));
    assert!(!layout.lock_files().contains(&staged));
}

fn overlap(a: LockSpan, b: LockSpan) -> bool {
    let (a_start, a_end) = a.bounds().expect("valid");
    let (b_start, b_end) = b.bounds().expect("valid");
    a_start < b_end && b_start < a_end
}
