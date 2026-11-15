// Copyright (c) Michael Grier.

use super::*;
use canopy::{DiagnosticView, Swap};

const WRITERS: usize = 4;
const WRITES_PER_PROCESS: usize = 64;
const DEBT_FILE: &str = ".dir-sync-debt";
const CLAIMS_DIRECTORY: &str = ".claims";
const LOG_FILE: &str = ".commit-log";

fn session(root: &Path, prefix: &str, content: &str, version: &str) -> Writer {
    let writer = Writer::spawn_version(root, "docs", prefix, content, "session", version);
    writer.expect("opened");
    writer
}

fn assert_document(store: &Store, directory: &str, name: &str, content: &str) {
    assert_eq!(
        store
            .read_document(node(directory), &document(name))
            .unwrap(),
        Read::Found(body(content))
    );
}

fn require_recovery(writer: &mut Writer) {
    writer.command("write");
    writer.expect("recovery-required");
    writer.command("recover");
    writer.expect("recovered");
}

#[test]
fn processes_interleave_logged_and_unlogged_writes_without_losing_documents() {
    let root = TempRoot::new("turns-at-scale");
    seed_store(root.path());
    let reader = open_reader(root.path());
    let count = reader.modifications().unwrap();
    let payload = "payload".repeat(128);
    let mut writers: Vec<_> = (0..WRITERS)
        .map(|index| session(root.path(), &format!("writer{index}"), &payload, "test"))
        .collect();
    for sequence in 0..WRITES_PER_PROCESS {
        for (index, writer) in writers.iter_mut().enumerate() {
            writer.command(if (sequence + index) % 2 == 0 {
                "transaction"
            } else {
                "write"
            });
        }
        for writer in &writers {
            writer.expect(&format!("wrote:{sequence}"));
        }
    }
    assert_eq!(
        reader.modifications().unwrap(),
        count + 2 * (WRITERS * WRITES_PER_PROCESS) as u64
    );
    for index in 0..WRITERS {
        for sequence in 0..WRITES_PER_PROCESS {
            let name = format!("writer{index}-{sequence}");
            assert_document(&reader, "docs", &name, &payload);
            if (sequence + index) % 2 == 0 {
                assert_document(&reader, "docs", &format!("{name}-peer"), &payload);
            }
        }
    }
    for writer in writers {
        writer.finish();
    }
}

#[test]
fn death_at_each_writer_boundary_refuses_survivors_until_explicit_recovery() {
    for mode in [
        "pause-before",
        "pause-publication",
        "pause-transaction",
        "pause-after",
    ] {
        let root = TempRoot::new(mode);
        seed_store(root.path());
        let (first, _) = Store::open(vfs(), root.path(), nonce_source("first")).unwrap();
        let (second, _) = Store::open(vfs(), root.path(), nonce_source("second")).unwrap();
        first
            .write_document_replicated(node("alpha"), &document("record"), &body("old"))
            .unwrap()
            .agent_stamp
            .unwrap();
        let diagnostic = DiagnosticView::open(vfs(), root.path()).unwrap();
        let writer = Writer::spawn_mode(root.path(), "alpha", "record", "new", mode);
        writer.expect("opened");
        writer.expect("paused");
        if matches!(mode, "pause-publication" | "pause-transaction") {
            let pointer = |index| {
                root.path()
                    .join("namespace")
                    .join(format!("p{index}"))
                    .join(node("alpha").relative_path())
                    .join(document("record").as_str())
            };
            assert_ne!(
                vfs().read(&pointer(0)).unwrap(),
                vfs().read(&pointer(1)).unwrap(),
                "one replica moved before the checkpoint"
            );
        }
        let process = writer.child.id();
        assert!(!diagnostic.recovery_need().unwrap().holder_died());
        assert_eq!(diagnostic.recovery_need().unwrap().unresolved_work(), None);
        writer.kill();
        assert!(diagnostic.recovery_need().unwrap().holder_died());
        assert!(
            first
                .attached_handles()
                .unwrap()
                .live
                .iter()
                .all(|entry| entry.process_id != process)
        );
        for survivor in [&first, &second] {
            assert!(matches!(
                survivor.write_document_replicated(
                    node("alpha"),
                    &document("later"),
                    &body("refused")
                ),
                Err(StoreError::RecoveryRequired { .. })
            ));
        }
        second.recover().unwrap().agent_stamp.unwrap();
        assert_document(
            &first,
            "alpha",
            "record",
            if mode == "pause-before" { "old" } else { "new" },
        );
        if mode == "pause-transaction" {
            assert_document(&first, "alpha", "record-peer", "new");
        }
        assert!(!diagnostic.recovery_need().unwrap().required());
        first
            .write_document_replicated(node("alpha"), &document("later"), &body("after recovery"))
            .unwrap()
            .agent_stamp
            .unwrap();
    }
}

#[test]
fn an_unresolved_transaction_in_a_live_process_fences_other_processes() {
    let root = TempRoot::new("live-fence");
    seed_store(root.path());
    let failed = Writer::spawn_mode(
        root.path(),
        "alpha",
        "record",
        "committed",
        "fail-transaction",
    );
    failed.expect("opened");
    failed.expect("failed");
    let diagnostic = DiagnosticView::open(vfs(), root.path()).unwrap();
    let need = diagnostic.recovery_need().unwrap();
    assert!(need.fenced() && !need.holder_died());
    let mut survivor = session(root.path(), "survivor", "after", "test");
    require_recovery(&mut survivor);
    assert_document(&open_reader(root.path()), "alpha", "record", "committed");
    survivor.command("transaction");
    survivor.expect("wrote:0");
    survivor.finish();
    failed.finish();
}

#[test]
fn a_commit_log_reset_waits_for_another_processes_transaction_to_end() {
    let root = TempRoot::new("reset-serialization");
    seed_store(root.path());
    let mut writer = Writer::spawn_mode(
        root.path(),
        "alpha",
        "record",
        "committed",
        "pause-transaction",
    );
    writer.expect("opened");
    writer.expect("paused");
    let before = vfs().read(&root.path().join(LOG_FILE)).unwrap();
    assert!(!before.is_empty());
    let mut resetter = session(root.path(), "resetter", "unused", "test");
    resetter.command("quiesce");
    resetter.expect("quiescing");
    assert!(test_timers::mpsc::recv_timeout(&resetter.lines, Duration::from_millis(100)).is_err());
    assert_eq!(vfs().read(&root.path().join(LOG_FILE)).unwrap(), before);
    writer.go();
    writer.expect("wrote");
    resetter.expect("quiesced");
    assert_document(&open_reader(root.path()), "alpha", "record", "committed");
    writer.finish();
    resetter.finish();
}

#[test]
fn agent_versions_follow_each_processes_successful_turn_and_registry_names_both() {
    let root = TempRoot::new("agent-turns");
    seed_store(root.path());
    let reader = open_reader(root.path());
    let mut first = session(root.path(), "first", "A", "A");
    let mut second = session(root.path(), "second", "B", "B");
    let listed = reader.attached_handles().unwrap();
    assert!(!listed.may_be_incomplete);
    for (writer, version) in [(&first, "A"), (&second, "B")] {
        assert!(
            listed.live.iter().any(
                |entry| entry.process_id == writer.child.id() && entry.agent_version == version
            )
        );
    }
    for sequence in 0..12 {
        for (writer, version) in [(&mut first, "A"), (&mut second, "B")] {
            writer.command("transaction");
            writer.expect(&format!("wrote:{sequence}"));
            assert_eq!(reader.properties().unwrap().agent_version, version);
        }
    }
    first.finish();
    second.finish();
}

#[test]
fn two_os_handles_in_one_process_serialize_conditional_creates() {
    let root = TempRoot::new("same-process-handles");
    seed_store(root.path());
    let (first, _) = Store::open(vfs(), root.path(), nonce_source("first")).unwrap();
    let (second, _) = Store::open(vfs(), root.path(), nonce_source("second")).unwrap();
    for sequence in 0..12 {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writers: Vec<_> = [first.clone(), second.clone()]
            .into_iter()
            .map(|store| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let outcome = store
                        .update_document(Swap::conditional_absent(
                            node("docs"),
                            document(&format!("once{sequence}")),
                            body("one winner"),
                        ))
                        .unwrap();
                    outcome.agent_stamp.unwrap();
                    outcome.value.is_committed()
                })
            })
            .collect();
        assert_eq!(
            writers
                .into_iter()
                .map(|writer| usize::from(writer.join().unwrap()))
                .sum::<usize>(),
            1
        );
    }
}

#[test]
fn a_process_using_another_coordination_implementation_is_refused_without_effects() {
    let root = TempRoot::new("foreign-implementation");
    seed_store(root.path());
    let reader = open_reader(root.path());
    let before = reader.properties().unwrap();
    let count = reader.modifications().unwrap();
    let writer = Writer::spawn_mode(root.path(), "docs", "forbidden", "unused", "foreign");
    writer.expect("refused");
    writer.expect_failure();
    assert_eq!(reader.properties().unwrap(), before);
    assert_eq!(reader.modifications().unwrap(), count);
    assert_eq!(
        reader
            .read_document(node("docs"), &document("forbidden"))
            .unwrap(),
        Read::Absent
    );
    assert_eq!(reader.attached_handles().unwrap().live.len(), 1);
}

#[test]
fn an_invalid_fixture_mode_is_refused_before_opening_or_writing() {
    let root = TempRoot::new("invalid-mode");
    seed_store(root.path());
    let reader = open_reader(root.path());
    let count = reader.modifications().unwrap();
    Writer::spawn_mode(root.path(), "docs", "forbidden", "unused", "unknown-mode").expect_failure();
    assert_eq!(reader.modifications().unwrap(), count);
    assert_eq!(
        reader
            .read_document(node("docs"), &document("forbidden"))
            .unwrap(),
        Read::Absent
    );
    assert_eq!(reader.attached_handles().unwrap().live.len(), 1);
}

fn dirty_transaction(root: &Path) {
    let writer = Writer::spawn_mode(root, "alpha", "record", "committed", "pause-transaction");
    writer.expect("opened");
    writer.expect("paused");
    writer.kill();
}

fn replica_path(root: &Path, name: &str) -> PathBuf {
    // Validated names carry the normalization required on case-sensitive filesystems.
    root.join("namespace")
        .join("p0")
        .join(node("docs").relative_path())
        .join(document(name).as_str())
}

#[test]
fn recovery_killed_after_every_phase_is_resumable_by_another_process() {
    for phase in [
        "DirectorySyncDebt",
        "Publication",
        "Transactions",
        "Collection",
        "Claims",
        "Completion",
    ] {
        let root = TempRoot::new(phase);
        seed_store(root.path());
        if phase == "Publication" {
            let writer = Writer::spawn(root.path(), "alpha", "record", "committed", true);
            writer.expect("opened");
            writer.expect("paused");
            writer.kill();
        } else {
            dirty_transaction(root.path());
        }
        if phase == "DirectorySyncDebt" {
            let filesystem = vfs();
            filesystem
                .create_dir_all(&root.path().join("namespace/needs-sync"))
                .unwrap();
            filesystem
                .write(&root.path().join(DEBT_FILE), b"namespace/needs-sync\n")
                .unwrap();
            filesystem.fsync(&root.path().join(DEBT_FILE)).unwrap();
            filesystem.fsync(root.path()).unwrap();
        }
        let mut recovering =
            Writer::spawn_mode(root.path(), "unused", "unused", "unused", "recover-step");
        recovering.expect("opened");
        let mut reached = false;
        for _ in 0..30 {
            let reported = recovering.next();
            if reported == format!("phase:{phase}") {
                reached = true;
                break;
            }
            assert!(reported.starts_with("phase:"), "{reported}");
            recovering.go();
        }
        assert!(reached, "the recovery did not reach {phase}");
        let process = recovering.child.id();
        recovering.kill();
        let reader = open_reader(root.path());
        assert!(
            reader
                .attached_handles()
                .unwrap()
                .live
                .iter()
                .all(|entry| entry.process_id != process)
        );
        let mut survivor = session(root.path(), "survivor", "after recovery", "test");
        if phase != "Completion" {
            assert!(reader.recovery_need().unwrap().fenced());
            require_recovery(&mut survivor);
        } else {
            assert!(!reader.recovery_need().unwrap().required());
            survivor.command("recover");
            survivor.expect("recovered");
        }
        assert_document(&reader, "alpha", "record", "committed");
        assert!(settled(reader.modifications().unwrap()));
        survivor.command("write");
        survivor.expect("wrote:0");
        survivor.finish();
    }
}

#[test]
fn two_processes_drive_the_same_recovery_without_stale_replays() {
    let root = TempRoot::new("concurrent-recovery");
    seed_store(root.path());
    dirty_transaction(root.path());
    let mut first = session(root.path(), "first", "A", "test");
    let mut second = session(root.path(), "second", "B", "test");
    let mut finished = [false; 2];
    for _ in 0..50 {
        for (index, writer) in [&mut first, &mut second].into_iter().enumerate() {
            if !finished[index] {
                writer.command("step");
            }
        }
        for (index, writer) in [&first, &second].into_iter().enumerate() {
            if !finished[index] {
                let line = writer.next();
                assert!(line.starts_with("phase:"), "{line}");
                finished[index] = line.ends_with(":true");
            }
        }
        if finished == [true; 2] {
            break;
        }
    }
    assert_eq!(finished, [true; 2]);
    assert_document(&open_reader(root.path()), "alpha", "record", "committed");
    first.finish();
    second.finish();
}

#[test]
fn a_scrub_and_another_processes_resolving_do_not_share_live_claims() {
    let root = TempRoot::new("scrub-resolve");
    seed_store(root.path());
    let (store, _) = Store::open(vfs(), root.path(), nonce_source("seed-docs")).unwrap();
    for sequence in 0..128 {
        let name = format!("repair{sequence}");
        store
            .write_document_replicated(node("docs"), &document(&name), &body("repairable"))
            .unwrap()
            .agent_stamp
            .unwrap();
        vfs()
            .remove_file(&replica_path(root.path(), &name))
            .unwrap();
    }
    drop(store);
    let mut scrub = Writer::spawn_mode(root.path(), "unused", "unused", "unused", "scrub-paused");
    scrub.expect("opened");
    scrub.expect("paused");
    assert!(
        vfs()
            .read_dir(&root.path().join(CLAIMS_DIRECTORY))
            .unwrap()
            .is_empty()
    );
    let mut resolver = session(root.path(), "resolver", "unused", "test");
    resolver.command("resolve");
    resolver.expect("resolved");
    scrub.go();
    scrub.expect("scrubbed");
    for sequence in 0..128 {
        assert!(
            vfs()
                .exists(&replica_path(root.path(), &format!("repair{sequence}")))
                .unwrap()
        );
    }
    assert!(
        vfs()
            .read_dir(&root.path().join(CLAIMS_DIRECTORY))
            .unwrap()
            .is_empty()
    );
    scrub.finish();
    resolver.finish();
}
