// Copyright (c) Michael Grier.

use super::*;
use canopy::{Agent, DiagnosticView, Swap};

fn options(prefix: &'static str, schema: u32) -> StoreOptions {
    let mut next = 0;
    StoreOptions::with_nonce_source(
        Agent::new("canopy-tests", "test", "Canopy test fixture", schema),
        move || {
            next += 1;
            Nonce::new(&format!("{prefix}-{next}")).unwrap()
        },
    )
    .with_coordination(Arc::new(acadia::coord::shared::SharedFactory::new()))
}

fn seed(root: &Path) -> Store {
    Store::init(
        vfs(),
        root,
        GenesisConfig::new(StoreId::new("shared-backend").unwrap(), 0),
        options("seed-shared", 1),
    )
    .unwrap()
}

#[test]
fn shared_processes_interleave_writes_and_snapshots_through_the_store_contract() {
    let root = TempRoot::new("shared-backend-turns");
    let retained = seed(root.path());
    let reader = Store::open_read_only(vfs(), root.path(), options("reader-shared", 1)).unwrap();
    let before = reader.modifications().unwrap();
    let payload = "shared payload".repeat(128);
    let mut writers: Vec<_> = (0..4)
        .map(|id| {
            let writer = Writer::spawn_mode(
                root.path(),
                "docs",
                &format!("shared{id}"),
                &payload,
                "shared-session",
            );
            writer.expect("opened");
            writer
        })
        .collect();
    for sequence in 0..64 {
        for writer in &mut writers {
            writer.command(if sequence % 2 == 0 {
                "transaction"
            } else {
                "write"
            });
        }
        for writer in &writers {
            writer.expect(&format!("wrote:{sequence}"));
        }
    }
    assert_eq!(reader.modifications().unwrap(), before + 2 * 4 * 64);
    for id in 0..4 {
        for sequence in 0..64 {
            let name = document(&format!("shared{id}-{sequence}"));
            assert_eq!(
                reader
                    .snapshot(|view| view.read_document(node("docs"), &name))
                    .unwrap()
                    .unwrap(),
                Read::Found(body(&payload))
            );
        }
    }
    for writer in writers {
        writer.finish();
    }
    drop(retained);
}

#[test]
fn shared_writer_death_requires_explicit_store_recovery_at_each_boundary() {
    for mode in [
        "shared-pause-before",
        "shared-pause-publication",
        "shared-pause-transaction",
        "shared-pause-after",
    ] {
        let root = TempRoot::new(mode);
        let store = seed(root.path());
        store
            .update_document(Swap::new(node("docs"), document("record"), body("before")))
            .unwrap()
            .agent_stamp
            .unwrap();
        let writer = Writer::spawn_mode(root.path(), "docs", "record", "after", mode);
        writer.expect("opened");
        writer.expect("paused");
        writer.kill();
        let deadline = Instant::now() + RELEASE_DEADLINE;
        loop {
            let need = store.recovery_required().unwrap();
            if need {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "writer death did not require recovery"
            );
            std::thread::yield_now();
        }
        assert!(
            store
                .update_document(Swap::new(
                    node("docs"),
                    document("blocked"),
                    body("blocked")
                ))
                .is_err()
        );
        store.recover().unwrap().agent_stamp.unwrap();
        store
            .update_document(Swap::new(node("docs"), document("resumed"), body("good")))
            .unwrap()
            .agent_stamp
            .unwrap();
    }
}

#[test]
fn canonical_file_and_shared_processes_refuse_each_others_roots() {
    for shared_root in [false, true] {
        let root = TempRoot::new("mixed-backend-refusal");
        let retained = if shared_root {
            Some(seed(root.path()))
        } else {
            seed_store(root.path());
            None
        };
        let before = vfs().read(&root.path().join(".coord-reservation")).unwrap();
        let mode = if shared_root {
            "file-foreign"
        } else {
            "shared-foreign"
        };
        let writer = Writer::spawn_mode(root.path(), "docs", "forbidden", "wrong backend", mode);
        writer.expect("refused");
        writer.expect_failure();
        assert_eq!(
            vfs().read(&root.path().join(".coord-reservation")).unwrap(),
            before
        );
        assert!(!vfs().exists(&root.path().join(".coord-shared")).unwrap() || shared_root);
        drop(retained);
    }
}

#[test]
fn interrupted_shared_schema_upgrade_resumes_with_target_units() {
    let root = TempRoot::new("shared-upgrade-recovery");
    let retained = seed(root.path());
    let witness = Store::open_read_only(vfs(), root.path(), options("upgrade-witness", 1)).unwrap();
    drop(retained);
    let mut writer = Writer::spawn_mode(
        root.path(),
        "docs",
        "migration",
        "migrated",
        "shared-upgrade-safe",
    );
    writer.expect("opened");
    writer.command("begin");
    writer.expect("intent");
    writer.command("step");
    writer.expect("unit:1");
    writer.kill();
    let mut survivor = Writer::spawn_extra(
        root.path(),
        "docs",
        "migration",
        "migrated",
        "shared-resume-upgrade",
        "test",
        &["2".into(), "3".into(), "2".into(), "3".into()],
    );
    survivor.expect("opening");
    survivor.expect("opened");
    survivor.expect("recovered");
    let diagnostic = DiagnosticView::open_with_coordination(
        vfs(),
        root.path(),
        Arc::new(acadia::coord::shared::SharedFactory::new()),
    )
    .unwrap();
    assert_eq!(diagnostic.properties().unwrap().agent_schema.current, 2);
    let reader = Store::open_read_only(vfs(), root.path(), options("target-reader", 2)).unwrap();
    for unit in 0..3 {
        for index in 0..64 {
            assert_eq!(
                reader
                    .read_document(
                        node("docs"),
                        &document(&format!("migration-{unit}-{index}"))
                    )
                    .unwrap(),
                Read::Found(body("migrated"))
            );
        }
    }
    survivor.command("write");
    survivor.expect("wrote:0");
    survivor.finish();
    drop(witness);
}
