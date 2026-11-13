// Copyright (c) Michael Grier.

//! `try_lock`: shared and exclusive locks on a file or a byte range of one, taken without
//! waiting, over both backends; and `MemFs`'s simulated processes and their abandonment.
//! What a real process's death does is the cross-process suite's, which spawns one.

use super::*;
use crate::{LockMode, LockSpan};

const SHARED: LockMode = LockMode::Shared;
const EXCLUSIVE: LockMode = LockMode::Exclusive;

fn range(offset: u64, len: u64) -> LockSpan {
    LockSpan::Range { offset, len }
}

/// Whether `fs` refuses a seam read or write inside another holder's lock: Windows does and
/// `MemFs` does as Windows does, while Linux locks are advisory.
fn refuses_locked_bytes(fs: &dyn Vfs) -> bool {
    !fs.is_os_filesystem() || cfg!(windows)
}

#[test]
fn a_lock_creates_its_file_and_keeps_what_is_already_there() {
    on_both("lock-creates", |fs, root| {
        let fresh = root.join("fresh.lock");
        let held = fs.try_lock(&fresh, EXCLUSIVE, LockSpan::File).unwrap();
        assert_eq!(fs.read_at(&fresh, 0, 1).unwrap(), b"");
        drop(held);

        let data = root.join("data");
        fs.write(&data, b"kept").unwrap();
        let held = fs.try_lock(&data, SHARED, range(1 << 40, 1)).unwrap();
        assert_eq!(fs.read(&data).unwrap(), b"kept", "never truncated");
        drop(held);
    });
}

#[test]
fn two_guards_in_one_process_are_two_holders() {
    on_both("lock-two-guards", |fs, root| {
        let path = root.join("one.lock");
        let held = fs.try_lock(&path, EXCLUSIVE, LockSpan::File).unwrap();
        assert_kind(
            fs.try_lock(&path, EXCLUSIVE, LockSpan::File),
            VfsErrorKind::Locked,
        );
        assert_kind(
            fs.try_lock(&path, SHARED, LockSpan::File),
            VfsErrorKind::Locked,
        );
        drop(held);
    });
}

#[test]
fn shared_holders_coexist_and_exclude_an_exclusive_one() {
    on_both("lock-shared", |fs, root| {
        let path = root.join("shared.lock");
        let first = fs.try_lock(&path, SHARED, LockSpan::File).unwrap();
        let second = fs.try_lock(&path, SHARED, LockSpan::File).unwrap();
        assert_kind(
            fs.try_lock(&path, EXCLUSIVE, LockSpan::File),
            VfsErrorKind::Locked,
        );
        drop(first);
        assert_kind(
            fs.try_lock(&path, EXCLUSIVE, LockSpan::File),
            VfsErrorKind::Locked,
        );
        drop(second);
        drop(fs.try_lock(&path, EXCLUSIVE, LockSpan::File).unwrap());
    });
}

#[test]
fn ranges_conflict_only_where_they_overlap() {
    on_both("lock-ranges", |fs, root| {
        let path = root.join("ranges");
        let low = fs.try_lock(&path, EXCLUSIVE, range(0, 10)).unwrap();
        let adjacent = fs.try_lock(&path, EXCLUSIVE, range(10, 10)).unwrap();
        let far = fs.try_lock(&path, EXCLUSIVE, range(1 << 40, 4)).unwrap();
        assert_kind(
            fs.try_lock(&path, SHARED, range(9, 2)),
            VfsErrorKind::Locked,
        );
        assert_kind(
            fs.try_lock(&path, EXCLUSIVE, range(15, 1)),
            VfsErrorKind::Locked,
        );
        let between = fs.try_lock(&path, SHARED, range(20, 100)).unwrap();
        drop((low, adjacent, far, between));
    });
}

#[test]
fn a_whole_file_lock_overlaps_every_range() {
    on_both("lock-whole", |fs, root| {
        let path = root.join("whole.lock");
        let ranged = fs
            .try_lock(&path, SHARED, range(LockSpan::MAX_END - 1, 1))
            .unwrap();
        assert_kind(
            fs.try_lock(&path, EXCLUSIVE, LockSpan::File),
            VfsErrorKind::Locked,
        );
        let shared_file = fs.try_lock(&path, SHARED, LockSpan::File).unwrap();
        assert_kind(
            fs.try_lock(&path, EXCLUSIVE, range(0, 1)),
            VfsErrorKind::Locked,
        );
        drop((ranged, shared_file));
    });
}

#[test]
fn dropping_a_guard_releases_only_its_own_lock() {
    on_both("lock-drop", |fs, root| {
        let path = root.join("drop.lock");
        let first = fs.try_lock(&path, SHARED, range(0, 8)).unwrap();
        let second = fs.try_lock(&path, SHARED, range(0, 8)).unwrap();
        drop(first);
        assert_kind(
            fs.try_lock(&path, EXCLUSIVE, range(0, 8)),
            VfsErrorKind::Locked,
        );
        drop(second);
        drop(fs.try_lock(&path, EXCLUSIVE, range(0, 8)).unwrap());
    });
}

#[test]
fn locks_on_different_files_are_independent() {
    on_both("lock-files", |fs, root| {
        let a = fs
            .try_lock(&root.join("a"), EXCLUSIVE, LockSpan::File)
            .unwrap();
        let b = fs
            .try_lock(&root.join("b"), EXCLUSIVE, LockSpan::File)
            .unwrap();
        drop((a, b));
    });
}

#[test]
fn an_empty_or_oversized_range_is_invalid_input() {
    on_both("lock-invalid", |fs, root| {
        let path = root.join("invalid.lock");
        for span in [
            range(0, 0),
            range(LockSpan::MAX_END, 1),
            range(LockSpan::MAX_END - 1, 2),
            range(u64::MAX, 1),
        ] {
            assert_kind(fs.try_lock(&path, SHARED, span), VfsErrorKind::InvalidInput);
        }
        drop(
            fs.try_lock(&path, SHARED, range(LockSpan::MAX_END - 1, 1))
                .unwrap(),
        );
    });
}

#[test]
fn a_directory_cannot_be_locked() {
    on_mem(&|fs, root| {
        let dir = root.join("dir");
        fs.create_dir(&dir).unwrap();
        assert_kind(
            fs.try_lock(&dir, SHARED, LockSpan::File),
            VfsErrorKind::IsADirectory,
        );
    });
}

#[test]
fn data_outside_every_locked_range_reads_and_writes_freely() {
    on_both("lock-apart", |fs, root| {
        let path = root.join("apart");
        fs.write(&path, b"0123456789").unwrap();
        let held = fs.try_lock(&path, EXCLUSIVE, range(1 << 40, 1)).unwrap();
        assert_eq!(fs.read(&path).unwrap(), b"0123456789");
        assert_eq!(fs.read_at(&path, 2, 3).unwrap(), b"234");
        fs.append(&path, b"ab").unwrap();
        fs.write(&path, b"replaced").unwrap();
        fs.truncate(&path, 4).unwrap();
        assert_eq!(fs.read(&path).unwrap(), b"repl");
        drop(held);
    });
}

#[test]
fn an_exclusive_range_refuses_reads_where_the_platform_does() {
    on_both("lock-refuse-read", |fs, root| {
        let path = root.join("refused");
        fs.write(&path, b"0123456789").unwrap();
        let held = fs.try_lock(&path, EXCLUSIVE, range(4, 2)).unwrap();
        if refuses_locked_bytes(fs) {
            assert_kind(fs.read(&path), VfsErrorKind::Locked);
            assert_kind(fs.read_at(&path, 3, 2), VfsErrorKind::Locked);
        } else {
            assert_eq!(fs.read(&path).unwrap(), b"0123456789");
        }
        assert_eq!(fs.read_at(&path, 0, 4).unwrap(), b"0123");
        assert_eq!(fs.read_at(&path, 6, 4).unwrap(), b"6789");
        drop(held);
        assert_eq!(fs.read(&path).unwrap(), b"0123456789");
    });
}

#[test]
fn a_locked_range_refuses_writes_into_it_where_the_platform_does() {
    for mode in [SHARED, EXCLUSIVE] {
        on_both("lock-refuse-write", |fs, root| {
            let path = root.join("growing");
            fs.write(&path, b"0123456789").unwrap();
            let held = fs.try_lock(&path, mode, range(10, 10)).unwrap();
            if refuses_locked_bytes(fs) {
                assert_kind(fs.append(&path, b"!"), VfsErrorKind::Locked);
                assert_eq!(fs.read_at(&path, 0, 100).unwrap(), b"0123456789");
            } else {
                fs.append(&path, b"!").unwrap();
            }
            drop(held);
            fs.append(&path, b"?").unwrap();
        });
    }
}

#[test]
fn a_shared_range_admits_reads() {
    on_both("lock-shared-read", |fs, root| {
        let path = root.join("shared-data");
        fs.write(&path, b"0123456789").unwrap();
        let held = fs.try_lock(&path, SHARED, range(0, 10)).unwrap();
        assert_eq!(fs.read(&path).unwrap(), b"0123456789");
        assert_eq!(fs.read_at(&path, 5, 5).unwrap(), b"56789");
        drop(held);
    });
}

#[test]
fn the_memory_backend_refuses_a_truncation_across_a_locked_range() {
    on_mem(&|fs, root| {
        let path = root.join("cut");
        fs.write(&path, b"0123456789").unwrap();
        let held = fs.try_lock(&path, SHARED, range(6, 2)).unwrap();
        assert_kind(fs.truncate(&path, 4), VfsErrorKind::Locked);
        assert_kind(fs.write(&path, b"short"), VfsErrorKind::Locked);
        fs.truncate(&path, 9).unwrap();
        drop(held);
        fs.truncate(&path, 4).unwrap();
        assert_eq!(fs.read(&path).unwrap(), b"0123");
    });
}

// -- Simulated processes and their death ------------------------------------------------------

#[test]
fn another_process_shares_the_tree_and_contends_for_its_locks() {
    let first = MemFs::new();
    first.create_dir_all(Path::new("/d")).unwrap();
    let second = first.another_process();
    second.write(Path::new("/d/f"), b"seen").unwrap();
    assert_eq!(first.read(Path::new("/d/f")).unwrap(), b"seen");

    let held = first
        .try_lock(Path::new("/d/f.lock"), EXCLUSIVE, LockSpan::File)
        .unwrap();
    assert_kind(
        second.try_lock(Path::new("/d/f.lock"), SHARED, LockSpan::File),
        VfsErrorKind::Locked,
    );
    drop(held);
    drop(
        second
            .try_lock(Path::new("/d/f.lock"), SHARED, LockSpan::File)
            .unwrap(),
    );
}

#[test]
fn abandoning_a_process_releases_its_locks_without_its_guards() {
    let survivor = MemFs::new();
    survivor.create_dir_all(Path::new("/d")).unwrap();
    let dying = survivor.another_process();
    let lock = Path::new("/d/x.lock");
    let root_lock = Path::new("/d/.lock");
    let guard = dying.try_lock(lock, EXCLUSIVE, range(0, 4)).unwrap();
    let root_guard = dying.lock_exclusive(root_lock).unwrap();

    dying.abandon();

    let taken = survivor.try_lock(lock, EXCLUSIVE, range(0, 4)).unwrap();
    let root_taken = survivor.lock_exclusive(root_lock).unwrap();
    // The dead process's guards release nothing: the locks are the survivor's now.
    drop((guard, root_guard));
    assert_kind(
        dying.try_lock(lock, SHARED, range(0, 4)),
        VfsErrorKind::Locked,
    );
    assert_kind(dying.lock_exclusive(root_lock), VfsErrorKind::Locked);
    drop((taken, root_taken));
}

#[test]
fn abandoning_one_process_leaves_every_other_process_its_locks() {
    let first = MemFs::new();
    first.create_dir_all(Path::new("/d")).unwrap();
    let second = first.another_process();
    let third = first.another_process();
    let lock = Path::new("/d/shared.lock");
    let kept = first.try_lock(lock, SHARED, LockSpan::File).unwrap();
    let lost = second.try_lock(lock, SHARED, LockSpan::File).unwrap();

    second.abandon();

    assert_kind(
        third.try_lock(lock, EXCLUSIVE, LockSpan::File),
        VfsErrorKind::Locked,
    );
    drop(kept);
    drop(third.try_lock(lock, EXCLUSIVE, LockSpan::File).unwrap());
    drop(lost);
}

#[test]
fn a_crash_loses_storage_and_leaves_the_locks_held() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d")).unwrap();
    let held = fs
        .try_lock(Path::new("/d/x.lock"), EXCLUSIVE, LockSpan::File)
        .unwrap();
    fs.simulate_crash();
    assert_kind(
        fs.try_lock(Path::new("/d/x.lock"), EXCLUSIVE, LockSpan::File),
        VfsErrorKind::Locked,
    );
    drop(held);
}

#[test]
fn a_fork_starts_with_no_locks() {
    let fs = MemFs::new();
    fs.create_dir_all(Path::new("/d")).unwrap();
    let held = fs
        .try_lock(Path::new("/d/x.lock"), EXCLUSIVE, LockSpan::File)
        .unwrap();
    let copy = fs.fork();
    drop(
        copy.try_lock(Path::new("/d/x.lock"), EXCLUSIVE, LockSpan::File)
            .unwrap(),
    );
    drop(held);
}

// -- Faults -----------------------------------------------------------------------------------

#[test]
fn a_lock_can_be_made_to_fail_and_is_otherwise_forwarded() {
    let lock = Path::new("/d/x.lock");
    let faulty = FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject(
            FaultOp::TryLock,
            lock,
            Fault::error(VfsErrorKind::PermissionDenied),
            When::Once,
        ),
    );
    faulty.create_dir_all(Path::new("/d")).unwrap();
    assert_kind(
        faulty.try_lock(lock, SHARED, LockSpan::File),
        VfsErrorKind::PermissionDenied,
    );
    assert!(
        !faulty.exists(lock).unwrap(),
        "a failed lock creates nothing"
    );
    let held = faulty.try_lock(lock, EXCLUSIVE, LockSpan::File).unwrap();
    assert_kind(
        faulty.try_lock(lock, SHARED, LockSpan::File),
        VfsErrorKind::Locked,
    );
    faulty.inner().abandon();
    drop(faulty.try_lock(lock, SHARED, LockSpan::File).unwrap());
    drop(held);
}
