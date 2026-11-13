// Copyright (c) Michael Grier.

//! `publish_new`: a file created only where nothing is, never visible incomplete, and complete
//! or absent after a power loss.

use super::*;

/// The names in `dir`, sorted, so a test can see a staging sibling left behind.
fn names(fs: &dyn Vfs, dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs
        .read_dir(dir)
        .unwrap()
        .into_iter()
        .map(|entry| entry.file_name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_publication_creates_the_file_with_its_contents() {
    on_both("publish-creates", |fs, root| {
        let path = root.join("reservation");
        fs.publish_new(&path, b"the whole record").unwrap();
        assert_eq!(fs.read(&path).unwrap(), b"the whole record");
        assert_eq!(
            names(fs, root),
            ["reservation"],
            "no staging sibling is left"
        );
    });
}

#[test]
fn an_empty_publication_is_still_a_file() {
    on_both("publish-empty", |fs, root| {
        let path = root.join("empty");
        fs.publish_new(&path, b"").unwrap();
        assert!(fs.metadata(&path).unwrap().is_file());
        assert_eq!(fs.read(&path).unwrap(), b"");
    });
}

#[test]
fn a_file_already_there_refuses_the_publication_and_is_unchanged() {
    on_both("publish-exists", |fs, root| {
        let path = root.join("taken");
        fs.publish_new(&path, b"first").unwrap();
        assert_kind(
            fs.publish_new(&path, b"second"),
            VfsErrorKind::AlreadyExists,
        );
        fs.write(&root.join("written"), b"plain").unwrap();
        assert_kind(
            fs.publish_new(&root.join("written"), b"over"),
            VfsErrorKind::AlreadyExists,
        );
        assert_eq!(fs.read(&path).unwrap(), b"first");
        assert_eq!(fs.read(&root.join("written")).unwrap(), b"plain");
        assert_eq!(names(fs, root), ["taken", "written"]);
    });
}

#[test]
fn a_directory_there_refuses_the_publication() {
    on_both("publish-dir", |fs, root| {
        let dir = root.join("dir");
        fs.create_dir(&dir).unwrap();
        assert_kind(fs.publish_new(&dir, b"x"), VfsErrorKind::AlreadyExists);
        assert!(fs.metadata(&dir).unwrap().is_dir());
        assert_eq!(names(fs, root), ["dir"]);
    });
}

#[test]
fn a_missing_parent_refuses_the_publication() {
    on_both("publish-parent", |fs, root| {
        assert_kind(
            fs.publish_new(&root.join("absent").join("file"), b"x"),
            VfsErrorKind::NotFound,
        );
        assert!(names(fs, root).is_empty());
    });
}

#[test]
fn publications_at_different_paths_are_independent() {
    on_both("publish-many", |fs, root| {
        for i in 0..20u8 {
            fs.publish_new(&root.join(format!("f{i}")), &[i; 3])
                .unwrap();
        }
        for i in 0..20u8 {
            assert_eq!(fs.read(&root.join(format!("f{i}"))).unwrap(), [i; 3]);
        }
        assert_eq!(names(fs, root).len(), 20);
    });
}

#[test]
fn of_racing_publishers_exactly_one_wins() {
    let tmp = TempDir::new("publish-race");
    let fs = Arc::new(OsFs::new());
    let path = tmp.path().join("contended");
    let outcomes: Vec<VfsResult<()>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8u8)
            .map(|i| {
                let (fs, path) = (Arc::clone(&fs), path.clone());
                scope.spawn(move || fs.publish_new(&path, &[i; 64]))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let winners: Vec<usize> = (0..outcomes.len())
        .filter(|&i| outcomes[i].is_ok())
        .collect();
    assert_eq!(winners.len(), 1, "{outcomes:?}");
    for (i, outcome) in outcomes.iter().enumerate() {
        if i != winners[0] {
            assert_eq!(
                outcome.as_ref().unwrap_err().kind(),
                VfsErrorKind::AlreadyExists
            );
        }
    }
    assert_eq!(fs.read(&path).unwrap(), [winners[0] as u8; 64]);
    assert_eq!(names(&*fs, tmp.path()), ["contended"]);
}

#[test]
fn a_crash_before_the_parent_is_synced_leaves_no_file() {
    let fs = MemFs::new();
    let root = Path::new("/r");
    fs.create_dir_all(root).unwrap();
    fs.fsync(root).unwrap();
    fs.publish_new(&root.join("record"), b"complete").unwrap();
    fs.simulate_crash();
    assert!(!fs.exists(&root.join("record")).unwrap());
}

#[test]
fn a_crash_after_the_parent_is_synced_leaves_the_whole_file() {
    // A plain write needs its own fsync before the name's; a publication's bytes are durable
    // before its name exists, so syncing the name is enough.
    let fs = MemFs::new();
    let root = Path::new("/r");
    fs.create_dir_all(root).unwrap();
    fs.publish_new(&root.join("record"), b"complete").unwrap();
    fs.fsync(root).unwrap();
    fs.simulate_crash();
    assert_eq!(fs.read(&root.join("record")).unwrap(), b"complete");
    fs.audit_refs();
}

#[test]
fn a_publication_can_be_made_to_fail_and_publishes_nothing() {
    let path = Path::new("/d/record");
    let faulty = FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject(FaultOp::PublishNew, path, Fault::io_error(), When::Once),
    );
    faulty.create_dir_all(Path::new("/d")).unwrap();
    assert_kind(faulty.publish_new(path, b"x"), VfsErrorKind::Other);
    assert!(!faulty.exists(path).unwrap());
    faulty.publish_new(path, b"y").unwrap();
    assert_eq!(faulty.read(path).unwrap(), b"y");
}
