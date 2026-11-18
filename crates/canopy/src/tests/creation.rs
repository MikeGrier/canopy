// Copyright (c) Michael Grier.

//! Creation is idempotent (DESIGN-NOTES.md "Store creation is idempotent"): creators of one
//! identity end with one store whenever they run, a crash at any step leaves a creation the next
//! one finishes, and a root holding anything else is refused before and under the exclusion.

use super::*;
use acadia::{Fault, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};

/// How many creators race in each round.
const CREATORS: usize = 8;

/// The file-based implementation's creation exclusion, which a creator waits for.
const CREATION_LOCK: &str = ".coord-creation.lock";

/// A root on `mem` whose own name is durable, so only what creation makes is at stake.
fn durable_root(mem: &MemFs) -> PathBuf {
    let root = PathBuf::from("store");
    mem.create_dir_all(&root).unwrap();
    mem.fsync(&root).unwrap();
    root
}

#[test]
fn identical_concurrent_creators_make_one_store_and_none_loses_anothers_write() {
    for round in 0..12 {
        let (vfs, root) = fresh_root();
        let barrier = Arc::new(std::sync::Barrier::new(CREATORS));
        let creators: Vec<_> = (0..CREATORS)
            .map(|i| {
                let (vfs, root, barrier) = (Arc::clone(&vfs), root.clone(), Arc::clone(&barrier));
                thread::spawn(move || {
                    barrier.wait();
                    let store = Store::init_unsourced(vfs, root, test_config())?;
                    store
                        .update_document_as(
                            Swap::new(
                                name("creations"),
                                doc(&format!("by-{i}")),
                                body(format!("creator {i}").as_bytes()),
                            ),
                            &nonce(&format!("creator-{i}")),
                        )
                        .map(|_| store)
                })
            })
            .collect();
        for (i, creator) in creators.into_iter().enumerate() {
            if let Err(error) = creator.join().unwrap() {
                panic!("round {round}: creator {i}: {error:?}");
            }
        }
        let again = Store::init_unsourced(vfs, root, test_config()).unwrap();
        for i in 0..CREATORS {
            assert_eq!(
                again
                    .read_body(name("creations"), &doc(&format!("by-{i}")))
                    .unwrap(),
                Some(body(format!("creator {i}").as_bytes())),
                "round {round}: creator {i}"
            );
        }
    }
}

#[test]
fn a_crash_at_any_step_of_creation_leaves_a_creation_an_identical_creator_finishes() {
    for op in [FaultOp::Fsync, FaultOp::CreateDirAll, FaultOp::PublishNew] {
        for n in 1.. {
            let mem = MemFs::new();
            let root = durable_root(&mem);
            let policy = FaultPolicy::new().inject_matching(
                op,
                PathMatcher::predicate(|_| true),
                Fault::io_error(),
                When::OnCall(n),
            );
            let faulty: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
            let cut = Store::init_unsourced(faulty, root.clone(), test_config());
            let finished = cut.is_ok();
            drop(cut);
            mem.simulate_crash();
            // A store whose marker survived the crash is a store that crashed, so its writers
            // recover first; before the marker, the resuming creator vouches for the root itself.
            let made = mem.exists(&root.join(MARKER_NAME)).unwrap();
            let store =
                Store::init_unsourced(Arc::new(mem.another_process()), root.clone(), test_config())
                    .unwrap_or_else(|error| panic!("{op:?} call {n}: {error:?}"));
            if made {
                store
                    .recover()
                    .unwrap_or_else(|error| panic!("{op:?} call {n}: {error:?}"))
                    .agent_stamp
                    .expect("agent version stamp");
            }
            store
                .update_document_as(
                    Swap::new(name("after"), doc("crash"), body(b"written")),
                    &nonce("after-crash"),
                )
                .unwrap_or_else(|error| panic!("{op:?} call {n}: {error:?}"));
            drop(store);
            mem.simulate_crash();
            let reopened = Store::open_unsourced(Arc::new(mem.another_process()), root).unwrap();
            assert_eq!(
                reopened.read_body(name("after"), &doc("crash")).unwrap(),
                Some(body(b"written")),
                "{op:?} call {n}"
            );
            if finished {
                break;
            }
        }
    }
}

#[test]
fn a_stray_entry_or_another_identity_refuses_a_creator_before_the_exclusion() {
    let (vfs, root) = fresh_root();
    vfs.write(&root.join("stray"), b"not a store").unwrap();
    let refused = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config())
        .expect_err("a stray entry occupies the root");
    assert!(
        matches!(&refused, StoreError::NotEmpty { entry, .. } if entry == "stray"),
        "{refused:?}"
    );
    assert_eq!(vfs.read_dir(&root).unwrap().len(), 1, "nothing was made");

    let (vfs, root) = fresh_root();
    drop(Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap());
    let other = GenesisConfig::new(StoreId::new("another-store").unwrap(), 12_345);
    let refused = Store::init_unsourced(Arc::clone(&vfs), root, other)
        .expect_err("another identity's store refuses the creator");
    assert!(
        matches!(refused, StoreError::AlreadyInitialized { .. }),
        "{refused:?}"
    );
}

#[test]
fn a_stray_entry_or_another_identity_that_appears_while_a_creator_waits_refuses_it() {
    let other = || GenesisConfig::new(StoreId::new("another-store").unwrap(), 12_345);
    for appears in ["stray", "another identity"] {
        let mem = MemFs::new();
        let root = durable_root(&mem);
        let (intruder, intruder_root) = (mem.another_process(), root.clone());
        let policy = FaultPolicy::new().inject(
            FaultOp::Lock,
            root.join(CREATION_LOCK),
            Fault::observe_with(move |_, _| {
                if appears == "stray" {
                    intruder.write(&intruder_root.join("stray"), b"x").unwrap();
                } else {
                    drop(
                        Store::init_unsourced(
                            Arc::new(intruder.another_process()),
                            intruder_root.clone(),
                            other(),
                        )
                        .unwrap(),
                    );
                }
            }),
            When::Once,
        );
        let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
        let refused = Store::init_unsourced(vfs, root.clone(), test_config())
            .expect_err("what appeared refuses the creator");
        if appears == "stray" {
            assert!(
                matches!(&refused, StoreError::NotEmpty { entry, .. } if entry == "stray"),
                "{refused:?}"
            );
            assert!(!mem.exists(&root.join(MARKER_NAME)).unwrap());
        } else {
            assert!(
                matches!(refused, StoreError::AlreadyInitialized { .. }),
                "{refused:?}"
            );
            let survivor =
                Store::open_unsourced(Arc::new(mem.another_process()), root.clone()).unwrap();
            assert_eq!(survivor.store_id(), "another-store", "not made over");
        }
    }
}

#[test]
fn creators_that_differ_only_in_provenance_find_one_store() {
    let (vfs, root) = fresh_root();
    let first = Store::init(
        Arc::clone(&vfs),
        root.clone(),
        test_config(),
        StoreOptions::without_nonce_source(test_agent()),
    )
    .unwrap();
    let marker = vfs.read(&root.join(MARKER_NAME)).unwrap();
    drop(first);
    for (agent, created_at) in [
        (
            crate::Agent::new("canopy-tests", "another version", "another description", 1),
            12_345,
        ),
        (test_agent(), 99_999),
    ] {
        let config = GenesisConfig::new(StoreId::new("test-store").unwrap(), created_at);
        let store = Store::init(
            Arc::clone(&vfs),
            root.clone(),
            config,
            StoreOptions::without_nonce_source(agent),
        )
        .unwrap();
        assert_eq!(store.created_at_unix_ms(), 12_345, "the maker's provenance");
        assert_eq!(vfs.read(&root.join(MARKER_NAME)).unwrap(), marker);
    }
}
