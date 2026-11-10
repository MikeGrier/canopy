// Copyright (c) Michael Grier.

//! MemFs bookkeeping across renames, replacement and reclamation: a replaced rename
//! destination keeps none of its predecessor's durable history, and every file's bytes are
//! released by the operation that makes them unreachable, at a cost bounded by what that
//! operation discarded. [`MemFs::audit_refs`] recounts from scratch after each step, so a
//! drift in the incremental counts is caught where it happens rather than at a crash.

use super::*;

fn p(path: &str) -> &Path {
    Path::new(path)
}

/// `/d/x/f` written and fully durable.
fn durable_file_in_dir() -> MemFs {
    let fs = MemFs::new();
    fs.create_dir_all(p("/d/x")).unwrap();
    fs.write(p("/d/x/f"), b"old").unwrap();
    fs.fsync(p("/d/x/f")).unwrap();
    fs.fsync(p("/d/x")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.audit_refs();
    fs
}

#[test]
fn mem_a_directory_renamed_over_another_does_not_inherit_what_that_one_carried() {
    // Sync orders matter to which snapshot is materialized when, so every order is covered.
    for sync_y_first in [false, true] {
        let fs = durable_file_in_dir();
        fs.rename(p("/d/x"), p("/d/y")).unwrap();
        fs.remove_file(p("/d/y/f")).unwrap();
        fs.create_dir(p("/d/z")).unwrap();
        fs.rename(p("/d/z"), p("/d/y")).unwrap();
        fs.audit_refs();
        if sync_y_first {
            fs.fsync(p("/d/y")).unwrap();
        }
        fs.fsync(p("/d")).unwrap();
        fs.audit_refs();
        fs.simulate_crash();
        fs.audit_refs();
        assert!(
            fs.read_dir(p("/d/y")).unwrap().is_empty(),
            "sync_y_first={sync_y_first}: the replacement resurrected its predecessor's file"
        );
        assert!(!fs.exists(p("/d/x")).unwrap());
        assert!(!fs.exists(p("/d/z")).unwrap());
    }
}

#[test]
fn mem_a_directory_renamed_over_another_keeps_what_it_carries_itself() {
    let fs = durable_file_in_dir();
    fs.create_dir(p("/d/y")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.rename(p("/d/x"), p("/d/y")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.audit_refs();
    fs.simulate_crash();
    fs.audit_refs();
    assert_eq!(fs.read(p("/d/y/f")).unwrap(), b"old");
}

#[test]
fn mem_renaming_a_file_over_another_releases_the_one_replaced() {
    let fs = MemFs::new();
    fs.create_dir_all(p("/d")).unwrap();
    fs.write(p("/d/a"), b"a").unwrap();
    fs.write(p("/d/b"), b"b").unwrap();
    fs.rename(p("/d/a"), p("/d/b")).unwrap();
    fs.audit_refs();
    assert_eq!(fs.held_file_data(), (1, 0));
    assert_eq!(fs.read(p("/d/b")).unwrap(), b"a");
}

#[test]
fn mem_recreating_a_synced_directory_reclaims_its_old_contents_without_a_crash() {
    let fs = MemFs::new();
    fs.create_dir_all(p("/d/x")).unwrap();
    for round in 0..20u32 {
        for file in 0..3u32 {
            let path = PathBuf::from(format!("/d/x/f{file}"));
            fs.write(&path, &round.to_le_bytes()).unwrap();
            fs.fsync(&path).unwrap();
        }
        fs.fsync(p("/d/x")).unwrap();
        fs.fsync(p("/d")).unwrap();
        assert_eq!(fs.held_file_data(), (3, 3), "round {round}");

        fs.remove_dir_all(p("/d/x")).unwrap();
        fs.create_dir(p("/d/x")).unwrap();
        fs.fsync(p("/d")).unwrap();
        fs.audit_refs();
        assert_eq!(
            fs.held_file_data(),
            (0, 0),
            "round {round}: the replaced directory's durable files are still held"
        );
    }
}

#[test]
fn mem_a_renamed_directory_materialized_over_an_old_one_releases_the_old_one() {
    // The rename leaves the old durable `y` at its path until `d` is synced; materializing
    // the carried `x` over it is the step that makes the old contents unreachable.
    let fs = durable_file_in_dir();
    fs.create_dir(p("/d/y")).unwrap();
    fs.write(p("/d/y/g"), b"gone").unwrap();
    fs.fsync(p("/d/y/g")).unwrap();
    fs.fsync(p("/d/y")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.remove_file(p("/d/y/g")).unwrap();
    fs.rename(p("/d/x"), p("/d/y")).unwrap();
    fs.fsync(p("/d/y")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.audit_refs();
    assert_eq!(fs.held_file_data(), (1, 1));
    fs.simulate_crash();
    assert_eq!(fs.read(p("/d/y/f")).unwrap(), b"old");
    assert!(!fs.exists(p("/d/y/g")).unwrap());
}

#[test]
fn mem_counts_survive_every_mutation_and_a_crash() {
    // One scripted walk through every operation that changes a tree, audited at each step.
    let fs = MemFs::new();
    type Step = Box<dyn Fn(&MemFs)>;
    let steps: Vec<Step> = vec![
        Box::new(|fs| fs.create_dir_all(p("/r/a/b")).unwrap()),
        Box::new(|fs| fs.write(p("/r/a/b/f"), b"1").unwrap()),
        Box::new(|fs| fs.append(p("/r/a/g"), b"2").unwrap()),
        Box::new(|fs| fs.fsync(p("/r/a/b/f")).unwrap()),
        Box::new(|fs| fs.fsync(p("/r/a/b")).unwrap()),
        Box::new(|fs| fs.fsync(p("/r/a")).unwrap()),
        Box::new(|fs| fs.fsync(p("/r")).unwrap()),
        Box::new(|fs| fs.rename(p("/r/a"), p("/r/c")).unwrap()),
        Box::new(|fs| fs.rename(p("/r/c/b"), p("/r/e")).unwrap()),
        Box::new(|fs| fs.write(p("/r/c/g"), b"3").unwrap()),
        Box::new(|fs| fs.rename(p("/r/c/g"), p("/r/e/f")).unwrap()),
        Box::new(|fs| fs.fsync(p("/r/e")).unwrap()),
        Box::new(|fs| fs.remove_dir(p("/r/c")).unwrap()),
        Box::new(|fs| fs.fsync(p("/r")).unwrap()),
        Box::new(|fs| fs.truncate(p("/r/e/f"), 0).unwrap()),
        Box::new(|fs| fs.remove_dir_all(p("/r/e")).unwrap()),
        Box::new(|fs| fs.write(p("/r/h"), b"4").unwrap()),
        Box::new(MemFs::simulate_crash),
        Box::new(|fs| fs.fsync(p("/r")).unwrap()),
    ];
    for (index, step) in steps.iter().enumerate() {
        step(&fs);
        let audit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fs.audit_refs()));
        assert!(audit.is_ok(), "the counts drifted at step {index}");
    }
}

#[test]
fn mem_an_overwrite_does_not_walk_unrelated_directories() {
    // Reclaiming what one sync discarded must cost what it discarded, not the size of the
    // filesystem: a store with many documents rewrites one at a time.
    let fs = MemFs::new();
    fs.create_dir_all(p("/big")).unwrap();
    fs.create_dir_all(p("/d")).unwrap();
    for file in 0..500u32 {
        let path = PathBuf::from(format!("/big/f{file}"));
        fs.write(&path, b"x").unwrap();
        fs.fsync(&path).unwrap();
    }
    fs.fsync(p("/big")).unwrap();

    let rounds = 50u64;
    let before = crate::mem::nodes_walked();
    for round in 0..rounds {
        fs.write(p("/d/f.tmp"), &round.to_le_bytes()).unwrap();
        fs.fsync(p("/d/f.tmp")).unwrap();
        fs.rename(p("/d/f.tmp"), p("/d/f")).unwrap();
        fs.fsync(p("/d")).unwrap();
    }
    let walked = crate::mem::nodes_walked() - before;
    assert!(
        walked <= rounds * 4,
        "{walked} nodes walked for {rounds} single-file overwrites"
    );
    assert_eq!(fs.held_file_data(), (501, 501));
    fs.audit_refs();
}

/// `/left/f` renamed to `/right/f`, only the destination synced, then a crash: the durable
/// tree kept the old name and gained the new one, so both now name one file.
fn crash_restored_alias() -> MemFs {
    let fs = MemFs::new();
    fs.create_dir_all(p("/left")).unwrap();
    fs.create_dir_all(p("/right")).unwrap();
    fs.write(p("/left/f"), b"payload").unwrap();
    fs.fsync(p("/left/f")).unwrap();
    fs.fsync(p("/left")).unwrap();
    fs.fsync(p("/right")).unwrap();
    fs.rename(p("/left/f"), p("/right/f")).unwrap();
    fs.fsync(p("/right")).unwrap();
    fs.simulate_crash();
    fs.audit_refs();
    assert_eq!(fs.read(p("/left/f")).unwrap(), b"payload");
    assert_eq!(fs.read(p("/right/f")).unwrap(), b"payload");
    fs
}

#[test]
fn mem_removing_one_name_of_a_crash_restored_alias_keeps_the_other_readable() {
    let fs = crash_restored_alias();
    fs.remove_file(p("/left/f")).unwrap();
    fs.audit_refs();
    assert_eq!(fs.read(p("/right/f")).unwrap(), b"payload");
    fs.fsync(p("/right/f")).unwrap();
    fs.fsync(p("/left")).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(p("/right/f")).unwrap(), b"payload");
    assert!(!fs.exists(p("/left/f")).unwrap());
}

#[test]
fn mem_renaming_over_one_name_of_an_alias_keeps_the_other_readable() {
    let fs = crash_restored_alias();
    fs.write(p("/left/g"), b"other").unwrap();
    fs.rename(p("/left/g"), p("/left/f")).unwrap();
    fs.audit_refs();
    assert_eq!(fs.read(p("/left/f")).unwrap(), b"other");
    assert_eq!(fs.read(p("/right/f")).unwrap(), b"payload");
}

#[test]
fn mem_removing_a_tree_holding_one_name_of_an_alias_keeps_the_other_readable() {
    let fs = crash_restored_alias();
    fs.remove_dir_all(p("/left")).unwrap();
    fs.audit_refs();
    assert_eq!(fs.read(p("/right/f")).unwrap(), b"payload");
}

#[test]
fn mem_an_alias_is_reclaimed_once_every_name_is_gone() {
    let fs = crash_restored_alias();
    fs.remove_file(p("/left/f")).unwrap();
    fs.remove_file(p("/right/f")).unwrap();
    fs.fsync(p("/left")).unwrap();
    fs.fsync(p("/right")).unwrap();
    fs.audit_refs();
    assert_eq!(fs.held_file_data(), (0, 0));
}

/// `/d/x` a durable file, replaced in the live tree by a directory holding `/d/x/f`.
fn file_replaced_by_directory() -> MemFs {
    let fs = MemFs::new();
    fs.create_dir_all(p("/d")).unwrap();
    fs.write(p("/d/x"), b"old file").unwrap();
    fs.fsync(p("/d/x")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.remove_file(p("/d/x")).unwrap();
    fs.create_dir(p("/d/x")).unwrap();
    fs.write(p("/d/x/f"), b"new").unwrap();
    fs.fsync(p("/d/x/f")).unwrap();
    fs
}

#[test]
fn mem_a_directory_replacing_a_file_can_be_synced_before_its_parent() {
    let fs = file_replaced_by_directory();
    fs.fsync(p("/d/x"))
        .expect("syncing the new directory first is valid");
    fs.audit_refs();
    fs.fsync(p("/d")).unwrap();
    fs.audit_refs();
    fs.simulate_crash();
    fs.audit_refs();
    assert_eq!(fs.read(p("/d/x/f")).unwrap(), b"new");
    assert_eq!(
        fs.held_file_data(),
        (1, 1),
        "the replaced file's bytes were released"
    );
}

#[test]
fn mem_a_directory_replacing_a_file_is_lost_to_a_crash_before_its_parent_sync() {
    // Only the parent's sync makes the name change durable; until then the file survives.
    let fs = file_replaced_by_directory();
    fs.fsync(p("/d/x")).unwrap();
    fs.simulate_crash();
    fs.audit_refs();
    assert_eq!(fs.read(p("/d/x")).unwrap(), b"old file");
}

#[test]
fn mem_a_directory_replacing_a_file_syncs_in_either_order() {
    let fs = file_replaced_by_directory();
    fs.fsync(p("/d")).unwrap();
    fs.fsync(p("/d/x")).unwrap();
    fs.simulate_crash();
    fs.audit_refs();
    assert_eq!(fs.read(p("/d/x/f")).unwrap(), b"new");
}

#[test]
fn mem_a_nested_directory_under_a_replaced_file_syncs_bottom_up() {
    let fs = MemFs::new();
    fs.create_dir_all(p("/d")).unwrap();
    fs.write(p("/d/x"), b"old file").unwrap();
    fs.fsync(p("/d/x")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.remove_file(p("/d/x")).unwrap();
    fs.create_dir_all(p("/d/x/y")).unwrap();
    fs.write(p("/d/x/y/f"), b"deep").unwrap();
    fs.fsync(p("/d/x/y/f")).unwrap();
    fs.fsync(p("/d/x/y")).unwrap();
    fs.fsync(p("/d/x")).unwrap();
    fs.audit_refs();
    fs.fsync(p("/d")).unwrap();
    fs.simulate_crash();
    fs.audit_refs();
    assert_eq!(fs.read(p("/d/x/y/f")).unwrap(), b"deep");
}

/// `/d/x` a durable file, and `/d/src/sub/f` a durable tree.
fn durable_file_and_tree() -> MemFs {
    let fs = MemFs::new();
    fs.create_dir_all(p("/d/src/sub")).unwrap();
    fs.write(p("/d/x"), b"old file").unwrap();
    fs.write(p("/d/src/sub/f"), b"moved").unwrap();
    for synced in ["/d/x", "/d/src/sub/f", "/d/src/sub", "/d/src", "/d"] {
        fs.fsync(p(synced)).unwrap();
    }
    fs.audit_refs();
    fs
}

#[test]
fn mem_a_moved_directory_synced_over_a_file_keeps_its_durable_contents() {
    // The rename carries `src`'s durable tree to `x`; syncing `x` before `d` must build on
    // that, not reset it because `x` was also marked replaced by the same rename.
    for sync_x_first in [true, false] {
        let fs = durable_file_and_tree();
        fs.remove_file(p("/d/x")).unwrap();
        fs.rename(p("/d/src"), p("/d/x")).unwrap();
        if sync_x_first {
            fs.fsync(p("/d/x")).unwrap();
            fs.audit_refs();
        }
        fs.fsync(p("/d")).unwrap();
        fs.audit_refs();
        fs.simulate_crash();
        fs.audit_refs();
        assert_eq!(
            fs.read(p("/d/x/sub/f")).unwrap(),
            b"moved",
            "sync_x_first={sync_x_first}"
        );
        assert!(!fs.exists(p("/d/src")).unwrap());
    }
}

#[test]
fn mem_a_file_replaced_inside_a_carried_directory_does_not_lose_what_was_carried() {
    // `x` replaced a durable file and was synced, so its snapshot holds file `a`; then `a`
    // is replaced by a directory that receives a moved durable tree. Syncing `a` must neither
    // fail on the old file inside the snapshot nor drop either snapshot.
    let fs = durable_file_and_tree();
    fs.remove_file(p("/d/x")).unwrap();
    fs.create_dir(p("/d/x")).unwrap();
    fs.write(p("/d/x/a"), b"inner file").unwrap();
    fs.fsync(p("/d/x/a")).unwrap();
    fs.fsync(p("/d/x")).unwrap();
    fs.remove_file(p("/d/x/a")).unwrap();
    fs.create_dir(p("/d/x/a")).unwrap();
    fs.rename(p("/d/src"), p("/d/x/a/b")).unwrap();

    fs.fsync(p("/d/x/a"))
        .expect("a directory over a carried file syncs");
    fs.audit_refs();
    fs.fsync(p("/d/x")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.audit_refs();
    fs.simulate_crash();
    fs.audit_refs();
    assert_eq!(fs.read(p("/d/x/a/b/sub/f")).unwrap(), b"moved");
}

#[test]
fn mem_a_directory_under_a_carried_one_is_lost_if_its_own_parent_was_never_synced() {
    // `y`'s name lives in `x`; syncing `y` and `d` but never `x` leaves it unnamed.
    let fs = durable_file_and_tree();
    fs.remove_file(p("/d/x")).unwrap();
    fs.create_dir_all(p("/d/x/y")).unwrap();
    fs.write(p("/d/x/y/f"), b"deep").unwrap();
    fs.fsync(p("/d/x/y/f")).unwrap();
    fs.fsync(p("/d/x/y")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.simulate_crash();
    fs.audit_refs();
    assert!(
        fs.read_dir(p("/d/x")).unwrap().is_empty(),
        "`x` was named by syncing `d`, but `y` was never named in `x`"
    );
}

#[test]
fn mem_a_directory_under_a_carried_one_survives_once_its_parent_is_synced() {
    let fs = durable_file_and_tree();
    fs.remove_file(p("/d/x")).unwrap();
    fs.create_dir_all(p("/d/x/y")).unwrap();
    fs.write(p("/d/x/y/f"), b"deep").unwrap();
    fs.fsync(p("/d/x/y/f")).unwrap();
    fs.fsync(p("/d/x/y")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.fsync(p("/d/x")).unwrap();
    fs.simulate_crash();
    fs.audit_refs();
    assert_eq!(fs.read(p("/d/x/y/f")).unwrap(), b"deep");
}
