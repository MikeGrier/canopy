// Copyright (c) Michael Grier.

use super::*;
use canopy::{
    Agent, DiagnosticView, MigrationUnit, RecoveryPhase, SchemaLayer, Swap, SwapSet,
    UPGRADE_INTENT_NAME, UpgradeRequest,
};

const UNITS: u64 = 3;
const DOCUMENTS_PER_UNIT: usize = 64;

fn spawn(root: &Path, mode: &str, source: u32, target: u32, floor: u32) -> Writer {
    Writer::spawn_extra(
        root,
        "docs",
        "migration",
        "migrated",
        mode,
        "test",
        &[
            source.to_string(),
            target.to_string(),
            floor.to_string(),
            UNITS.to_string(),
        ],
    )
}

fn opened(root: &Path, mode: &str, schema: u32) -> Writer {
    let writer = spawn(root, mode, schema, schema + 1, schema);
    writer.expect("opening");
    writer.expect("opened");
    writer
}

fn prepare(writer: &mut Writer, begin: bool) {
    writer.command(if begin { "begin" } else { "poll" });
    let deadline = Instant::now() + STEP_DEADLINE;
    loop {
        match writer.next().as_str() {
            "intent" => return,
            "pending" => {
                assert!(
                    Instant::now() < deadline,
                    "the upgrade never obtained its tiers"
                );
                test_timers::thread::sleep(Duration::from_millis(5));
                writer.command("poll");
            }
            other => panic!("unexpected upgrade progress: {other}"),
        }
    }
}

fn steps(writer: &mut Writer, count: u64) {
    for unit in 1..=count {
        writer.command("step");
        writer.expect(&format!("unit:{unit}"));
    }
}

fn assert_migrated(root: &Path, target: u32) {
    let diagnostic = DiagnosticView::open(vfs(), root).unwrap();
    assert_eq!(
        diagnostic.properties().unwrap().agent_schema.current,
        target
    );
    assert!(!diagnostic.recovery_need().unwrap().required());
    assert!(!vfs().exists(&root.join(UPGRADE_INTENT_NAME)).unwrap());
    let reader = Store::open_read_only(
        vfs(),
        root,
        StoreOptions::without_nonce_source(Agent::new(
            "canopy-tests",
            "test",
            "Canopy test fixture",
            target,
        )),
    )
    .unwrap();
    for unit in 0..UNITS {
        for index in 0..DOCUMENTS_PER_UNIT {
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
}

fn resume_process(root: &Path, target: u32) {
    let mut survivor = opened(root, "resume-upgrade", target);
    survivor.expect("recovered");
    assert_migrated(root, target);
    survivor.command("write");
    survivor.expect("wrote:0");
    survivor.finish();
}

fn assert_source_refused(root: &Path, source: u32) {
    let refused = spawn(root, "source-probe", source, source + 1, source);
    refused.expect("opening");
    refused.expect("schema-refused");
    refused.expect_failure();
}

fn target_options() -> StoreOptions {
    let mut sequence = 0;
    StoreOptions::with_nonce_source(
        Agent::new("canopy-tests", "target", "Canopy test fixture", 2),
        move || {
            sequence += 1;
            Nonce::new(&format!("resume{sequence}")).unwrap()
        },
    )
}

fn unit(context: &MigrationUnit<'_>) -> Result<(), StoreError> {
    let content = body("migrated");
    let mut swaps = Vec::new();
    for index in 0..DOCUMENTS_PER_UNIT {
        let name = document(&format!(
            "migration-{}-{index}",
            context.progress().next_unit
        ));
        if !matches!(context.read_document(node("docs"), &name)?, Read::Found(ref existing) if existing == &content)
        {
            swaps.push(Swap::new(node("docs"), name, content.clone()));
        }
    }
    if swaps.is_empty() {
        return Ok(());
    }
    context
        .apply_transaction(&SwapSet::new(swaps).unwrap())
        .map(|_| ())
}

#[test]
fn an_upgrader_killed_after_intent_is_refused_to_source_writers_and_resumed_by_target() {
    let root = TempRoot::new("upgrade-intent-death");
    seed_store(root.path());
    let mut writer =
        Writer::spawn_mode(root.path(), "docs", "migration", "migrated", "upgrade-safe");
    writer.expect("opened");
    writer.command("begin");
    writer.expect("intent");
    assert!(
        vfs()
            .exists(&root.path().join(UPGRADE_INTENT_NAME))
            .unwrap()
    );
    writer.kill();
    assert!(matches!(
        Store::open(vfs(), root.path(), nonce_source("source")),
        Err(StoreError::SchemaMismatch { .. })
    ));
    let (resumer, found) = Store::open(vfs(), root.path(), target_options()).unwrap();
    assert!(found.required());
    let mut recovery = resumer.recovery().unwrap();
    while !recovery.progress().finished() {
        if recovery.progress().phase() == RecoveryPhase::UpgradeBody {
            recovery.step_upgrade(unit).unwrap().agent_stamp.unwrap();
        } else {
            recovery.step().unwrap().agent_stamp.unwrap();
        }
    }
    assert_eq!(resumer.properties().unwrap().agent_schema.current, 2);
    assert!(
        !vfs()
            .exists(&root.path().join(UPGRADE_INTENT_NAME))
            .unwrap()
    );
}

#[test]
fn a_killed_upgrader_resumes_after_every_durable_body_boundary() {
    for stopped_after in 0..=UNITS {
        let root = TempRoot::new(&format!("upgrade-unit-{stopped_after}"));
        seed_store(root.path());
        let mut upgrader = spawn(root.path(), "upgrade-safe", 1, 2, 1);
        upgrader.expect("opened");
        prepare(&mut upgrader, true);
        steps(&mut upgrader, stopped_after);
        let process = upgrader.child.id();
        let diagnostic = DiagnosticView::open(vfs(), root.path()).unwrap();
        assert!(
            diagnostic
                .attached_handles()
                .unwrap()
                .live
                .iter()
                .any(|entry| entry.process_id == process
                    && entry.tiers.writer == Some(acadia::coord::TierHold::Exclusive))
        );
        assert!(!diagnostic.recovery_need().unwrap().holder_died());
        upgrader.kill();
        assert!(diagnostic.recovery_need().unwrap().holder_died());
        assert!(
            diagnostic
                .attached_handles()
                .unwrap()
                .live
                .iter()
                .all(|entry| entry.process_id != process)
        );
        assert_source_refused(root.path(), 1);
        resume_process(root.path(), 2);
    }
}

#[test]
fn death_after_schema_publication_clears_intent_without_reapplying_units() {
    let root = TempRoot::new("upgrade-after-schema");
    seed_store(root.path());
    let mut upgrader = spawn(root.path(), "upgrade-after-schema", 1, 2, 1);
    upgrader.expect("opened");
    prepare(&mut upgrader, true);
    steps(&mut upgrader, UNITS);
    upgrader.command("complete");
    upgrader.expect("schema-advanced");
    let diagnostic = DiagnosticView::open(vfs(), root.path()).unwrap();
    assert_eq!(diagnostic.properties().unwrap().agent_schema.current, 2);
    assert!(
        vfs()
            .exists(&root.path().join(UPGRADE_INTENT_NAME))
            .unwrap()
    );
    upgrader.kill();
    assert_source_refused(root.path(), 1);
    resume_process(root.path(), 2);
}

#[test]
fn an_upgrade_waits_for_an_attached_writer_and_the_registry_names_it() {
    let root = TempRoot::new("upgrade-writer-blocker");
    seed_store(root.path());
    let blocker = Writer::spawn_mode(root.path(), "docs", "blocker", "held", "session");
    blocker.expect("opened");
    let process = blocker.child.id();
    let mut upgrader = spawn(root.path(), "upgrade-safe", 1, 2, 1);
    upgrader.expect("opened");
    upgrader.command("begin");
    upgrader.expect("pending");
    let diagnostic = DiagnosticView::open(vfs(), root.path()).unwrap();
    assert!(
        diagnostic
            .attached_handles()
            .unwrap()
            .live
            .iter()
            .any(|entry| entry.process_id == process && entry.agent_schema == 1)
    );
    assert!(
        !vfs()
            .exists(&root.path().join(UPGRADE_INTENT_NAME))
            .unwrap()
    );
    blocker.kill();
    assert!(
        diagnostic
            .attached_handles()
            .unwrap()
            .live
            .iter()
            .all(|entry| entry.process_id != process)
    );
    prepare(&mut upgrader, false);
    steps(&mut upgrader, UNITS);
    upgrader.command("complete");
    upgrader.expect("completed");
    upgrader.finish();
    assert_migrated(root.path(), 2);
}

fn advance_to_two(root: &Path) {
    let (store, _) = Store::open(vfs(), root, nonce_source("setup")).unwrap();
    let upgrade = store
        .drain_upgrade()
        .unwrap()
        .begin_conversion(UpgradeRequest::new(SchemaLayer::Agent, 1, 2, 1).unwrap())
        .unwrap();
    assert!(upgrade.poll().unwrap());
    upgrade.complete().unwrap().agent_stamp.unwrap();
}

#[test]
fn an_unsafe_upgrade_drains_attached_readers_of_every_admissible_schema() {
    let root = TempRoot::new("upgrade-reader-blockers");
    seed_store(root.path());
    advance_to_two(root.path());
    let first = opened(root.path(), "reader", 1);
    let second = opened(root.path(), "reader", 2);
    let first_process = first.child.id();
    let second_process = second.child.id();
    let mut upgrader = spawn(root.path(), "upgrade-unsafe", 2, 3, 3);
    upgrader.expect("opened");
    upgrader.command("begin");
    upgrader.expect("pending");
    let diagnostic = DiagnosticView::open(vfs(), root.path()).unwrap();
    let listed = diagnostic.attached_handles().unwrap();
    assert!(
        listed
            .live
            .iter()
            .any(|entry| entry.process_id == first_process && entry.agent_schema == 1)
    );
    assert!(
        listed
            .live
            .iter()
            .any(|entry| entry.process_id == second_process && entry.agent_schema == 2)
    );
    first.kill();
    upgrader.command("poll");
    upgrader.expect("pending");
    second.kill();
    assert!(
        diagnostic
            .attached_handles()
            .unwrap()
            .live
            .iter()
            .all(|entry| entry.process_id != first_process && entry.process_id != second_process)
    );
    prepare(&mut upgrader, false);
    steps(&mut upgrader, UNITS);
    upgrader.command("complete");
    upgrader.expect("completed");
    upgrader.finish();
    assert_migrated(root.path(), 3);
    assert_eq!(
        diagnostic.properties().unwrap().agent_schema.oldest_reader,
        3
    );
}

#[test]
fn a_new_reader_waits_at_the_gate_and_is_refused_after_an_unsafe_completion() {
    let root = TempRoot::new("upgrade-gate-admission");
    seed_store(root.path());
    let blocker = opened(root.path(), "reader", 1);
    let mut upgrader = spawn(root.path(), "upgrade-unsafe", 1, 2, 2);
    upgrader.expect("opened");
    upgrader.command("begin");
    upgrader.expect("pending");
    let waiting = spawn(root.path(), "reader-wait", 1, 2, 1);
    waiting.expect("opening");
    waiting.expect("admission-gate");
    assert!(test_timers::mpsc::recv_timeout(&waiting.lines, Duration::from_millis(100)).is_err());
    let process = waiting.child.id();
    assert!(
        DiagnosticView::open(vfs(), root.path())
            .unwrap()
            .attached_handles()
            .unwrap()
            .live
            .iter()
            .all(|entry| entry.process_id != process)
    );
    blocker.kill();
    prepare(&mut upgrader, false);
    assert!(test_timers::mpsc::recv_timeout(&waiting.lines, Duration::from_millis(50)).is_err());
    steps(&mut upgrader, UNITS);
    upgrader.command("complete");
    upgrader.expect("completed");
    waiting.expect("schema-refused");
    waiting.expect_failure();
    upgrader.finish();
    assert_migrated(root.path(), 2);
}

#[test]
fn a_failed_body_leaves_fence_and_intent_without_an_abandoned_owner_record() {
    let root = TempRoot::new("upgrade-body-failure");
    seed_store(root.path());
    let mut upgrader = spawn(root.path(), "upgrade-safe", 1, 2, 1);
    upgrader.expect("opened");
    prepare(&mut upgrader, true);
    upgrader.command("fail-unit");
    upgrader.expect("body-failed");
    upgrader.expect_failure();
    let diagnostic = DiagnosticView::open(vfs(), root.path()).unwrap();
    let need = diagnostic.recovery_need().unwrap();
    assert!(need.fenced() && !need.holder_died());
    assert!(
        vfs()
            .exists(&root.path().join(UPGRADE_INTENT_NAME))
            .unwrap()
    );
    assert_source_refused(root.path(), 1);
    resume_process(root.path(), 2);
}

#[test]
fn recovery_required_state_refuses_an_upgrader_before_any_intent() {
    let root = TempRoot::new("upgrade-recovery-refusal");
    seed_store(root.path());
    let failed = Writer::spawn_mode(
        root.path(),
        "docs",
        "failed",
        "committed",
        "fail-transaction",
    );
    failed.expect("opened");
    failed.expect("failed");
    failed.finish();
    let diagnostic = DiagnosticView::open(vfs(), root.path()).unwrap();
    let before = diagnostic.properties().unwrap();
    let mut upgrader = spawn(root.path(), "upgrade-safe", 1, 2, 1);
    upgrader.expect("opened");
    upgrader.command("begin");
    upgrader.expect("upgrade-refused");
    upgrader.expect_failure();
    assert_eq!(diagnostic.properties().unwrap(), before);
    assert!(
        !vfs()
            .exists(&root.path().join(UPGRADE_INTENT_NAME))
            .unwrap()
    );
    let (recovering, _) = Store::open(vfs(), root.path(), nonce_source("recover")).unwrap();
    recovering.recover().unwrap().agent_stamp.unwrap();
}

#[test]
fn competing_upgraders_release_shared_tiers_and_finish_without_deadlock() {
    let root = TempRoot::new("competing-upgraders");
    seed_store(root.path());
    let mut first = spawn(root.path(), "upgrade-safe", 1, 2, 1);
    let mut second = spawn(root.path(), "upgrade-safe", 1, 2, 1);
    first.expect("opened");
    second.expect("opened");
    first.command("begin");
    first.expect("pending");
    second.command("begin");
    second.expect("pending");
    prepare(&mut first, false);
    second.command("poll");
    second.expect("upgrade-refused");
    second.expect_failure();
    steps(&mut first, UNITS);
    first.command("complete");
    first.expect("completed");
    first.finish();
    assert_migrated(root.path(), 2);
}
