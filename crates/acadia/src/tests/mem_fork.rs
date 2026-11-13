// Copyright (c) Michael Grier.

//! `MemFs::fork` copies the whole state, durable and pending alike, and the copy is
//! independent of its original in both directions.

use super::*;

fn p(path: &str) -> &Path {
    Path::new(path)
}

/// `/d/kept` durable, `/d/pending` written and named but never synced.
fn half_durable() -> MemFs {
    let fs = MemFs::new();
    fs.create_dir(p("/d")).unwrap();
    fs.write(p("/d/kept"), b"durable").unwrap();
    fs.fsync(p("/d/kept")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.write(p("/d/pending"), b"pending").unwrap();
    fs
}

#[test]
fn mem_a_fork_reads_what_its_original_reads() {
    let fs = half_durable();
    let fork = fs.fork();
    assert_eq!(fork.read(p("/d/kept")).unwrap(), b"durable");
    assert_eq!(fork.read(p("/d/pending")).unwrap(), b"pending");
    assert_eq!(
        fork.read_dir(p("/d")).unwrap(),
        fs.read_dir(p("/d")).unwrap()
    );
    fork.audit_refs();
}

#[test]
fn mem_a_fork_keeps_the_durable_line_of_its_original() {
    let fork = half_durable().fork();
    fork.simulate_crash();
    assert_eq!(fork.read(p("/d/kept")).unwrap(), b"durable");
    assert!(!fork.exists(p("/d/pending")).unwrap());
    fork.audit_refs();
}

#[test]
fn mem_changes_to_a_fork_do_not_reach_its_original() {
    let fs = half_durable();
    let fork = fs.fork();
    fork.write(p("/d/kept"), b"changed").unwrap();
    fork.remove_file(p("/d/pending")).unwrap();
    fork.create_dir(p("/e")).unwrap();
    fork.fsync(p("/d")).unwrap();
    assert_eq!(fs.read(p("/d/kept")).unwrap(), b"durable");
    assert_eq!(fs.read(p("/d/pending")).unwrap(), b"pending");
    assert!(!fs.exists(p("/e")).unwrap());
    fs.audit_refs();
}

#[test]
fn mem_changes_to_the_original_do_not_reach_a_fork() {
    let fs = half_durable();
    let fork = fs.fork();
    fs.fsync(p("/d/pending")).unwrap();
    fs.fsync(p("/d")).unwrap();
    fs.remove_file(p("/d/kept")).unwrap();
    assert_eq!(fork.read(p("/d/kept")).unwrap(), b"durable");
    fork.simulate_crash();
    assert!(!fork.exists(p("/d/pending")).unwrap());
    fork.audit_refs();
}

#[test]
fn mem_a_crash_of_the_original_leaves_a_fork_whole() {
    let fs = half_durable();
    let fork = fs.fork();
    fs.simulate_crash();
    assert_eq!(fork.read(p("/d/pending")).unwrap(), b"pending");
}

#[test]
fn mem_a_fork_does_not_inherit_its_originals_locks() {
    let fs = MemFs::new();
    fs.create_dir(p("/d")).unwrap();
    let _held = fs.lock_exclusive(p("/d/lock")).unwrap();
    let fork = fs.fork();
    let taken = fork.lock_exclusive(p("/d/lock"));
    assert!(taken.is_ok(), "the fork's lock table starts empty");
    assert!(fs.lock_exclusive(p("/d/lock")).is_err());
}

#[test]
fn mem_forks_of_one_original_are_independent_of_each_other() {
    let fs = half_durable();
    let first = fs.fork();
    let second = fs.fork();
    first.write(p("/d/kept"), b"first").unwrap();
    second.write(p("/d/kept"), b"second").unwrap();
    assert_eq!(first.read(p("/d/kept")).unwrap(), b"first");
    assert_eq!(second.read(p("/d/kept")).unwrap(), b"second");
    assert_eq!(fs.read(p("/d/kept")).unwrap(), b"durable");
}

#[test]
fn mem_a_fork_continues_the_inode_sequence_without_collision() {
    let fs = half_durable();
    let fork = fs.fork();
    fork.write(p("/d/new"), b"new").unwrap();
    assert_eq!(fork.read(p("/d/kept")).unwrap(), b"durable");
    assert_eq!(fork.read(p("/d/new")).unwrap(), b"new");
    fork.audit_refs();
}

#[test]
fn mem_a_fork_carries_the_pending_change_records() {
    let fs = half_durable();
    let fork = fs.fork();
    fork.fsync(p("/d/pending")).unwrap();
    fork.fsync(p("/d")).unwrap();
    fork.simulate_crash();
    assert_eq!(fork.read(p("/d/pending")).unwrap(), b"pending");
}

#[test]
fn mem_a_fork_of_an_empty_filesystem_is_empty() {
    let fork = MemFs::new().fork();
    assert!(!fork.exists(p("/d")).unwrap());
    fork.audit_refs();
}
