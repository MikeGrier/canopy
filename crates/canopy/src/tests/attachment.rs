// Copyright (c) Michael Grier.

use super::*;
use crate::{Agent, CANOPY_SCHEMA_VERSION, SchemaLayer, StoreOptions};
use acadia::coord::file::FileFactory;
use acadia::coord::{
    Access, CoordError, Coordination, CoordinationFactory, Listing, ReaderSafety, TierHold, Waiting,
};
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, VfsErrorKind, When};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use test_timers::mpsc::recv_timeout;

const REGISTRY_DIRECTORY: &str = "registry";
const REGISTRY_LOCK: &str = "registry.lock";
const WAIT_BOUND: Duration = Duration::from_secs(10);

fn participant(vfs: &Arc<dyn Vfs>, root: &Path) -> Box<dyn Coordination> {
    FileFactory::new().open(Arc::clone(vfs), root).unwrap()
}

fn listing(vfs: &Arc<dyn Vfs>, root: &Path) -> Listing {
    participant(vfs, root).list().unwrap()
}

fn open(vfs: &Arc<dyn Vfs>, root: &Path, access: Access, schema: u32) -> Result<Store, StoreError> {
    let agent = Agent::new(
        test_agent().identifier(),
        "opened-version",
        "opened description",
        schema,
    );
    let options = StoreOptions::without_nonce_source(agent);
    match access {
        Access::Read => Store::open_read_only(Arc::clone(vfs), root.to_path_buf(), options),
        Access::Write => {
            Store::open(Arc::clone(vfs), root.to_path_buf(), options).map(|(store, _)| store)
        }
    }
}

fn rewrite_marker(vfs: &Arc<dyn Vfs>, root: &Path, update: impl FnOnce(&mut serde_json::Value)) {
    let path = root.join(MARKER_NAME);
    let mut marker: serde_json::Value = serde_json::from_slice(&vfs.read(&path).unwrap()).unwrap();
    update(&mut marker);
    vfs.write(&path, &serde_json::to_vec(&marker).unwrap())
        .unwrap();
}

fn assert_unheld(vfs: &Arc<dyn Vfs>, root: &Path) {
    assert!(listing(vfs, root).live.is_empty());
    let probe = participant(vfs, root);
    let attached = probe.attach(Access::Write, Waiting::Never).unwrap();
    let _upgrade = attached
        .upgrade(ReaderSafety::Unsafe, Waiting::Never)
        .unwrap();
    let _writer = probe.acquire(Waiting::Never).unwrap();
}

#[test]
fn created_and_reopened_writers_and_readers_register_their_spoken_values() {
    let (vfs, writer) = fresh_store();
    let root = writer.root().to_path_buf();
    let created = listing(&vfs, &root);
    assert_eq!(created.live.len(), 1);
    assert_eq!(created.unknown, 0);
    assert!(!created.may_be_incomplete);
    let payload: serde_json::Value = serde_json::from_slice(&created.live[0].payload).unwrap();
    assert_eq!(payload["agent_version"], test_agent().version());
    drop(writer);
    let writer = open(&vfs, &root, Access::Write, 1).unwrap();
    let reader = open(&vfs, &root, Access::Read, 1).unwrap();
    let found = listing(&vfs, &root);
    assert_eq!(found.live.len(), 2);
    assert_ne!(found.live[0].token, found.live[1].token);
    for entry in found.live {
        let payload: serde_json::Value = serde_json::from_slice(&entry.payload).unwrap();
        assert_eq!(payload["agent_identifier"], test_agent().identifier());
        assert_eq!(payload["agent_version"], "opened-version");
        assert_eq!(payload["canopy_schema"], CANOPY_SCHEMA_VERSION);
        assert_eq!(payload["agent_schema"], 1);
        assert_eq!(payload["process_id"], std::process::id());
        assert_eq!(payload["draining"], false);
        assert_eq!(entry.tiers.reader, Some(TierHold::Shared));
        assert_eq!(
            payload["writer_tier"].is_null(),
            entry.tiers.writer.is_none()
        );
    }
    drop(reader);
    assert_eq!(listing(&vfs, &root).live.len(), 1);
    drop(writer);
    assert_unheld(&vfs, &root);
}

#[test]
fn readers_are_admitted_through_the_inclusive_agent_range_and_writers_only_at_current() {
    for (floor, current) in [(0, 0), (0, 2), (1, 3), (3, 3), (u32::MAX - 1, u32::MAX)] {
        let (vfs, store) = fresh_store();
        let root = store.root().to_path_buf();
        drop(store);
        rewrite_marker(&vfs, &root, |marker| {
            marker["metadata"]["agent_schema"]["current"] = current.into();
            marker["metadata"]["agent_schema"]["oldest_reader"] = floor.into();
        });
        for access in [Access::Read, Access::Write] {
            for spoken in [0, floor, current, current.saturating_add(1), u32::MAX] {
                let admitted = if access == Access::Read {
                    (floor..=current).contains(&spoken)
                } else {
                    spoken == current
                };
                let result = open(&vfs, &root, access, spoken);
                assert_eq!(
                    result.is_ok(),
                    admitted,
                    "{access:?}: {spoken}, {floor}..={current}"
                );
                match result {
                    Ok(store) => {
                        let found = listing(&vfs, &root);
                        assert_eq!(found.live.len(), 1);
                        let payload: serde_json::Value =
                            serde_json::from_slice(&found.live[0].payload).unwrap();
                        assert_eq!(payload["agent_schema"], spoken);
                        drop(store);
                    }
                    Err(error) => assert!(matches!(
                        error,
                        StoreError::SchemaMismatch {
                            layer: SchemaLayer::Agent,
                            ..
                        }
                    )),
                }
                assert_unheld(&vfs, &root);
            }
        }
    }
}

#[test]
fn canopy_schema_admission_is_independent_of_the_agent_schema() {
    for (floor, current) in [(0, 0), (0, 1), (1, 1), (1, 2), (2, 2)] {
        let (vfs, store) = fresh_store();
        let root = store.root().to_path_buf();
        drop(store);
        rewrite_marker(&vfs, &root, |marker| {
            marker["metadata"]["canopy_schema"]["current"] = current.into();
            marker["metadata"]["canopy_schema"]["oldest_reader"] = floor.into();
        });
        for access in [Access::Read, Access::Write] {
            let admitted = if access == Access::Read {
                (floor..=current).contains(&CANOPY_SCHEMA_VERSION)
            } else {
                current == CANOPY_SCHEMA_VERSION
            };
            let result = open(&vfs, &root, access, 1);
            assert_eq!(result.is_ok(), admitted, "{access:?}: {floor}..={current}");
            if let Err(error) = &result {
                assert!(matches!(
                    error,
                    StoreError::SchemaMismatch {
                        layer: SchemaLayer::Canopy,
                        ..
                    }
                ));
            }
            drop(result);
            assert_unheld(&vfs, &root);
        }
    }
}

#[test]
fn each_attachment_lives_until_its_last_clone_is_dropped() {
    for access in [Access::Read, Access::Write] {
        let (vfs, store) = fresh_store();
        let root = store.root().to_path_buf();
        drop(store);
        let store = open(&vfs, &root, access, 1).unwrap();
        let token = listing(&vfs, &root).live[0].token;
        let first = store.clone();
        let last = first.clone();
        drop(store);
        drop(first);
        assert_eq!(listing(&vfs, &root).live[0].token, token);
        let probe = participant(&vfs, &root);
        let attached = probe.attach(Access::Read, Waiting::Never).unwrap();
        assert!(matches!(
            attached.upgrade(ReaderSafety::Unsafe, Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        if access == Access::Read {
            let attached = probe.attach(Access::Read, Waiting::Never).unwrap();
            drop(
                attached
                    .upgrade(ReaderSafety::Safe, Waiting::Never)
                    .unwrap(),
            );
        }
        drop(last);
        assert_unheld(&vfs, &root);
    }
}

#[test]
fn an_opener_rechecks_identity_and_schemas_after_waiting_for_attachment() {
    for access in [Access::Read, Access::Write] {
        for change_identity in [false, true] {
            let mem = MemFs::new();
            let vfs: Arc<dyn Vfs> = Arc::new(mem.another_process());
            let root = PathBuf::from("admission-recheck");
            vfs.create_dir_all(&root).unwrap();
            drop(Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap());
            let probe = participant(&vfs, &root);
            let held = probe
                .attach(Access::Read, Waiting::Never)
                .unwrap()
                .upgrade(ReaderSafety::Unsafe, Waiting::Never)
                .unwrap();
            let (reached, waiting) = mpsc::channel();
            let policy = FaultPolicy::new().inject_matching(
                FaultOp::Lock,
                PathMatcher::predicate(|_| true),
                Fault::observe_with(move |_, _| {
                    let _ = reached.send(());
                }),
                When::Once,
            );
            let opening: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem, policy));
            let path = root.clone();
            let worker = std::thread::spawn(move || open(&opening, &path, access, 1));
            let arrived = recv_timeout(&waiting, WAIT_BOUND);
            rewrite_marker(&vfs, &root, |marker| {
                if change_identity {
                    marker["metadata"]["agent_identifier"] = "different".into();
                } else {
                    marker["metadata"]["agent_schema"]["current"] = 2.into();
                    marker["metadata"]["agent_schema"]["oldest_reader"] = 2.into();
                }
            });
            drop(held);
            let result = worker.join().unwrap();
            arrived.expect("the opener reached admission after its initial marker read");
            if change_identity {
                assert!(matches!(result, Err(StoreError::AgentMismatch { .. })));
            } else {
                assert!(matches!(result, Err(StoreError::SchemaMismatch { .. })));
            }
            assert_unheld(&vfs, &root);
        }
    }
}

#[test]
fn a_failure_to_acquire_attachment_is_not_treated_as_best_effort() {
    let root = PathBuf::from("attachment-failure");
    let mem = MemFs::new();
    let base: Arc<dyn Vfs> = Arc::new(mem.another_process());
    base.create_dir_all(&root).unwrap();
    drop(Store::init_unsourced(Arc::clone(&base), root.clone(), test_config()).unwrap());
    let faulty: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        mem,
        FaultPolicy::new().inject_matching(
            FaultOp::Lock,
            PathMatcher::predicate(|_| true),
            Fault::io_error(),
            When::Always,
        ),
    ));
    for access in [Access::Read, Access::Write] {
        assert!(matches!(
            open(&faulty, &root, access, 1),
            Err(StoreError::Coordination(CoordError::Vfs(_)))
        ));
        assert_unheld(&base, &root);
    }
}

#[test]
fn a_full_registry_does_not_prevent_attachment_and_keeps_listings_incomplete() {
    for access in [Access::Read, Access::Write] {
        let gate = FaultGate::closed();
        let mem = MemFs::new();
        let base: Arc<dyn Vfs> = Arc::new(mem.another_process());
        let root = PathBuf::from("full-registry");
        base.create_dir_all(&root).unwrap();
        drop(Store::init_unsourced(Arc::clone(&base), root.clone(), test_config()).unwrap());
        assert!(!listing(&base, &root).may_be_incomplete);
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
            mem,
            FaultPolicy::new().inject_gated(
                FaultOp::TryLock,
                PathMatcher::file_name(REGISTRY_LOCK),
                Fault::error(VfsErrorKind::Locked),
                When::Always,
                gate.clone(),
            ),
        ));
        gate.open();
        let store = open(&vfs, &root, access, 1).unwrap();
        gate.close();
        let found = listing(&base, &root);
        assert!(found.live.is_empty());
        assert_eq!(found.unknown, 0);
        assert!(found.may_be_incomplete);
        let probe = participant(&base, &root);
        let attached = probe.attach(Access::Read, Waiting::Never).unwrap();
        assert!(matches!(
            attached.upgrade(ReaderSafety::Unsafe, Waiting::Never),
            Err(CoordError::WouldWait)
        ));
        drop(store);
        assert_unheld(&base, &root);
    }
}

#[test]
fn a_registry_write_failure_does_not_refuse_an_admitted_reader() {
    let mem = MemFs::new();
    let base: Arc<dyn Vfs> = Arc::new(mem.another_process());
    let root = PathBuf::from("registry-write-failure");
    base.create_dir_all(&root).unwrap();
    drop(Store::init_unsourced(Arc::clone(&base), root.clone(), test_config()).unwrap());
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        mem,
        FaultPolicy::new().inject_matching(
            FaultOp::Write,
            PathMatcher::has_component(REGISTRY_DIRECTORY),
            Fault::io_error(),
            When::Always,
        ),
    ));
    let store = open(&vfs, &root, Access::Read, 1).unwrap();
    assert!(listing(&base, &root).may_be_incomplete);
    assert!(store.snapshot(|_| ()).is_ok());
    let probe = participant(&base, &root);
    let attached = probe.attach(Access::Read, Waiting::Never).unwrap();
    assert!(matches!(
        attached.upgrade(ReaderSafety::Unsafe, Waiting::Never),
        Err(CoordError::WouldWait)
    ));
    drop(store);
    assert_unheld(&base, &root);
}

#[test]
fn reads_and_writes_do_not_rewrite_a_live_registration() {
    let written = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&written);
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_matching(
            FaultOp::Write,
            PathMatcher::has_component(REGISTRY_DIRECTORY),
            Fault::observe_with(move |_, _| {
                observed.fetch_add(1, Ordering::Relaxed);
            }),
            When::Always,
        ),
    ));
    let root = PathBuf::from("stable-registration");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    assert_eq!(written.load(Ordering::Relaxed), 1);
    let entry = listing(&vfs, &root).live[0].clone();
    for index in 0..10 {
        store
            .write_document_replicated_as(
                name("docs"),
                &doc("one"),
                &body(b"value"),
                &nonce(&format!("n{index}")),
            )
            .unwrap();
        assert!(
            store
                .snapshot(|view| view.body(name("docs"), &doc("one")))
                .unwrap()
                .unwrap()
                .is_some()
        );
        assert_eq!(listing(&vfs, &root).live[0], entry);
    }
    assert_eq!(written.load(Ordering::Relaxed), 1);
}

#[test]
fn unregistering_a_store_still_holds_its_reader_tier() {
    let mem = MemFs::new();
    let base: Arc<dyn Vfs> = Arc::new(mem.another_process());
    let root = PathBuf::from("unregister-order");
    base.create_dir_all(&root).unwrap();
    let probe = participant(&base, &root);
    let seen = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&seen);
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        mem,
        FaultPolicy::new().inject_matching(
            FaultOp::RemoveFile,
            PathMatcher::has_component(REGISTRY_DIRECTORY),
            Fault::observe_with(move |_, _| {
                let attached = probe.attach(Access::Read, Waiting::Never).unwrap();
                assert!(matches!(
                    attached.upgrade(ReaderSafety::Unsafe, Waiting::Never),
                    Err(CoordError::WouldWait)
                ));
                observed.store(true, Ordering::Release);
            }),
            When::Once,
        ),
    ));
    drop(Store::init_unsourced(vfs, root.clone(), test_config()).unwrap());
    assert!(seen.load(Ordering::Acquire));
    assert_unheld(&base, &root);
}
