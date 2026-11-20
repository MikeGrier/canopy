// Copyright (c) Michael Grier.
use super::*;
use crate::tests::on_both;
use crate::{Fault, FaultOp, FaultPolicy, FaultyFs, MemFs, When};

#[test]
fn canonical_csv_is_strict_and_reversible() {
    let rows = ["", "a,61\n", "a,61\nb,62\n"];
    for row in rows {
        let csv = format!("{HEADER}{row}");
        assert_eq!(encode(&decode(csv.as_bytes()).unwrap()), csv);
    }
    for row in [
        "\n",
        "a,61",
        "a,61\r\n",
        "a,61\n\n",
        "a,610\n",
        "a,6A\n",
        "a,ff\n",
        "a,c080\n",
        "a,62\n",
        "a,\"61\"\n",
        "a,61,\n",
        "a,61\na,61\n",
        "b,62\na,61\n",
        "../a,61\n",
    ] {
        assert!(
            decode(format!("{HEADER}{row}").as_bytes()).is_err(),
            "{row:?}"
        );
    }
    for invalid in [
        b"".as_slice(),
        b"acadia_names_v2,logical_utf8_hex\n",
        b"\xef\xbb\xbfacadia_names_v1,logical_utf8_hex\n",
    ] {
        assert!(decode(invalid).is_err());
    }
}

#[test]
fn bindings_survive_reopen_and_hide_unpublished_children() {
    on_both("names-reopen", |fs, parent| {
        let long = "\u{1f642}".repeat(200);
        let logicals = [
            "", ".store", ".STORE", "abc", "ABC", "../", "\\", "a:b", "nul", "\0\n,\"", "\u{e9}",
            "e\u{301}", "\u{c9}", MAP, STAGE, LOCK, &long,
        ];
        let mut dir = ComponentDirectory::open(fs, parent).unwrap();
        for logical in logicals {
            let path = dir.reserve(logical).unwrap();
            assert_eq!(path.parent(), Some(parent));
            assert!(dir.lookup(logical).unwrap().is_none());
            fs.publish_new(&path, logical.as_bytes()).unwrap();
        }
        dir.reserve("unfinished").unwrap();
        drop(dir);
        let mut dir = ComponentDirectory::open(fs, parent).unwrap();
        let mut listed = dir
            .list()
            .unwrap()
            .into_iter()
            .map(|child| child.file_name)
            .collect::<Vec<_>>();
        listed.sort();
        let mut expected = logicals.to_vec();
        expected.sort();
        assert_eq!(listed, expected);
        for logical in logicals {
            assert_eq!(
                fs.read(&dir.lookup(logical).unwrap().unwrap()).unwrap(),
                logical.as_bytes()
            );
            assert!(dir.remove(logical).unwrap());
        }
        assert!(dir.remove("unfinished").unwrap());
        assert!(!dir.remove("unfinished").unwrap());
        assert_eq!(fs.read(&parent.join(MAP)).unwrap(), HEADER.as_bytes());
    });
}

#[test]
fn refuses_to_claim_an_unowned_stage() {
    on_both("names-unowned-stage", |fs, parent| {
        let stage = parent.join(STAGE);
        fs.write(&stage, b"not an Acadia stage").unwrap();
        assert!(ComponentDirectory::open(fs, parent).is_err());
        assert_eq!(fs.read(&stage).unwrap(), b"not an Acadia stage");
        assert!(!fs.exists(&parent.join(MAP)).unwrap());
    });
}

#[test]
fn refuses_unowned_children_and_inconsistent_metadata() {
    on_both("names-ownership", |fs, parent| {
        let mut dir = ComponentDirectory::open(fs, parent).unwrap();
        for raw in ["abc", "ABC", &spelling("/")] {
            fs.write(&parent.join(raw), b"not owned").unwrap();
            let logical = if raw.starts_with(DIGEST_PREFIX) {
                "/"
            } else {
                "abc"
            };
            assert!(dir.reserve(logical).is_err());
            assert!(dir.lookup(logical).is_err());
            assert!(dir.remove(logical).is_err());
            assert_eq!(fs.read(&parent.join(raw)).unwrap(), b"not owned");
            fs.remove_file(&parent.join(raw)).unwrap();
        }
        fs.write(
            &parent.join(MAP),
            format!("{HEADER}abc,646566\n").as_bytes(),
        )
        .unwrap();
        assert!(dir.reserve("def").is_err());
        assert!(dir.list().is_err());
        drop(dir);
        assert!(ComponentDirectory::open(fs, parent).is_err());
        fs.remove_file(&parent.join(MAP)).unwrap();
        fs.create_dir(&parent.join(MAP)).unwrap();
        assert!(ComponentDirectory::open(fs, parent).is_err());
    });
}

#[test]
fn concurrent_openers_preserve_every_row() {
    on_both("names-concurrent", |fs, parent| {
        let barrier = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for logical in ["A", "a", "/", "\0"] {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    let mut dir = ComponentDirectory::open(fs, parent).unwrap();
                    let path = dir.reserve(logical).unwrap();
                    fs.publish_new(&path, logical.as_bytes()).unwrap();
                });
            }
        });
        let dir = ComponentDirectory::open(fs, parent).unwrap();
        assert_eq!(dir.list().unwrap().len(), 4);
        for name in ["A", "a", "/", "\0"] {
            assert_eq!(
                fs.read(&dir.lookup(name).unwrap().unwrap()).unwrap(),
                name.as_bytes()
            );
        }
    });
}

#[test]
fn retry_commits_binding_after_stage_or_parent_failure() {
    let parent = Path::new("/data");
    for (op, target, fault) in [
        (FaultOp::Write, parent.join(STAGE), Fault::short_write(3)),
        (FaultOp::Fsync, parent.join(STAGE), Fault::io_error()),
        (FaultOp::Fsync, parent.to_owned(), Fault::io_error()),
    ] {
        let mem = MemFs::new();
        mem.create_dir_all(parent).unwrap();
        mem.fsync(parent).unwrap();
        let fs = FaultyFs::new(
            mem,
            FaultPolicy::new().inject(op, &target, fault, When::Once),
        );
        let mut dir = ComponentDirectory::open(&fs, parent).unwrap();
        assert!(dir.reserve("A").is_err());
        assert!(dir.lookup("A").unwrap().is_none());
        let path = dir.reserve("A").unwrap();
        drop(dir);
        let restarted = fs.inner().simulate_restart();
        assert_eq!(
            decode(&restarted.read(&parent.join(MAP)).unwrap())
                .unwrap()
                .get(&spelling("A"))
                .unwrap(),
            "A"
        );
        assert!(!restarted.exists(&path).unwrap());
    }
}

#[test]
fn retry_flushes_a_visible_header_before_writing_a_stage() {
    let parent = Path::new("/data");
    let mem = MemFs::new();
    mem.create_dir_all(parent).unwrap();
    mem.fsync(parent).unwrap();
    let owner = ComponentDirectory::open(&mem, parent).unwrap();
    owner.claim_ownership().unwrap();
    drop(owner);
    let first = FaultyFs::new(
        mem.another_process(),
        FaultPolicy::new().inject(FaultOp::Fsync, parent, Fault::io_error(), When::OnCall(2)),
    );
    let mut directory = ComponentDirectory::open(&first, parent).unwrap();
    assert!(directory.reserve("A").is_err());
    drop(directory);
    assert_eq!(first.read(&parent.join(MAP)).unwrap(), HEADER.as_bytes());
    let second = FaultyFs::new(
        first.inner().another_process(),
        FaultPolicy::new().inject(FaultOp::Fsync, parent, Fault::io_error(), When::Always),
    );
    let mut directory = ComponentDirectory::open(&second, parent).unwrap();
    assert!(directory.reserve("A").is_err());
    drop(directory);
    let crashed = second.inner().fork().simulate_restart();
    assert!(!crashed.exists(&parent.join(MAP)).unwrap());
    let final_process = second.inner().another_process();
    let mut directory = ComponentDirectory::open(&final_process, parent).unwrap();
    directory.reserve("A").unwrap();
}

#[test]
fn equal_attempt_sequences_cannot_claim_the_same_directory() {
    let fs = MemFs::new();
    let parent = Path::new("/data");
    fs.create_dir_all(parent).unwrap();
    let target = parent.join("target");
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            barrier.wait();
            claim_artifact_attempt_at(&fs, parent, &target, 7).unwrap()
        });
        let right = scope.spawn(|| {
            barrier.wait();
            claim_artifact_attempt_at(&fs, parent, &target, 7).unwrap()
        });
        let claims = [left.join().unwrap(), right.join().unwrap()];
        assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    });
}

#[test]
fn portable_spelling_vectors() {
    for logical in [
        "abc",
        "-",
        "_",
        "a-b_c9",
        ".store",
        "0",
        "z",
        "com",
        "lpt10",
        "auxiliary",
        &"x".repeat(212),
    ] {
        assert_eq!(spelling(logical), logical);
    }
    let mut unique = std::collections::BTreeSet::new();
    for logical in [
        "",
        ".",
        "..",
        "con",
        "prn",
        "aux",
        "nul",
        "com0",
        "lpt9",
        "ABC",
        ".STORE",
        "a:b",
        "a/b",
        "a\\b",
        "\0",
        "\"\r\n,",
        "\u{c9}",
        "\u{e9}",
        "e\u{301}",
        &"x".repeat(213),
    ] {
        let physical = spelling(logical);
        assert_eq!(physical.strip_prefix(DIGEST_PREFIX).unwrap().len(), 64);
        assert!(unique.insert(physical));
    }
    assert_eq!(
        spelling(""),
        ".acadia-h1-e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn relative_native_parent_stays_fixed_after_working_directory_change() {
    const BASE: &str = "ACADIA_TEST_RELATIVE_NAME_BASE";
    let fs = crate::OsFs::new();
    if let Some(base) = std::env::var_os(BASE) {
        let base = Path::new(&base);
        let first = base.join("first");
        let second = base.join("second");
        std::env::set_current_dir(&first).unwrap();
        let mut directory = ComponentDirectory::open(&fs, Path::new("data")).unwrap();
        std::env::set_current_dir(&second).unwrap();
        let target = directory.reserve("entry").unwrap();
        fs.publish_new(&target, b"first root").unwrap();
        assert_eq!(target.parent().unwrap(), first.join("data"));
        return;
    }

    let temp = crate::tests::TempDir::new("names-relative-root");
    for side in ["first", "second"] {
        std::fs::create_dir_all(temp.path().join(side).join("data")).unwrap();
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "names::tests::relative_native_parent_stays_fixed_after_working_directory_change",
        ])
        .env(BASE, temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let first = ComponentDirectory::open(&fs, &temp.path().join("first/data")).unwrap();
    let second = ComponentDirectory::open(&fs, &temp.path().join("second/data")).unwrap();
    assert_eq!(
        fs.read(&first.lookup("entry").unwrap().unwrap()).unwrap(),
        b"first root"
    );
    assert!(second.lookup("entry").unwrap().is_none());
}

#[test]
fn native_processes_share_the_guard_and_binding_table() {
    const CHILD_PARENT: &str = "ACADIA_TEST_NAME_RESOLVER_PARENT";
    const CHILD_MODE: &str = "ACADIA_TEST_NAME_RESOLVER_MODE";
    let fs = crate::OsFs::new();
    if let Some(parent) = std::env::var_os(CHILD_PARENT) {
        let parent = Path::new(&parent);
        match std::env::var(CHILD_MODE).unwrap().as_str() {
            "held" => assert_eq!(
                fs.try_lock(&parent.join(LOCK), LockMode::Exclusive, LockSpan::File)
                    .unwrap_err()
                    .kind(),
                VfsErrorKind::Locked
            ),
            "publish" => {
                let mut dir = ComponentDirectory::open(&fs, parent).unwrap();
                assert!(dir.lookup("A").unwrap().is_some());
                fs.publish_new(&dir.reserve("a").unwrap(), b"second")
                    .unwrap();
            }
            _ => panic!("unknown helper mode"),
        }
        return;
    }
    let temp = crate::tests::TempDir::new("names-process");
    let child = |mode: &str| {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "names::tests::native_processes_share_the_guard_and_binding_table",
            ])
            .env(CHILD_PARENT, temp.path())
            .env(CHILD_MODE, mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    };
    let mut dir = ComponentDirectory::open(&fs, temp.path()).unwrap();
    fs.publish_new(&dir.reserve("A").unwrap(), b"first")
        .unwrap();
    child("held");
    drop(dir);
    child("publish");
    let dir = ComponentDirectory::open(&fs, temp.path()).unwrap();
    for (logical, expected) in [("A", "first"), ("a", "second")] {
        assert_eq!(
            fs.read(&dir.lookup(logical).unwrap().unwrap()).unwrap(),
            expected.as_bytes()
        );
    }
}

#[test]
fn failed_removal_flush_does_not_release_the_binding() {
    let parent = Path::new("/data");
    let mem = MemFs::new();
    mem.create_dir_all(parent).unwrap();
    let mut dir = ComponentDirectory::open(&mem, parent).unwrap();
    let child = dir.reserve("/").unwrap();
    mem.publish_new(&child, b"original").unwrap();
    mem.fsync(parent).unwrap();
    drop(dir);
    let fs = FaultyFs::new(
        mem,
        FaultPolicy::new().inject(FaultOp::Fsync, parent, Fault::io_error(), When::Once),
    );
    let mut dir = ComponentDirectory::open(&fs, parent).unwrap();
    assert!(dir.remove("/").is_err());
    drop(dir);
    let restarted = fs.inner().simulate_restart();
    let mut dir = ComponentDirectory::open(&restarted, parent).unwrap();
    assert_eq!(
        restarted.read(&dir.lookup("/").unwrap().unwrap()).unwrap(),
        b"original"
    );
    assert!(dir.remove("/").unwrap());
    drop(dir);
    let restarted = restarted.simulate_restart();
    assert_eq!(
        restarted.read(&parent.join(MAP)).unwrap(),
        HEADER.as_bytes()
    );
    assert!(!restarted.exists(&child).unwrap());
}

#[test]
fn removal_retry_flushes_the_replacement_map_before_reporting_absence() {
    let parent = Path::new("/data");
    let mem = MemFs::new();
    mem.create_dir_all(parent).unwrap();
    let mut dir = ComponentDirectory::open(&mem, parent).unwrap();
    let child = dir.reserve("removed").unwrap();
    mem.publish_new(&child, b"data").unwrap();
    mem.fsync(parent).unwrap();
    drop(dir);
    let first = FaultyFs::new(
        mem.another_process(),
        FaultPolicy::new().inject(FaultOp::Fsync, parent, Fault::io_error(), When::OnCall(4)),
    );
    let mut dir = ComponentDirectory::open(&first, parent).unwrap();
    assert!(dir.remove("removed").is_err());
    drop(dir);
    let second = FaultyFs::new(
        first.inner().another_process(),
        FaultPolicy::new().inject(FaultOp::Fsync, parent, Fault::io_error(), When::Always),
    );
    let mut dir = ComponentDirectory::open(&second, parent).unwrap();
    assert!(dir.remove("removed").is_err());
    drop(dir);
    let final_process = second.inner().another_process();
    let mut dir = ComponentDirectory::open(&final_process, parent).unwrap();
    assert!(!dir.remove("removed").unwrap());
    drop(dir);
    let restarted = final_process.simulate_restart();
    assert_eq!(
        decode(&restarted.read(&parent.join(MAP)).unwrap())
            .unwrap()
            .len(),
        0
    );
    assert!(!restarted.exists(&child).unwrap());
}
