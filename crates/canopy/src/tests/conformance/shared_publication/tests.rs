// Copyright (c) Michael Grier.

use crate::store::PUBLICATION_RECORD_NAME;
use crate::tests::{test_agent, test_config};
use crate::{ContentDigest, Nonce, Store, StoreOptions};
use acadia::{MemFs, Vfs};
use serde_json::{Map, Value, json};
use std::sync::Arc;

use super::super::corpus;
use super::action::infer;
use super::process::{self, Class, Command, Implementation, Process, Reply};
use super::projection::{self, World};

fn initialized(world: &World) -> (Arc<MemFs>, Store) {
    initialized_with(world, Implementation::File)
}

pub(super) fn initialized_with(
    world: &World,
    implementation: Implementation,
) -> (Arc<MemFs>, Store) {
    let mem = Arc::new(MemFs::new());
    mem.create_dir_all(&world.root).unwrap();
    let config = test_config()
        .with_pointer_replica_count(world.pointers)
        .unwrap()
        .with_blob_replica_count(world.blobs)
        .unwrap();
    let mut next_nonce = 0;
    let options = StoreOptions::with_nonce_source(test_agent(), move || {
        next_nonce += 1;
        Nonce::new(&format!("N{next_nonce}")).unwrap()
    });
    let store = Store::init(
        mem.clone(),
        &world.root,
        config,
        options.with_coordination(process::factory(implementation, None)),
    )
    .unwrap();
    for (index, document) in world.documents.iter().enumerate() {
        store
            .write_document_replicated_as(
                &document.node,
                &document.name,
                &World::body(0),
                &Nonce::new(&format!("initial{index}")).unwrap(),
            )
            .unwrap();
    }
    store.quiesce().unwrap().agent_stamp.unwrap();
    store.recover().unwrap().agent_stamp.unwrap();
    {
        let _exclusion = store.exclusive();
        store.resolve_data_work().unwrap();
    }
    let debt = world.directory_record();
    assert!(
        mem.read(&debt).unwrap_or_default().is_empty(),
        "fixture leaves live directory debt"
    );
    mem.fsync(&debt).unwrap();
    let durable = mem.fork();
    durable.simulate_crash();
    assert!(
        durable.read(&debt).unwrap_or_default().is_empty(),
        "fixture leaves durable directory debt"
    );
    (mem, store)
}

#[test]
fn a_store_process_pauses_before_publication_and_then_finishes_its_write() {
    for implementation in [Implementation::File, Implementation::Shared] {
        let world = Arc::new(World::new(2, 3, 1, 1, true));
        let (mem, _keeper) = initialized_with(&world, implementation);
        let process = Process::start(&mem, world.clone(), implementation, 1);
        process.command(Command::Open, None).unwrap();
        assert_eq!(process.returned(), Some(Reply::Done));
        let stop = Class::Document(super::super::pointer_publication::class::Class::RecordAppend);
        process
            .command(
                Command::Write {
                    document: 2,
                    version: 1,
                    logged: false,
                },
                Some(stop),
            )
            .unwrap();
        assert_eq!(process.paused(), Some(stop));
        let paused = projection::observe(&world, &mem).unwrap();
        assert_eq!(paused.live.pointers[1], vec![Some(0); 3]);
        assert_eq!(paused.live.records[1], None);
        process.step(None, false).unwrap();
        assert_eq!(process.returned(), Some(Reply::Done));
        assert_eq!(
            projection::observe(&world, &mem).unwrap().durable.pointers[1],
            vec![Some(1); 3]
        );
        process.command(Command::Close, None).unwrap();
    }
}

#[test]
fn a_shared_atomic_failure_is_refused_without_resuming_the_call() {
    let world = Arc::new(World::new(1, 3, 1, 1, true));
    let (mem, _keeper) = initialized_with(&world, Implementation::Shared);
    let process = Process::start(&mem, world, Implementation::Shared, 1);
    process.command(Command::Open, None).unwrap();
    let stop = Class::Contract(super::super::coordination::process::Class::Count);
    process.command(Command::Empty, Some(stop)).unwrap();
    assert!(process.step(None, true).unwrap_err().contains("infallible"));
    assert_eq!(process.paused(), Some(stop));
    process.step(None, false).unwrap();
    assert_eq!(process.returned(), Some(Reply::Done));
}

#[test]
fn the_initialized_store_matches_every_shared_publication_layout() {
    for trace in corpus::load("SharedPublication") {
        let world = World::of(&trace);
        let (mem, _store) = initialized(&world);
        world.seed_directory_debt(&mem);
        assert_eq!(
            projection::observe(&world, &mem).unwrap(),
            projection::model(&world, &trace.states[0]),
            "{}",
            trace.file
        );
    }
}

#[test]
fn alternating_document_writes_project_every_version() {
    let trace = corpus::load("SharedPublication")
        .into_iter()
        .find(|trace| trace.states[0]["ptrLive"].as_array().unwrap().len() == 2)
        .unwrap();
    let world = World::new(2, 3, 1, 10, true);
    let (mem, store) = initialized(&world);
    let mut expected = trace.states[0].clone();
    for version in 1..=10 {
        let index = (version - 1) % world.documents.len();
        let document = &world.documents[index];
        store
            .write_document_replicated_as(
                &document.node,
                &document.name,
                &World::body(version),
                &Nonce::new(&format!("write{version}")).unwrap(),
            )
            .unwrap();
        for field in ["ptrLive", "ptrDur"] {
            expected.get_mut(field).unwrap()[index] = json!(vec![version; world.pointers]);
        }
        for field in ["blobLive", "blobDur"] {
            expected.get_mut(field).unwrap()[index] = json!(vec![vec![version]; world.blobs]);
        }
        assert_eq!(
            projection::observe(&world, &mem).unwrap(),
            projection::model(&world, &expected),
            "version {version}"
        );
    }
}

#[test]
fn the_projection_distinguishes_live_and_durable_pointer_removal() {
    let world = World::new(2, 3, 1, 1, true);
    let (mem, _store) = initialized(&world);
    let initial = projection::observe(&world, &mem).unwrap();
    mem.remove_file(&world.entry(0, 0)).unwrap();
    let removed = projection::observe(&world, &mem).unwrap();
    assert_eq!(removed.live.pointers[0][0], None);
    assert_eq!(removed.durable.pointers[0][0], Some(0));
    assert_eq!(removed.live.pointers[1], initial.live.pointers[1]);
    mem.simulate_crash();
    assert_eq!(projection::observe(&world, &mem).unwrap(), initial);
}

fn publication_line(world: &World, document: usize, version: usize) -> Vec<u8> {
    let document = &world.documents[document];
    let mut bytes = serde_json::to_vec(&json!({"node": document.node.to_string(),
        "document": document.name.as_str(), "digest": ContentDigest::of(World::body(version).as_bytes()).to_hex(),
        "nonce": "record"})).unwrap();
    bytes.push(b'\n');
    bytes
}

#[test]
fn record_append_and_clear_have_independent_durability() {
    let world = World::new(2, 3, 1, 1, true);
    let (mem, _store) = initialized(&world);
    let path = world.root.join(PUBLICATION_RECORD_NAME);
    mem.append(&path, &publication_line(&world, 1, 1)).unwrap();
    let appended = projection::observe(&world, &mem).unwrap();
    assert_eq!(appended.live.records, vec![None, Some(1)]);
    assert_eq!(appended.durable.records, vec![None, None]);
    mem.fsync(&path).unwrap();
    mem.truncate(&path, 0).unwrap();
    let cleared = projection::observe(&world, &mem).unwrap();
    assert_eq!(cleared.live.records, vec![None, None]);
    assert_eq!(cleared.durable.records, vec![None, Some(1)]);
}

#[test]
fn torn_records_are_ignored_but_complete_invalid_or_duplicate_records_are_rejected() {
    let world = World::new(2, 3, 1, 1, true);
    let (mem, _store) = initialized(&world);
    let path = world.root.join(PUBLICATION_RECORD_NAME);
    let initial = projection::observe(&world, &mem).unwrap();
    mem.append(&path, b"{\"node\":").unwrap();
    assert_eq!(projection::observe(&world, &mem).unwrap(), initial);
    mem.append(&path, b"\n").unwrap();
    assert!(projection::observe(&world, &mem).is_err());
    mem.truncate(&path, 0).unwrap();
    let line = publication_line(&world, 0, 1);
    mem.append(&path, &line).unwrap();
    mem.append(&path, &line).unwrap();
    assert!(
        projection::observe(&world, &mem)
            .unwrap_err()
            .contains("duplicate")
    );
}

fn state(phase: &str, work: &str) -> Map<String, Value> {
    json!({"pc": [phase], "att": ["attached"], "wph": work, "wdoc": 1, "wix": 1,
        "lockHolder": 1, "crashes": 0, "terminations": 0, "fence": false, "log": "empty"})
    .as_object()
    .unwrap()
    .clone()
}

#[test]
fn document_steps_are_inferred_from_the_work_phase() {
    for (before, after, name) in [
        ("ingRen", "ingSync", "IngRen"),
        ("ingSync", "recWrite", "IngSync"),
        ("recWrite", "recSync", "RecWrite"),
        ("recSync", "ptrRen", "RecSync"),
        ("ptrRen", "ptrSync", "PtrRen"),
        ("ptrSync", "recClr", "PtrSync"),
        ("recClr", "recClrSync", "RecClr"),
        ("recClrSync", "collRead", "RecClrSync"),
        ("collRead", "collSync", "CollRead"),
        ("collSync", "collRem", "CollSync"),
        ("collRem", "collFsync", "CollRem"),
        ("collFsync", "done", "CollFsync"),
    ] {
        let action = infer(&state("writing", before), &state("writing", after)).unwrap();
        assert_eq!(action.name, name);
        assert_eq!(
            (action.actor, action.document, action.replica),
            (Some(1), 1, 1)
        );
    }
}

#[test]
fn every_existing_shared_publication_step_has_an_action() {
    let traces = corpus::load("SharedPublication");
    assert!(!traces.is_empty());
    for trace in traces {
        for (index, states) in trace.states.windows(2).enumerate() {
            infer(&states[0], &states[1])
                .unwrap_or_else(|error| panic!("{} {}: {error}", trace.file, trace.at(index)));
        }
    }
}

#[test]
fn a_crash_can_change_all_participants() {
    let mut before = state("writing", "ptrSync");
    before.insert("pc".into(), json!(["writing", "waiting"]));
    let mut after = before.clone();
    after.insert("pc".into(), json!(["terminated", "terminated"]));
    after.insert("crashes".into(), json!(1));
    assert_eq!(infer(&before, &after).unwrap().name, "Crash");
}

#[test]
fn another_participants_attachment_is_not_the_holders_document_step() {
    for work in [
        "ingRen",
        "ingSync",
        "recWrite",
        "recSync",
        "ptrRen",
        "ptrSync",
        "recClr",
        "recClrSync",
        "collRead",
        "collSync",
        "collRem",
        "collFsync",
    ] {
        let mut before = state("writing", work);
        before.insert("pc".into(), json!(["writing", "new"]));
        before.insert("att".into(), json!(["attached", "off"]));
        let mut after = before.clone();
        after.insert("pc".into(), json!(["writing", "attaching"]));
        after.insert("att".into(), json!(["attached", "wGate"]));
        let action = infer(&before, &after).unwrap();
        assert_eq!(action.name, "BeginAttach", "{work}");
        assert_eq!(action.actor, Some(2));
    }
}

#[test]
fn a_step_with_no_model_change_is_terminal() {
    let state = state("exited", "none");
    assert_eq!(infer(&state, &state).unwrap().name, "Terminal");
}

#[test]
fn unit_completion_is_distinct_from_interrupted_completion() {
    for (phase, work, expected) in [
        ("directories", "dirDone", "RecoveryUnitEnd"),
        ("collection", "txDone", "RecoveryUnitEnd"),
        ("completion", "txDone", "RecStop"),
    ] {
        let mut before = state("recReset", work);
        before.insert("runPhase".into(), json!([phase]));
        let mut after = before.clone();
        after.insert("pc".into(), json!(["told"]));
        after.insert("wph".into(), json!("none"));
        assert_eq!(infer(&before, &after).unwrap().name, expected);
    }
}

#[test]
fn unknown_transitions_and_malformed_indices_are_rejected() {
    let before = state("held", "none");
    let mut after = before.clone();
    after.insert("wph".into(), json!("unknown"));
    assert!(infer(&before, &after).is_err());
    after.insert("wix".into(), json!(-1));
    assert!(infer(&after, &before).is_err());
}

fn finish_recovery(process: &Process) {
    for _ in 0..32 {
        if matches!(
            process.returned(),
            Some(Reply::Recovery { finished: true, .. })
        ) {
            return;
        }
        process.command(Command::RecoveryUnit, None).unwrap();
    }
    panic!("recovery did not finish");
}

fn leave_pending_record(mem: &MemFs, world: Arc<World>, implementation: Implementation) {
    let mut writer = Process::start(mem, world.clone(), implementation, 1);
    writer.command(Command::Open, None).unwrap();
    let sync = Class::Document(super::super::pointer_publication::class::Class::RecordSync);
    let append = Class::Document(super::super::pointer_publication::class::Class::RecordAppend);
    writer
        .command(
            Command::Write {
                document: 2,
                version: 1,
                logged: false,
            },
            Some(append),
        )
        .unwrap();
    writer.step(Some(sync), false).unwrap();
    assert_eq!(
        projection::observe(&world, mem).unwrap().live.records[1],
        Some(1)
    );
    writer.end();
}

#[test]
fn an_ended_store_process_leaves_recovery_required_and_units_refuse_other_writes() {
    for implementation in [Implementation::File, Implementation::Shared] {
        let world = Arc::new(World::new(2, 3, 1, 2, true));
        let (mem, keeper) = initialized_with(&world, implementation);
        leave_pending_record(&mem, world.clone(), implementation);
        assert!(keeper.recovery_required().unwrap());
        let recovery = Process::start(&mem, world.clone(), implementation, 2);
        recovery.command(Command::Open, None).unwrap();
        recovery.command(Command::BeginRecovery, None).unwrap();
        recovery.command(Command::RecoveryUnit, None).unwrap();
        let writer = Process::start(&mem, world.clone(), implementation, 3);
        writer.command(Command::Open, None).unwrap();
        let settled = projection::observe(&world, &mem).unwrap();
        writer
            .command(
                Command::Write {
                    document: 1,
                    version: 2,
                    logged: false,
                },
                None,
            )
            .unwrap();
        assert!(
            matches!(writer.returned(), Some(Reply::Failed(error)) if error.contains("recovery"))
        );
        assert_eq!(projection::observe(&world, &mem).unwrap(), settled);
        finish_recovery(&recovery);
        assert!(!keeper.recovery_required().unwrap());
        let recovered = projection::observe(&world, &mem).unwrap();
        assert_eq!(recovered.live.records, vec![None, None]);
        assert_eq!(recovered.durable.pointers[1], vec![Some(1); 3]);
    }
}

#[test]
fn a_paused_recovery_restarts_after_another_recovery_and_failed_write() {
    for implementation in [Implementation::File, Implementation::Shared] {
        let world = Arc::new(World::new(2, 3, 1, 2, true));
        let (mem, keeper) = initialized_with(&world, implementation);
        leave_pending_record(&mem, world.clone(), implementation);
        let paused = Process::start(&mem, world.clone(), implementation, 2);
        paused.command(Command::Open, None).unwrap();
        paused.command(Command::BeginRecovery, None).unwrap();
        for _ in 0..32 {
            if matches!(
                paused.returned(),
                Some(Reply::Recovery {
                    phase: crate::RecoveryPhase::Completion,
                    finished: false
                })
            ) {
                break;
            }
            paused.command(Command::RecoveryUnit, None).unwrap();
        }
        assert!(matches!(
            paused.returned(),
            Some(Reply::Recovery {
                phase: crate::RecoveryPhase::Completion,
                finished: false
            })
        ));
        let finisher = Process::start(&mem, world.clone(), implementation, 3);
        finisher.command(Command::Open, None).unwrap();
        finisher.command(Command::BeginRecovery, None).unwrap();
        finish_recovery(&finisher);
        assert!(!keeper.recovery_required().unwrap());
        let sync = Class::Document(super::super::pointer_publication::class::Class::RecordSync);
        let append = Class::Document(super::super::pointer_publication::class::Class::RecordAppend);
        finisher
            .command(
                Command::Write {
                    document: 1,
                    version: 2,
                    logged: false,
                },
                Some(append),
            )
            .unwrap();
        finisher.step(Some(sync), false).unwrap();
        finisher.step(None, true).unwrap();
        assert!(matches!(finisher.returned(), Some(Reply::Failed(_))));
        assert_eq!(
            projection::observe(&world, &mem).unwrap().live.records[0],
            Some(2)
        );
        paused.command(Command::RecoveryUnit, None).unwrap();
        assert!(matches!(
            paused.returned(),
            Some(Reply::Recovery {
                finished: false,
                ..
            })
        ));
        assert_eq!(
            projection::observe(&world, &mem).unwrap().durable.pointers[0],
            vec![Some(2); 3]
        );
        finish_recovery(&paused);
        assert!(!keeper.recovery_required().unwrap());
    }
}

#[test]
fn a_healthy_corpus_prefix_replays_through_store_calls() {
    let mut selected = None;
    for mut trace in corpus::load("SharedPublication") {
        let mut unsafe_prefix = false;
        for index in 0..trace.steps.len() {
            let action = infer(&trace.states[index], &trace.states[index + 1]).unwrap();
            unsafe_prefix |= matches!(
                action.name,
                "Crash"
                    | "ProcessEnds"
                    | "RecStart"
                    | "FailUnresolved"
                    | "PrepareFails"
                    | "OpenFails"
                    | "CloseFails"
                    | "RollbackBeforePrepare"
            );
            if matches!(action.name, "RecWrite" | "PtrRen") && !unsafe_prefix {
                trace.steps.truncate(index + 1);
                trace.states.truncate(index + 2);
                trace.departs = None;
                selected = Some(trace);
                break;
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let trace = selected.expect("the corpus contains a healthy publication prefix");
    for implementation in [Implementation::File, Implementation::Shared] {
        super::judge(&trace, implementation)
            .unwrap_or_else(|error| panic!("{}: {error}", trace.file));
    }
}

macro_rules! corpus_cases {
    ($($name:ident => $file:literal),+ $(,)?) => {
        const CORPUS_CASES: &[&str] = &[$($file),+];
        $(#[test]
        fn $name() {
            let trace = corpus::load_one("SharedPublication", $file);
            check_corpus_trace(&trace);
        })+
    };
}

fn check_corpus_trace(trace: &corpus::Trace) {
    let implementation = if trace.constant("PublicationsFail") == "FALSE" {
        Implementation::Shared
    } else {
        Implementation::File
    };
    super::judge(trace, implementation).unwrap_or_else(|error| panic!("{}: {error}", trace.file));
}

corpus_cases! {
    recovery_control_timeline => "T27.json",
    publication_seed_1 => "sim-SharedPublication-s1.json",
    publication_seed_2 => "sim-SharedPublication-s2.json",
    publication_seed_3 => "sim-SharedPublication-s3.json",
    publication_seed_4 => "sim-SharedPublication-s4.json",
    transaction_seed_1 => "sim-SharedPublication-RecordTxn-s1.json",
    transaction_seed_2 => "sim-SharedPublication-RecordTxn-s2.json",
    transaction_seed_3 => "sim-SharedPublication-RecordTxn-s3.json",
    transaction_seed_4 => "sim-SharedPublication-RecordTxn-s4.json",
    mixed_seed_1 => "sim-SharedPublication-Mixed-s1.json",
    mixed_seed_2 => "sim-SharedPublication-Mixed-s2.json",
    mixed_seed_3 => "sim-SharedPublication-Mixed-s3.json",
    mixed_seed_4 => "sim-SharedPublication-Mixed-s4.json",
    healthy_publication_seed_1 => "sim-SharedPublication-HealthyPublication-s1.json",
    healthy_publication_seed_2 => "sim-SharedPublication-HealthyPublication-s2.json",
    healthy_publication_seed_3 => "sim-SharedPublication-HealthyPublication-s3.json",
    healthy_publication_seed_4 => "sim-SharedPublication-HealthyPublication-s4.json",
    healthy_transaction_seed_1 => "sim-SharedPublication-HealthyTransactions-s1.json",
    healthy_transaction_seed_2 => "sim-SharedPublication-HealthyTransactions-s2.json",
    healthy_transaction_seed_3 => "sim-SharedPublication-HealthyTransactions-s3.json",
    healthy_transaction_seed_4 => "sim-SharedPublication-HealthyTransactions-s4.json",
    shared_publication_seed_1 => "sim-SharedPublication-SharedPublication-s1.json",
    shared_publication_seed_2 => "sim-SharedPublication-SharedPublication-s2.json",
    shared_publication_seed_3 => "sim-SharedPublication-SharedPublication-s3.json",
    shared_publication_seed_4 => "sim-SharedPublication-SharedPublication-s4.json",
    shared_transaction_seed_1 => "sim-SharedPublication-SharedTransactions-s1.json",
    shared_transaction_seed_2 => "sim-SharedPublication-SharedTransactions-s2.json",
    shared_transaction_seed_3 => "sim-SharedPublication-SharedTransactions-s3.json",
    shared_transaction_seed_4 => "sim-SharedPublication-SharedTransactions-s4.json",
    directory_clear_crash_witness => "witness-directory-clear-crash.json",
    directory_unit_witness => "witness-directory-unit.json",
    committed_collection_witness => "witness-recovery-collects-committed.json",
    recovered_rollback_collection_witness => "witness-recovery-collects-rollback.json",
    rollback_collection_witness => "witness-rollback-collects.json",
    stale_recovery_restart_witness => "witness-stale-recovery-restarts.json",
    write_collection_witness => "witness-write-collects.json",
}

#[test]
fn every_corpus_trace_has_a_replay_test() {
    let found: std::collections::BTreeSet<_> = corpus::load("SharedPublication")
        .into_iter()
        .map(|trace| trace.file)
        .collect();
    let expected = CORPUS_CASES
        .iter()
        .map(|file| format!("SharedPublication/{file}"))
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn directory_debt_recovery_uses_sync_and_clear_units() {
    for implementation in [Implementation::File, Implementation::Shared] {
        let mut world = World::new(2, 3, 1, 1, true);
        world.initial_directory_debt = 1;
        let world = Arc::new(world);
        let (mem, keeper) = initialized_with(&world, implementation);
        world.seed_directory_debt(&mem);
        keeper.abandon_for_replay();
        mem.simulate_crash();
        let process = Process::start(&mem, world.clone(), implementation, 1);
        process.command(Command::Open, None).unwrap();
        assert_eq!(process.returned(), Some(Reply::Done), "{implementation:?}");
        process.command(Command::BeginRecovery, None).unwrap();
        assert!(
            matches!(
                process.returned(),
                Some(Reply::Recovery {
                    phase: crate::RecoveryPhase::DirectorySyncDebt,
                    ..
                })
            ),
            "{implementation:?}: {:?}",
            process.returned()
        );
        process
            .command(Command::RecoveryUnit, Some(Class::DirectorySync))
            .unwrap();
        process.step(None, false).unwrap();
        process
            .command(Command::RecoveryUnit, Some(Class::DirectoryClear))
            .unwrap();
        process
            .step(Some(Class::DirectoryClearSync), false)
            .unwrap();
        assert!(mem.read(&world.directory_record()).unwrap().is_empty());
        let durable = mem.fork();
        durable.simulate_crash();
        assert!(!durable.read(&world.directory_record()).unwrap().is_empty());
        process.step(None, true).unwrap();
        assert!(matches!(process.returned(), Some(Reply::Failed(_))));
        process
            .command(Command::RecoveryUnit, Some(Class::DirectoryClearSync))
            .unwrap();
        process.step(None, false).unwrap();
        assert!(matches!(
            process.returned(),
            Some(Reply::Recovery {
                phase: crate::RecoveryPhase::Publication,
                ..
            })
        ));
        finish_recovery(&process);
    }
}

#[test]
fn the_unit_corpus_exercises_publication_transaction_collection_and_recovery() {
    let mut actions = std::collections::BTreeSet::new();
    for trace in corpus::load("SharedPublication") {
        for states in trace.states.windows(2) {
            actions.insert(infer(&states[0], &states[1]).unwrap().name);
        }
    }
    for required in [
        "RecWrite",
        "Prepare",
        "PtrRen",
        "CollRem",
        "RecoveryBegin",
        "RecoveryStep",
    ] {
        assert!(
            actions.contains(required),
            "the corpus does not exercise {required}; found {actions:?}"
        );
    }
}

#[test]
fn the_bridge_rejects_an_expected_pointer_step_the_store_does_not_take() {
    let mut trace = corpus::load("SharedPublication")
        .into_iter()
        .find(|trace| trace.file.ends_with("witness-write-collects.json"))
        .unwrap();
    let step = trace
        .states
        .windows(2)
        .position(|states| infer(&states[0], &states[1]).unwrap().name == "PtrRen")
        .unwrap();
    let previous = trace.states[step]["ptrLive"].clone();
    trace.states[step + 1].insert("ptrLive".to_owned(), previous);
    let departure = super::replay::replay(&trace, Implementation::File)
        .expect_err("the altered trace does not describe the Store");
    assert!(
        departure.step <= step + 1,
        "the altered pointer step was not detected promptly"
    );
}
