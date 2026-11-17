// Copyright (c) Michael Grier.

//! Replay harness for the finite MemFs durability model. Every action is one model step;
//! after it runs, the MemFs reference counts are audited, and the final projection checks the
//! live tree after the scenario's crash against the state the TLA+ scenario promises.

use std::fmt;

use super::*;

#[derive(Clone)]
enum Action {
    CreateDir(&'static str),
    Write(&'static str, &'static [u8]),
    RemoveFile(&'static str),
    RemoveDir(&'static str),
    RemoveDirAll(&'static str),
    Rename(&'static str, &'static str),
    Fsync(&'static str),
    Crash,
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::CreateDir(path) => write!(formatter, "create-dir {path}"),
            Action::Write(path, _) => write!(formatter, "write {path}"),
            Action::RemoveFile(path) => write!(formatter, "remove-file {path}"),
            Action::RemoveDir(path) => write!(formatter, "remove-dir {path}"),
            Action::RemoveDirAll(path) => write!(formatter, "remove-dir-all {path}"),
            Action::Rename(from, to) => write!(formatter, "rename {from} -> {to}"),
            Action::Fsync(path) => write!(formatter, "fsync {path}"),
            Action::Crash => formatter.write_str("crash"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Expected<'a> {
    Missing(&'a str),
    File(&'a str, &'a [u8]),
    Dir(&'a str),
}

fn replay(name: &str, actions: &[Action], expected: &[Expected<'_>]) {
    let fs = MemFs::new();
    for (step, action) in actions.iter().enumerate() {
        let result = match action {
            Action::CreateDir(path) => fs.create_dir(Path::new(path)),
            Action::Write(path, contents) => fs.write(Path::new(path), contents),
            Action::RemoveFile(path) => fs.remove_file(Path::new(path)),
            Action::RemoveDir(path) => fs.remove_dir(Path::new(path)),
            Action::RemoveDirAll(path) => fs.remove_dir_all(Path::new(path)),
            Action::Rename(from, to) => fs.rename(Path::new(from), Path::new(to)),
            Action::Fsync(path) => fs.fsync(Path::new(path)),
            Action::Crash => {
                fs.simulate_crash();
                Ok(())
            }
        };
        assert!(
            result.is_ok(),
            "{name} step {step} ({action}) failed: {result:?}"
        );
        fs.audit_refs();
    }
    for projection in expected {
        match projection {
            Expected::Missing(path) => assert!(
                !fs.exists(Path::new(path)).unwrap(),
                "{name}: {path} survived unexpectedly"
            ),
            Expected::File(path, contents) => assert_eq!(
                fs.read(Path::new(path)).unwrap(),
                *contents,
                "{name}: wrong contents at {path}"
            ),
            Expected::Dir(path) => assert_eq!(
                fs.metadata(Path::new(path)).unwrap().file_type,
                FileType::Dir,
                "{name}: {path} is not a directory"
            ),
        }
    }
}

#[test]
fn mem_model_double_name_after_a_one_parent_sync() {
    let actions = [
        Action::CreateDir("/d"),
        Action::Fsync("/d"),
        Action::CreateDir("/d/l"),
        Action::CreateDir("/d/r"),
        Action::Fsync("/d"),
        Action::Write("/d/l/f", b"A"),
        Action::Fsync("/d/l/f"),
        Action::Fsync("/d/l"),
        Action::Rename("/d/l/f", "/d/r/f"),
        Action::Fsync("/d/r"),
        Action::Crash,
    ];
    let fs = MemFs::new();
    for action in actions {
        match action {
            Action::CreateDir(path) => fs.create_dir(Path::new(path)).unwrap(),
            Action::Write(path, bytes) => fs.write(Path::new(path), bytes).unwrap(),
            Action::Rename(from, to) => fs.rename(Path::new(from), Path::new(to)).unwrap(),
            Action::Fsync(path) => fs.fsync(Path::new(path)).unwrap(),
            Action::Crash => fs.simulate_crash(),
            Action::RemoveFile(_) | Action::RemoveDir(_) | Action::RemoveDirAll(_) => {
                unreachable!()
            }
        }
        fs.audit_refs();
    }
    let (left_live, left_durable) = fs.file_inodes(Path::new("/d/l/f")).unwrap();
    let (right_live, right_durable) = fs.file_inodes(Path::new("/d/r/f")).unwrap();
    assert_eq!(left_live, right_live);
    assert_eq!(left_durable, right_durable);
    assert_eq!(fs.read(Path::new("/d/l/f")).unwrap(), b"A");
    assert_eq!(fs.read(Path::new("/d/r/f")).unwrap(), b"A");
}

#[test]
fn mem_model_carried_snapshots_and_unnamed_children() {
    replay(
        "durable file replaced by directory",
        &[
            Action::CreateDir("/d"),
            Action::Fsync("/d"),
            Action::Write("/d/x", b"old"),
            Action::Fsync("/d/x"),
            Action::Fsync("/d"),
            Action::RemoveFile("/d/x"),
            Action::CreateDir("/d/x"),
            Action::Write("/d/x/y", b"B"),
            Action::Fsync("/d/x/y"),
            Action::Fsync("/d/x"),
            Action::Fsync("/d"),
            Action::Crash,
        ],
        &[Expected::Dir("/d/x"), Expected::File("/d/x/y", b"B")],
    );

    replay(
        "moved directory carries durable descendants",
        &[
            Action::CreateDir("/d"),
            Action::Fsync("/d"),
            Action::CreateDir("/d/src"),
            Action::CreateDir("/d/src/sub"),
            Action::Write("/d/src/sub/f", b"A"),
            Action::Fsync("/d/src/sub/f"),
            Action::Fsync("/d/src/sub"),
            Action::Fsync("/d/src"),
            Action::Fsync("/d"),
            Action::Rename("/d/src", "/d/x"),
            Action::Fsync("/d/x"),
            Action::Fsync("/d"),
            Action::Crash,
        ],
        &[
            Expected::Missing("/d/src"),
            Expected::Dir("/d/x"),
            Expected::File("/d/x/sub/f", b"A"),
        ],
    );

    replay(
        "nested directory remains unnamed until its own parent syncs",
        &[
            Action::CreateDir("/d"),
            Action::Fsync("/d"),
            Action::CreateDir("/d/x"),
            Action::CreateDir("/d/x/y"),
            Action::Write("/d/x/y/f", b"A"),
            Action::Fsync("/d/x/y/f"),
            Action::Fsync("/d/x/y"),
            Action::Fsync("/d"),
            Action::Crash,
        ],
        &[
            Expected::Dir("/d/x"),
            Expected::Missing("/d/x/y"),
            Expected::Missing("/d/x/y/f"),
        ],
    );
}

#[test]
fn mem_model_normal_write_sync_and_removal_traces() {
    let cases = [
        (
            "unsynced file and name",
            vec![
                Action::CreateDir("/d"),
                Action::Write("/d/f", b"x"),
                Action::Crash,
            ],
            vec![Expected::Missing("/d"), Expected::Missing("/d/f")],
        ),
        (
            "file data without durable name",
            vec![
                Action::CreateDir("/d"),
                Action::Fsync("/d"),
                Action::Write("/d/f", b"x"),
                Action::Fsync("/d/f"),
                Action::Crash,
            ],
            vec![Expected::Missing("/d/f")],
        ),
        (
            "file data and name",
            vec![
                Action::CreateDir("/d"),
                Action::Fsync("/d"),
                Action::Write("/d/f", b"x"),
                Action::Fsync("/d/f"),
                Action::Fsync("/d"),
                Action::Crash,
            ],
            vec![Expected::File("/d/f", b"x")],
        ),
        (
            "directory entries do not name the directory",
            vec![
                Action::CreateDir("/d"),
                Action::Fsync("/d"),
                Action::CreateDir("/d/sub"),
                Action::Write("/d/sub/f", b"x"),
                Action::Fsync("/d/sub/f"),
                Action::Fsync("/d/sub"),
                Action::Crash,
            ],
            vec![Expected::Dir("/d"), Expected::Missing("/d/sub")],
        ),
        (
            "parent sync names directory and its previously synced entries",
            vec![
                Action::CreateDir("/d"),
                Action::Fsync("/d"),
                Action::CreateDir("/d/sub"),
                Action::Write("/d/sub/f", b"x"),
                Action::Fsync("/d/sub/f"),
                Action::Fsync("/d/sub"),
                Action::Fsync("/d"),
                Action::Crash,
            ],
            vec![Expected::File("/d/sub/f", b"x")],
        ),
        (
            "removed name disappears after its parent sync",
            vec![
                Action::CreateDir("/d"),
                Action::Fsync("/d"),
                Action::Write("/d/f", b"x"),
                Action::Fsync("/d/f"),
                Action::Fsync("/d"),
                Action::RemoveFile("/d/f"),
                Action::Fsync("/d"),
                Action::Crash,
            ],
            vec![Expected::Missing("/d/f")],
        ),
    ];
    for (name, actions, expected) in cases {
        replay(name, &actions, &expected);
    }
}

#[test]
fn mem_model_directory_removal_traces() {
    let durable_tree = [
        Action::CreateDir("/d"),
        Action::Fsync("/d"),
        Action::CreateDir("/d/x"),
        Action::CreateDir("/d/x/y"),
        Action::Write("/d/x/y/f", b"A"),
        Action::Fsync("/d/x/y/f"),
        Action::Fsync("/d/x/y"),
        Action::Fsync("/d/x"),
        Action::Fsync("/d"),
        Action::RemoveDirAll("/d/x"),
    ];
    let moved_tree = [
        Action::CreateDir("/d"),
        Action::Fsync("/d"),
        Action::CreateDir("/d/src"),
        Action::CreateDir("/d/src/sub"),
        Action::Write("/d/src/sub/f", b"A"),
        Action::Fsync("/d/src/sub/f"),
        Action::Fsync("/d/src/sub"),
        Action::Fsync("/d/src"),
        Action::Fsync("/d"),
        Action::Rename("/d/src", "/d/x"),
    ];
    let recreated_empty = [
        Expected::Dir("/d/x"),
        Expected::Missing("/d/x/sub"),
        Expected::Missing("/d/x/sub/f"),
        Expected::Missing("/d/src"),
    ];

    replay(
        "unsynced recursive removal is undone by a crash",
        &[&durable_tree[..], &[Action::Crash]].concat(),
        &[
            Expected::Dir("/d/x"),
            Expected::Dir("/d/x/y"),
            Expected::File("/d/x/y/f", b"A"),
        ],
    );

    replay(
        "recursive removal survives a crash after its parent sync",
        &[&durable_tree[..], &[Action::Fsync("/d"), Action::Crash]].concat(),
        &[
            Expected::Dir("/d"),
            Expected::Missing("/d/x"),
            Expected::Missing("/d/x/y"),
            Expected::Missing("/d/x/y/f"),
        ],
    );

    replay(
        "directory recreated after recursive removal starts empty",
        &[
            &moved_tree[..],
            &[
                Action::RemoveDirAll("/d/x"),
                Action::CreateDir("/d/x"),
                Action::Fsync("/d"),
                Action::Crash,
            ],
        ]
        .concat(),
        &recreated_empty,
    );

    replay(
        "directory recreated after emptying and removing it starts empty",
        &[
            &moved_tree[..],
            &[
                Action::RemoveFile("/d/x/sub/f"),
                Action::RemoveDir("/d/x/sub"),
                Action::RemoveDir("/d/x"),
                Action::CreateDir("/d/x"),
                Action::Fsync("/d"),
                Action::Crash,
            ],
        ]
        .concat(),
        &recreated_empty,
    );
}

#[test]
fn mem_model_moved_directory_keeps_its_durable_descendant() {
    replay(
        "a moved directory's durable descendant survives syncing it and then the parent",
        &[
            Action::CreateDir("/d"),
            Action::Fsync("/d"),
            Action::CreateDir("/d/src"),
            Action::CreateDir("/d/src/sub"),
            Action::Write("/d/src/sub/f", b"A"),
            Action::Fsync("/d/src/sub/f"),
            Action::Fsync("/d/src/sub"),
            Action::Fsync("/d/src"),
            Action::Fsync("/d"),
            Action::Rename("/d/src", "/d/x"),
            Action::Fsync("/d/x/sub"),
            Action::Fsync("/d"),
            Action::Crash,
        ],
        &[
            Expected::Missing("/d/src"),
            Expected::Dir("/d/x"),
            Expected::Dir("/d/x/sub"),
            Expected::File("/d/x/sub/f", b"A"),
        ],
    );
}

#[test]
fn mem_model_moved_directory_does_not_name_its_unnamed_child() {
    replay(
        "a child never named in a moved directory stays unnamed after the move",
        &[
            Action::CreateDir("/d"),
            Action::Fsync("/d"),
            Action::CreateDir("/d/src"),
            Action::Fsync("/d/src"),
            Action::Fsync("/d"),
            Action::CreateDir("/d/src/sub"),
            Action::Write("/d/src/sub/f", b"A"),
            Action::Fsync("/d/src/sub/f"),
            Action::Fsync("/d/src/sub"),
            Action::Rename("/d/src", "/d/x"),
            Action::Fsync("/d"),
            Action::Crash,
        ],
        &[
            Expected::Missing("/d/src"),
            Expected::Dir("/d/x"),
            Expected::Missing("/d/x/sub"),
            Expected::Missing("/d/x/sub/f"),
        ],
    );
}
