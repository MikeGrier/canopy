// Copyright (c) Michael Grier.
// Split from tests.rs at 0b1f7d4.

//! The shared two-backend conformance suite, the error taxonomy, and the
//! durability model (VFS-5).

use super::*;

// ---------------------------------------------------------------------------
// Shared conformance suite -- asserted identically on OsFs and MemFs.
// ---------------------------------------------------------------------------

#[test]
fn write_then_read_roundtrips() {
    on_both("write_then_read_roundtrips", |fs, root| {
        let f = root.join("file.bin");
        fs.write(&f, b"hello world").unwrap();
        assert_eq!(fs.read(&f).unwrap(), b"hello world");
    });
}

#[test]
fn write_overwrites_existing_contents() {
    on_both("write_overwrites_existing_contents", |fs, root| {
        let f = root.join("f");
        fs.write(&f, b"the original, longer contents").unwrap();
        fs.write(&f, b"short").unwrap();
        assert_eq!(fs.read(&f).unwrap(), b"short");
    });
}

#[test]
fn append_extends_and_creates() {
    on_both("append_extends_and_creates", |fs, root| {
        let f = root.join("log");
        // Append to a non-existent file creates it.
        fs.append(&f, b"ab").unwrap();
        fs.append(&f, b"cd").unwrap();
        fs.append(&f, b"ef").unwrap();
        assert_eq!(fs.read(&f).unwrap(), b"abcdef");
    });
}

#[test]
fn truncate_grows_with_zeros_and_shrinks() {
    on_both("truncate_grows_with_zeros_and_shrinks", |fs, root| {
        let f = root.join("f");
        fs.write(&f, b"hello").unwrap();
        fs.truncate(&f, 3).unwrap();
        assert_eq!(fs.read(&f).unwrap(), b"hel");
        fs.truncate(&f, 5).unwrap();
        assert_eq!(fs.read(&f).unwrap(), b"hel\0\0");
        fs.truncate(&f, 0).unwrap();
        assert_eq!(fs.read(&f).unwrap(), b"");
    });
}

#[test]
fn create_dir_and_read_dir_lists_entries() {
    on_both("create_dir_and_read_dir_lists_entries", |fs, root| {
        let dir = root.join("d");
        fs.create_dir(&dir).unwrap();
        fs.write(&dir.join("a"), b"1").unwrap();
        fs.write(&dir.join("b"), b"2").unwrap();
        fs.create_dir(&dir.join("sub")).unwrap();

        let mut by_name = std::collections::BTreeMap::new();
        for e in fs.read_dir(&dir).unwrap() {
            by_name.insert(e.file_name.to_string_lossy().into_owned(), e.file_type);
        }
        assert_eq!(by_name.remove("a"), Some(FileType::File));
        assert_eq!(by_name.remove("b"), Some(FileType::File));
        assert_eq!(by_name.remove("sub"), Some(FileType::Dir));
        assert!(by_name.is_empty(), "unexpected extra entries: {by_name:?}");
    });
}

#[test]
fn rename_moves_file_and_source_disappears() {
    on_both("rename_moves_file_and_source_disappears", |fs, root| {
        let a = root.join("a");
        let b = root.join("b");
        fs.write(&a, b"payload").unwrap();
        fs.rename(&a, &b).unwrap();
        assert_eq!(fs.read(&b).unwrap(), b"payload");
        assert!(!fs.exists(&a).unwrap());
        assert!(fs.exists(&b).unwrap());
    });
}

#[test]
fn rename_atomically_replaces_destination() {
    on_both("rename_atomically_replaces_destination", |fs, root| {
        let a = root.join("a");
        let b = root.join("b");
        fs.write(&a, b"source").unwrap();
        fs.write(&b, b"destination-old").unwrap();
        fs.rename(&a, &b).unwrap();
        // The destination is exactly the source's bytes -- never a mix of the
        // two and never the old destination.
        assert_eq!(fs.read(&b).unwrap(), b"source");
        assert!(!fs.exists(&a).unwrap());
    });
}

#[test]
fn remove_file_deletes() {
    on_both("remove_file_deletes", |fs, root| {
        let f = root.join("f");
        fs.write(&f, b"x").unwrap();
        assert!(fs.exists(&f).unwrap());
        fs.remove_file(&f).unwrap();
        assert!(!fs.exists(&f).unwrap());
        assert_kind(fs.read(&f), VfsErrorKind::NotFound);
    });
}

#[test]
fn remove_empty_dir() {
    on_both("remove_empty_dir", |fs, root| {
        let d = root.join("d");
        fs.create_dir(&d).unwrap();
        fs.remove_dir(&d).unwrap();
        assert!(!fs.exists(&d).unwrap());
    });
}

#[test]
fn remove_dir_all_is_recursive() {
    on_both("remove_dir_all_is_recursive", |fs, root| {
        let d = root.join("tree");
        fs.create_dir(&d).unwrap();
        fs.create_dir(&d.join("sub")).unwrap();
        fs.write(&d.join("sub").join("a"), b"1").unwrap();
        fs.write(&d.join("b"), b"2").unwrap();
        fs.remove_dir_all(&d).unwrap();
        assert!(!fs.exists(&d).unwrap());
    });
}

#[test]
fn metadata_and_exists_report_type_and_len() {
    on_both("metadata_and_exists_report_type_and_len", |fs, root| {
        let f = root.join("f");
        let d = root.join("d");
        fs.write(&f, b"12345").unwrap();
        fs.create_dir(&d).unwrap();

        let fm = fs.metadata(&f).unwrap();
        assert!(fm.is_file());
        assert!(!fm.is_dir());
        assert_eq!(fm.len, 5);

        let dm = fs.metadata(&d).unwrap();
        assert!(dm.is_dir());
        assert!(!dm.is_file());

        assert!(fs.exists(&f).unwrap());
        assert!(!fs.exists(&root.join("nope")).unwrap());
    });
}

#[test]
fn create_dir_all_is_idempotent_and_nested() {
    on_both("create_dir_all_is_idempotent_and_nested", |fs, root| {
        let nested = root.join("x").join("y").join("z");
        fs.create_dir_all(&nested).unwrap();
        fs.create_dir_all(&nested).unwrap();
        assert!(fs.metadata(&nested).unwrap().is_dir());
    });
}

#[test]
fn fsync_file_and_dir_succeed() {
    on_both("fsync_file_and_dir_succeed", |fs, root| {
        let f = root.join("f");
        fs.write(&f, b"durable?").unwrap();
        fs.fsync(&f).unwrap();
        fs.fsync(root).unwrap();
        assert_eq!(fs.read(&f).unwrap(), b"durable?");
    });
}

#[test]
fn read_missing_file_is_not_found_on_both() {
    on_both("read_missing_file_is_not_found_on_both", |fs, root| {
        assert_kind(fs.read(&root.join("absent")), VfsErrorKind::NotFound);
    });
}

// ---------------------------------------------------------------------------
// Error taxonomy -- exact VfsErrorKind pinned against the mock (the reference
// implementation of the Canopy-owned taxonomy).
// ---------------------------------------------------------------------------

#[test]
fn mem_error_taxonomy() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/data")).unwrap();

    // Missing paths.
    assert_kind(fs.read(Path::new("/data/nope")), VfsErrorKind::NotFound);
    assert_kind(fs.metadata(Path::new("/data/nope")), VfsErrorKind::NotFound);
    assert_kind(
        fs.truncate(Path::new("/data/nope"), 0),
        VfsErrorKind::NotFound,
    );
    assert_kind(
        fs.remove_file(Path::new("/data/nope")),
        VfsErrorKind::NotFound,
    );
    assert_kind(
        fs.remove_dir(Path::new("/data/nope")),
        VfsErrorKind::NotFound,
    );
    assert_kind(
        fs.rename(Path::new("/data/nope"), Path::new("/data/x")),
        VfsErrorKind::NotFound,
    );

    // Type mismatches.
    fs.create_dir(Path::new("/data/dir")).unwrap();
    fs.write(Path::new("/data/file"), b"x").unwrap();
    assert_kind(fs.read(Path::new("/data/dir")), VfsErrorKind::IsADirectory);
    assert_kind(
        fs.write(Path::new("/data/dir"), b"x"),
        VfsErrorKind::IsADirectory,
    );
    assert_kind(
        fs.remove_file(Path::new("/data/dir")),
        VfsErrorKind::IsADirectory,
    );
    assert_kind(
        fs.read_dir(Path::new("/data/file")),
        VfsErrorKind::NotADirectory,
    );
    assert_kind(
        fs.remove_dir(Path::new("/data/file")),
        VfsErrorKind::NotADirectory,
    );

    // Structural rules.
    assert_kind(
        fs.create_dir(Path::new("/data/dir")),
        VfsErrorKind::AlreadyExists,
    );
    fs.write(Path::new("/data/dir/child"), b"y").unwrap();
    assert_kind(
        fs.remove_dir(Path::new("/data/dir")),
        VfsErrorKind::DirectoryNotEmpty,
    );

    // Malformed input: the root/empty path names no entry.
    assert_kind(fs.read(Path::new("/")), VfsErrorKind::InvalidInput);
}

#[test]
fn os_error_edge_cases_surface_typed_errors() {
    let tmp = TempDir::new("os_error_edge_cases");
    let fs = OsFs::new();
    let root = tmp.path();

    // Cross-platform-stable categories.
    assert_kind(fs.read(&root.join("absent")), VfsErrorKind::NotFound);
    let d = root.join("d");
    fs.create_dir(&d).unwrap();
    assert_kind(fs.create_dir(&d), VfsErrorKind::AlreadyExists);

    // Categories the OS maps per-platform: assert only that a typed error is
    // surfaced (never a panic, never a silent success).
    let f = root.join("f");
    fs.write(&f, b"x").unwrap();
    assert!(fs.read_dir(&f).is_err());
    assert!(fs.remove_dir(&f).is_err());
    assert!(fs.remove_file(&d).is_err());
    let d2 = root.join("d2");
    fs.create_dir(&d2).unwrap();
    fs.write(&d2.join("c"), b"y").unwrap();
    assert!(fs.remove_dir(&d2).is_err());
    assert!(fs.rename(&root.join("missing"), &root.join("x")).is_err());
}

// ---------------------------------------------------------------------------
// Durability model -- MemFs write-vs-fsync ordering under simulated crash.
// ---------------------------------------------------------------------------

#[test]
fn mem_crash_loses_unsynced_writes() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d")).unwrap();
    fs.write(Path::new("/d/f"), b"never synced").unwrap();
    // Nothing was fsync'd, so a crash reverts to the (empty) durable state.
    fs.simulate_crash();
    assert!(!fs.exists(Path::new("/d/f")).unwrap());
}

#[test]
fn mem_fsynced_file_and_name_survive_crash() {
    let fs = MemFs::new();
    fs.create_dir(Path::new("/d")).unwrap();
    fs.fsync(Path::new("/d")).unwrap(); // make the directory name durable
    fs.write(Path::new("/d/f"), b"payload").unwrap();
    fs.fsync(Path::new("/d/f")).unwrap(); // make the file bytes durable
    fs.fsync(Path::new("/d")).unwrap(); // make the file's name durable
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/f")).unwrap(), b"payload");
}

#[test]
fn mem_fsynced_bytes_without_dir_fsync_lose_the_name() {
    let fs = MemFs::new();
    fs.create_dir(Path::new("/d")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.write(Path::new("/d/f"), b"payload").unwrap();
    fs.fsync(Path::new("/d/f")).unwrap(); // bytes durable, but the name is not
    fs.simulate_crash();
    // The containing directory was never fsync'd after the create, so the name
    // does not survive even though the bytes were flushed.
    assert!(!fs.exists(Path::new("/d/f")).unwrap());
}

#[test]
fn mem_syncing_a_directory_does_not_make_its_own_name_durable() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d/sub")).unwrap();
    fs.write(Path::new("/d/sub/f"), b"payload").unwrap();
    fs.fsync(Path::new("/d/sub/f")).unwrap();
    fs.fsync(Path::new("/d/sub")).unwrap(); // `sub`'s entries, not `sub` itself
    fs.simulate_crash();
    assert!(!fs.exists(Path::new("/d/sub")).unwrap());
}

#[test]
fn mem_syncing_the_parent_too_makes_a_new_directory_survive() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d/sub")).unwrap();
    fs.write(Path::new("/d/sub/f"), b"payload").unwrap();
    fs.fsync(Path::new("/d/sub/f")).unwrap();
    fs.fsync(Path::new("/d/sub")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/sub/f")).unwrap(), b"payload");
}

#[test]
fn mem_syncing_the_parent_first_still_needs_the_child_synced() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d/sub")).unwrap();
    fs.fsync(Path::new("/d")).unwrap(); // `sub`'s name is durable, its entries are not
    fs.write(Path::new("/d/sub/f"), b"payload").unwrap();
    fs.fsync(Path::new("/d/sub/f")).unwrap();
    fs.simulate_crash();
    assert!(fs.exists(Path::new("/d/sub")).unwrap());
    assert!(!fs.exists(Path::new("/d/sub/f")).unwrap());
}

#[test]
fn mem_an_unnamed_chain_is_lost_from_its_first_unsynced_link() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d/a/b")).unwrap();
    fs.write(Path::new("/d/a/b/f"), b"payload").unwrap();
    fs.fsync(Path::new("/d/a/b/f")).unwrap();
    fs.fsync(Path::new("/d/a/b")).unwrap();
    fs.fsync(Path::new("/d")).unwrap(); // names `a`, but `a`'s entry for `b` is not durable
    fs.simulate_crash();
    assert!(fs.exists(Path::new("/d/a")).unwrap());
    assert!(!fs.exists(Path::new("/d/a/b")).unwrap());
}

/// A durable directory `/d/x` holding a durable file `f`.
fn mem_with_durable_dir() -> MemFs {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d/x")).unwrap();
    fs.write(Path::new("/d/x/f"), b"payload").unwrap();
    fs.fsync(Path::new("/d/x/f")).unwrap();
    fs.fsync(Path::new("/d/x")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs
}

#[test]
fn mem_a_renamed_directory_keeps_its_durable_contents() {
    let fs = mem_with_durable_dir();
    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/y/f")).unwrap(), b"payload");
    assert!(!fs.exists(Path::new("/d/x")).unwrap());
}

#[test]
fn mem_a_directory_moved_across_parents_keeps_its_contents() {
    let fs = mem_with_durable_dir();
    fs.create_dir(Path::new("/d/other")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.rename(Path::new("/d/x"), Path::new("/d/other/x"))
        .unwrap();
    fs.fsync(Path::new("/d/other")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/other/x/f")).unwrap(), b"payload");
    assert!(!fs.exists(Path::new("/d/x")).unwrap());
}

#[test]
fn mem_a_moved_directory_survives_its_old_parent_being_synced_first() {
    let fs = mem_with_durable_dir();
    fs.create_dir(Path::new("/d/other")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.rename(Path::new("/d/x"), Path::new("/d/other/x"))
        .unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.fsync(Path::new("/d/other")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/other/x/f")).unwrap(), b"payload");
    assert!(!fs.exists(Path::new("/d/x")).unwrap());
}

#[test]
fn mem_a_directory_renamed_over_an_empty_one_brings_its_contents() {
    let fs = mem_with_durable_dir();
    fs.create_dir(Path::new("/d/y")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/y/f")).unwrap(), b"payload");
}

#[test]
fn mem_a_renamed_directory_synced_before_its_parent_keeps_old_and_new_contents() {
    let fs = mem_with_durable_dir();
    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    fs.write(Path::new("/d/y/g"), b"later").unwrap();
    fs.fsync(Path::new("/d/y/g")).unwrap();
    fs.fsync(Path::new("/d/y")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/y/f")).unwrap(), b"payload");
    assert_eq!(fs.read(Path::new("/d/y/g")).unwrap(), b"later");
}

#[test]
fn mem_a_refused_removal_does_not_drop_what_a_renamed_directory_carries() {
    let fs = mem_with_durable_dir();
    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    assert!(fs.remove_dir(Path::new("/d/y")).is_err(), "not empty");
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/y/f")).unwrap(), b"payload");
}

#[test]
fn mem_a_directory_moved_out_of_a_renamed_one_keeps_its_contents() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d/x/inner")).unwrap();
    fs.write(Path::new("/d/x/inner/f"), b"deep").unwrap();
    fs.fsync(Path::new("/d/x/inner/f")).unwrap();
    fs.fsync(Path::new("/d/x/inner")).unwrap();
    fs.fsync(Path::new("/d/x")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();

    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    fs.rename(Path::new("/d/y/inner"), Path::new("/d/z"))
        .unwrap();
    fs.fsync(Path::new("/d/y")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/z/f")).unwrap(), b"deep");
    assert!(fs.read_dir(Path::new("/d/y")).unwrap().is_empty());
}

#[test]
fn mem_a_recreated_directory_moved_out_of_a_renamed_one_does_not_inherit_old_contents() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d/x/inner")).unwrap();
    fs.write(Path::new("/d/x/inner/f"), b"deep").unwrap();
    fs.fsync(Path::new("/d/x/inner/f")).unwrap();
    fs.fsync(Path::new("/d/x/inner")).unwrap();
    fs.fsync(Path::new("/d/x")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();

    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    fs.remove_dir_all(Path::new("/d/y/inner")).unwrap();
    fs.create_dir(Path::new("/d/y/inner")).unwrap();
    fs.rename(Path::new("/d/y/inner"), Path::new("/d/z"))
        .unwrap();
    fs.fsync(Path::new("/d/y")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert!(fs.read_dir(Path::new("/d/z")).unwrap().is_empty());
}

#[test]
fn mem_reclaims_the_data_of_files_no_tree_can_reach() {
    // Repeated durable overwrites must not keep every historical version's bytes.
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d")).unwrap();
    for round in 0..50u32 {
        fs.write(Path::new("/d/f.tmp"), &round.to_le_bytes())
            .unwrap();
        fs.fsync(Path::new("/d/f.tmp")).unwrap();
        fs.rename(Path::new("/d/f.tmp"), Path::new("/d/f")).unwrap();
        fs.fsync(Path::new("/d")).unwrap();
    }
    assert_eq!(fs.held_file_data(), (1, 1));

    fs.remove_file(Path::new("/d/f")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    assert_eq!(fs.held_file_data(), (0, 0));

    fs.write(Path::new("/d/g"), b"volatile").unwrap();
    fs.simulate_crash();
    assert_eq!(fs.held_file_data(), (0, 0));
}

#[test]
fn mem_a_directory_recreated_at_its_own_path_starts_empty() {
    let fs = mem_with_durable_dir();
    fs.remove_dir_all(Path::new("/d/x")).unwrap();
    fs.create_dir(Path::new("/d/x")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert!(fs.read_dir(Path::new("/d/x")).unwrap().is_empty());
}

#[test]
fn mem_a_directory_renamed_away_and_replaced_does_not_inherit_its_contents() {
    let fs = mem_with_durable_dir();
    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    fs.create_dir(Path::new("/d/x")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert!(fs.read_dir(Path::new("/d/x")).unwrap().is_empty());
    assert_eq!(fs.read(Path::new("/d/y/f")).unwrap(), b"payload");
}

#[test]
fn mem_an_unsynced_directory_rename_is_undone_by_a_crash() {
    let fs = mem_with_durable_dir();
    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/x/f")).unwrap(), b"payload");
    assert!(!fs.exists(Path::new("/d/y")).unwrap());
}

#[test]
fn mem_a_nested_directory_moves_with_its_renamed_parent() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d/x/inner")).unwrap();
    fs.write(Path::new("/d/x/inner/f"), b"deep").unwrap();
    fs.fsync(Path::new("/d/x/inner/f")).unwrap();
    fs.fsync(Path::new("/d/x/inner")).unwrap();
    fs.fsync(Path::new("/d/x")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();

    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(Path::new("/d/y/inner/f")).unwrap(), b"deep");
}

#[test]
fn mem_a_directory_recreated_after_removal_does_not_inherit_old_contents() {
    let fs = mem_with_durable_dir();
    fs.rename(Path::new("/d/x"), Path::new("/d/y")).unwrap();
    fs.remove_dir_all(Path::new("/d/y")).unwrap();
    fs.create_dir(Path::new("/d/y")).unwrap();
    fs.fsync(Path::new("/d")).unwrap();
    fs.simulate_crash();
    assert!(fs.read_dir(Path::new("/d/y")).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Exclusive locking (`D116`) -- the contract both backends honour, each at its
// own scope. Cross-process behaviour is not testable here by construction; it is
// the store's `LOCK-4` suite that spawns a real second process.
// ---------------------------------------------------------------------------

#[test]
fn a_lock_is_granted_and_creates_its_file() {
    on_both("a_lock_is_granted_and_creates_its_file", |fs, root| {
        let lock_path = root.join(".lock");
        let held = fs.lock_exclusive(&lock_path).expect("first acquisition");
        assert!(
            fs.exists(&lock_path).unwrap(),
            "the lock names a file, and acquiring creates it"
        );
        drop(held);
    });
}

#[test]
fn a_second_acquisition_is_refused_while_the_first_is_held() {
    on_both(
        "a_second_acquisition_is_refused_while_the_first_is_held",
        |fs, root| {
            let lock_path = root.join(".lock");
            let held = fs.lock_exclusive(&lock_path).expect("first acquisition");

            let refused = fs
                .lock_exclusive(&lock_path)
                .expect_err("a held lock refuses a second owner");
            assert_eq!(refused.kind(), VfsErrorKind::Locked);

            drop(held);
        },
    );
}

#[test]
fn dropping_the_guard_releases_the_lock() {
    on_both("dropping_the_guard_releases_the_lock", |fs, root| {
        let lock_path = root.join(".lock");

        let first = fs.lock_exclusive(&lock_path).expect("first acquisition");
        drop(first);
        let second = fs
            .lock_exclusive(&lock_path)
            .expect("the lock is free once the guard is dropped");
        drop(second);
    });
}

/// The file outliving the lock is the point, not an oversight: a successor must
/// take the lock to learn anything, and must never read the file's presence as
/// "someone holds this" or its absence as "nobody does".
#[test]
fn the_lock_file_survives_release_and_means_nothing_on_its_own() {
    on_both(
        "the_lock_file_survives_release_and_means_nothing_on_its_own",
        |fs, root| {
            let lock_path = root.join(".lock");
            drop(fs.lock_exclusive(&lock_path).expect("acquire"));

            assert!(
                fs.exists(&lock_path).unwrap(),
                "the file remains after release"
            );
            drop(
                fs.lock_exclusive(&lock_path)
                    .expect("a leftover file does not wedge the lock"),
            );
        },
    );
}

/// Two locks in one filesystem are independent, so a store root's lock does not
/// exclude an unrelated root's.
#[test]
fn distinct_paths_lock_independently() {
    on_both("distinct_paths_lock_independently", |fs, root| {
        let first = fs.lock_exclusive(&root.join("a.lock")).expect("first");
        let second = fs.lock_exclusive(&root.join("b.lock")).expect("second");
        drop((first, second));
    });
}

/// Matching is on the normalized path, so a lock cannot be evaded by spelling
/// the same location differently.
#[test]
fn an_equivalent_spelling_names_the_same_lock() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d/sub")).unwrap();
    let held = fs.lock_exclusive(Path::new("/d/.lock")).expect("acquire");

    let refused = fs
        .lock_exclusive(Path::new("/d/sub/../.lock"))
        .expect_err("the same path spelled differently is the same lock");
    assert_eq!(refused.kind(), VfsErrorKind::Locked);

    drop(held);
}

/// Two separate `MemFs` instances are two separate filesystems, and the contract
/// is exclusion among openers of *one* filesystem -- so they do not exclude each
/// other. This is the mock's scope stated as a test rather than left implied.
#[test]
fn separate_mem_filesystems_do_not_exclude_each_other() {
    let first = MemFs::new();
    let second = MemFs::new();
    first.create_dir_all(Path::new("/d")).unwrap();
    second.create_dir_all(Path::new("/d")).unwrap();

    let a = first
        .lock_exclusive(Path::new("/d/.lock"))
        .expect("first fs");
    let b = second
        .lock_exclusive(Path::new("/d/.lock"))
        .expect("a different filesystem is not excluded by the first's lock");
    drop((a, b));
}

/// A backend that offers no exclusion must say so rather than pretend, so the
/// default is an error and never a silently granted no-op lock.
#[test]
fn the_default_implementation_reports_unsupported() {
    #[derive(Debug)]
    struct Bare;

    // Every operation is unreachable for this test; only the defaulted
    // `lock_exclusive` is exercised.
    impl Vfs for Bare {
        fn read(&self, _: &Path) -> VfsResult<Vec<u8>> {
            unimplemented!()
        }
        fn read_at(&self, _: &Path, _: u64, _: usize) -> VfsResult<Vec<u8>> {
            unimplemented!()
        }
        fn write(&self, _: &Path, _: &[u8]) -> VfsResult<()> {
            unimplemented!()
        }
        fn append(&self, _: &Path, _: &[u8]) -> VfsResult<()> {
            unimplemented!()
        }
        fn truncate(&self, _: &Path, _: u64) -> VfsResult<()> {
            unimplemented!()
        }
        fn fsync(&self, _: &Path) -> VfsResult<()> {
            unimplemented!()
        }
        fn rename(&self, _: &Path, _: &Path) -> VfsResult<()> {
            unimplemented!()
        }
        fn remove_file(&self, _: &Path) -> VfsResult<()> {
            unimplemented!()
        }
        fn create_dir(&self, _: &Path) -> VfsResult<()> {
            unimplemented!()
        }
        fn create_dir_all(&self, _: &Path) -> VfsResult<()> {
            unimplemented!()
        }
        fn remove_dir(&self, _: &Path) -> VfsResult<()> {
            unimplemented!()
        }
        fn remove_dir_all(&self, _: &Path) -> VfsResult<()> {
            unimplemented!()
        }
        fn read_dir(&self, _: &Path) -> VfsResult<Vec<crate::DirEntry>> {
            unimplemented!()
        }
        fn metadata(&self, _: &Path) -> VfsResult<crate::Metadata> {
            unimplemented!()
        }
    }

    let refused = Bare
        .lock_exclusive(Path::new("/d/.lock"))
        .expect_err("the default offers no exclusion");
    assert_eq!(refused.kind(), VfsErrorKind::Unsupported);
    let refused = Bare
        .try_lock(
            Path::new("/d/x.lock"),
            crate::LockMode::Shared,
            crate::LockSpan::File,
        )
        .expect_err("the default offers no locks");
    assert_eq!(refused.kind(), VfsErrorKind::Unsupported);
    let refused = Bare
        .lock(
            Path::new("/d/x.lock"),
            crate::LockMode::Exclusive,
            crate::LockSpan::File,
        )
        .expect_err("the default offers no waiting locks");
    assert_eq!(refused.kind(), VfsErrorKind::Unsupported);
    let refused = Bare
        .publish_new(Path::new("/d/x"), b"x")
        .expect_err("the default cannot publish");
    assert_eq!(refused.kind(), VfsErrorKind::Unsupported);
}

// ---------------------------------------------------------------------------
// `open_output`: the handle a child writes into (`EO-17`).
// ---------------------------------------------------------------------------

/// The handle is a real file the opener can write through, the file is created
/// where it was absent, and anything already there is discarded: the same open
/// policy a child's output channel has always had.
#[test]
fn open_output_creates_and_truncates_an_os_file_the_caller_writes_through() {
    use std::io::Write;

    let tmp = TempDir::new("open_output_truncates");
    let fs = OsFs::new();
    let path = tmp.path().join("stdout");

    let mut created = fs.open_output(&path).expect("created where absent");
    created.write_all(b"first run").unwrap();
    drop(created);
    assert_eq!(fs.read(&path).unwrap(), b"first run");

    let mut again = fs.open_output(&path).expect("reopened");
    assert_eq!(fs.metadata(&path).unwrap().len, 0, "truncated at open");
    again.write_all(b"second").unwrap();
    drop(again);
    assert_eq!(fs.read(&path).unwrap(), b"second");
}

/// A reader can open the file while the handle is live, because the output is
/// tailed as it is written.
#[test]
fn open_output_admits_a_concurrent_reader() {
    use std::io::Write;

    let tmp = TempDir::new("open_output_reader");
    let fs = OsFs::new();
    let path = tmp.path().join("stdout");
    let mut handle = fs.open_output(&path).unwrap();

    handle.write_all(b"live").unwrap();
    handle.flush().unwrap();

    assert_eq!(fs.read(&path).unwrap(), b"live");
}

#[test]
fn open_output_reports_what_went_wrong_as_a_typed_error() {
    let tmp = TempDir::new("open_output_errors");
    let fs = OsFs::new();

    let no_parent = fs
        .open_output(&tmp.path().join("absent").join("stdout"))
        .expect_err("the parent does not exist");
    assert_eq!(no_parent.kind(), VfsErrorKind::NotFound);
    assert_eq!(no_parent.op(), "open_output");

    let directory = fs
        .open_output(tmp.path())
        .expect_err("a directory is not an output file");
    assert!(
        matches!(
            directory.kind(),
            VfsErrorKind::IsADirectory | VfsErrorKind::PermissionDenied
        ),
        "{directory}"
    );
}

/// A backend with no operating-system files says so, so a caller that needs a
/// real handle learns it at the open and not by a child writing nowhere.
#[test]
fn the_memory_backend_cannot_hand_out_an_os_file_and_says_so() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d")).unwrap();

    let refused = fs
        .open_output(Path::new("/d/stdout"))
        .expect_err("memory has no OS handles");

    assert_eq!(refused.kind(), VfsErrorKind::Unsupported);
    assert!(
        !fs.exists(Path::new("/d/stdout")).unwrap(),
        "nothing created"
    );
}

/// A `VfsError` becomes the nearest standard `io::Error`, with itself as the
/// source, for callers whose own contract is `io::Result`.
#[test]
fn a_vfs_error_converts_to_the_nearest_io_error_and_is_kept_as_its_source() {
    use std::io::ErrorKind;

    for (kind, expected) in [
        (VfsErrorKind::NotFound, ErrorKind::NotFound),
        (VfsErrorKind::AlreadyExists, ErrorKind::AlreadyExists),
        (VfsErrorKind::NotADirectory, ErrorKind::NotADirectory),
        (VfsErrorKind::IsADirectory, ErrorKind::IsADirectory),
        (
            VfsErrorKind::DirectoryNotEmpty,
            ErrorKind::DirectoryNotEmpty,
        ),
        (VfsErrorKind::PermissionDenied, ErrorKind::PermissionDenied),
        (VfsErrorKind::InvalidInput, ErrorKind::InvalidInput),
        (VfsErrorKind::UnexpectedEof, ErrorKind::UnexpectedEof),
        (VfsErrorKind::WriteZero, ErrorKind::WriteZero),
        (VfsErrorKind::StorageFull, ErrorKind::StorageFull),
        (VfsErrorKind::Interrupted, ErrorKind::Interrupted),
        (VfsErrorKind::Unsupported, ErrorKind::Unsupported),
        (VfsErrorKind::Locked, ErrorKind::Other),
        (VfsErrorKind::Other, ErrorKind::Other),
    ] {
        let converted: std::io::Error = crate::VfsError::new(kind, "op", "message")
            .with_path("/p")
            .into();
        assert_eq!(converted.kind(), expected, "{kind:?}");
        let source = converted.get_ref().expect("the VfsError is kept");
        assert!(source.to_string().contains("message"), "{source}");
    }
}

// ---------------------------------------------------------------------------
// `read_at`: the read of a file still being written (`EO-18`), on both backends.
// ---------------------------------------------------------------------------

#[test]
fn read_at_returns_the_requested_slice() {
    on_both("read_at_slice", |fs, root| {
        let f = root.join("file.bin");
        fs.write(&f, b"0123456789").unwrap();

        assert_eq!(fs.read_at(&f, 0, 4).unwrap(), b"0123");
        assert_eq!(fs.read_at(&f, 3, 4).unwrap(), b"3456");
        assert_eq!(fs.read_at(&f, 9, 1).unwrap(), b"9");
        assert_eq!(fs.read_at(&f, 0, 10).unwrap(), b"0123456789");
    });
}

#[test]
fn read_at_stops_at_the_end_of_the_file_without_failing() {
    on_both("read_at_end", |fs, root| {
        let f = root.join("file.bin");
        fs.write(&f, b"0123456789").unwrap();

        assert_eq!(fs.read_at(&f, 6, 100).unwrap(), b"6789", "short at the end");
        assert_eq!(fs.read_at(&f, 0, usize::MAX).unwrap(), b"0123456789");
        assert!(fs.read_at(&f, 10, 5).unwrap().is_empty(), "at the end");
        assert!(fs.read_at(&f, 11, 5).unwrap().is_empty(), "just past it");
        assert!(
            fs.read_at(&f, u64::MAX, 5).unwrap().is_empty(),
            "far past it"
        );
        assert!(
            fs.read_at(&f, 3, 0).unwrap().is_empty(),
            "nothing asked for"
        );
    });
}

#[test]
fn read_at_of_an_empty_file_is_empty() {
    on_both("read_at_empty", |fs, root| {
        let f = root.join("file.bin");
        fs.write(&f, b"").unwrap();

        assert!(fs.read_at(&f, 0, 8).unwrap().is_empty());
    });
}

#[test]
fn read_at_sees_what_a_writer_has_appended_so_far() {
    on_both("read_at_growing", |fs, root| {
        let f = root.join("file.bin");
        fs.append(&f, b"abc").unwrap();
        assert_eq!(fs.read_at(&f, 0, 16).unwrap(), b"abc");

        fs.append(&f, b"defg").unwrap();

        assert_eq!(
            fs.read_at(&f, 3, 16).unwrap(),
            b"defg",
            "from where it left off"
        );
        assert_eq!(fs.read_at(&f, 0, 16).unwrap(), b"abcdefg");
    });
}

#[test]
fn read_at_does_not_hold_the_file_so_it_can_be_removed_and_replaced() {
    on_both("read_at_holds_nothing", |fs, root| {
        let f = root.join("file.bin");
        fs.write(&f, b"first").unwrap();
        assert_eq!(fs.read_at(&f, 0, 5).unwrap(), b"first");

        fs.remove_file(&f).expect("nothing is holding it");
        fs.write(&f, b"second").unwrap();

        assert_eq!(fs.read_at(&f, 0, 6).unwrap(), b"second");
    });
}

#[test]
fn read_at_reports_a_missing_file_and_a_directory_as_typed_errors() {
    on_both("read_at_errors", |fs, root| {
        let missing = fs.read_at(&root.join("absent"), 0, 1).unwrap_err();
        assert_eq!(missing.kind(), VfsErrorKind::NotFound);
        assert_eq!(missing.op(), "read_at");

        // Including the ranges a file would answer with nothing, which must not make a
        // directory look like an empty file.
        for (offset, len) in [(0, 1), (0, 0), (1 << 20, 1), (1 << 20, 0)] {
            let directory = fs.read_at(root, offset, len).unwrap_err();
            assert!(
                matches!(
                    directory.kind(),
                    VfsErrorKind::IsADirectory | VfsErrorKind::PermissionDenied
                ),
                "offset {offset} len {len}: {directory}"
            );
        }
    });
}

#[test]
fn read_at_agrees_with_read_over_every_slice_of_a_file() {
    on_both("read_at_agrees", |fs, root| {
        let f = root.join("file.bin");
        let contents: Vec<u8> = (0..=255u8).cycle().take(1_000).collect();
        fs.write(&f, &contents).unwrap();
        let whole = fs.read(&f).unwrap();

        for offset in [0usize, 1, 7, 255, 256, 511, 999, 1_000] {
            for len in [0usize, 1, 13, 256, 999, 1_000, 2_000] {
                let end = offset.saturating_add(len).min(whole.len());
                let expected = &whole[offset.min(whole.len())..end];
                assert_eq!(
                    fs.read_at(&f, offset as u64, len).unwrap(),
                    expected,
                    "offset {offset} len {len}"
                );
            }
        }
    });
}

#[test]
fn the_backends_say_which_is_the_operating_system() {
    assert!(OsFs::new().is_os_filesystem());
    assert!(!MemFs::new().is_os_filesystem());
}
