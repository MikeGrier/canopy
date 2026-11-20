// Copyright (c) Michael Grier.

//! The store's own nonce: unset by creation, set by its first user under writer exclusion.

use super::*;
use crate::{Nonce, StoreOptions};
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The record's name at the store root, which is part of the on-disk layout.
const RECORD: &str = ".store-nonce";

/// Options whose nonce source draws `<prefix>-1`, `<prefix>-2`, ... and counts its draws.
fn counted(prefix: &'static str) -> (StoreOptions, Arc<AtomicUsize>) {
    let drawn = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&drawn);
    let options = StoreOptions::with_nonce_source(test_agent(), move || {
        let n = counter.fetch_add(1, Ordering::AcqRel) + 1;
        Nonce::new(&format!("{prefix}-{n}")).unwrap()
    });
    (options, drawn)
}

/// A store over `mem` at a fresh root, made through one process's view of it.
fn store_on(mem: &MemFs, prefix: &'static str) -> (Store, PathBuf) {
    let root = PathBuf::from("store");
    mem.create_dir_all(&root).unwrap();
    let store = Store::init(
        Arc::new(mem.another_process()),
        root.clone(),
        test_config(),
        counted(prefix).0,
    )
    .unwrap();
    (store, root)
}

#[test]
fn creation_leaves_the_nonce_unset_and_the_first_call_sets_it_once() {
    let (vfs, root) = fresh_root();
    let (options, drawn) = counted("first");
    let store = Store::init(Arc::clone(&vfs), root.clone(), test_config(), options).unwrap();
    assert!(!vfs.exists(&root.join(RECORD)).unwrap());
    let before = drawn.load(Ordering::Acquire);
    let nonce = store.store_nonce().unwrap();
    assert_eq!(drawn.load(Ordering::Acquire), before + 1);
    for _ in 0..10 {
        assert_eq!(store.store_nonce().unwrap(), nonce);
    }
    assert_eq!(drawn.load(Ordering::Acquire), before + 1, "drawn once");
    assert_eq!(
        vfs.read(&root.join(RECORD)).unwrap(),
        nonce.as_str().as_bytes()
    );
}

#[test]
fn handles_whose_first_uses_race_agree_on_one_nonce() {
    for round in 0..8 {
        let mem = MemFs::new();
        let (made, root) = store_on(&mem, "init");
        drop(made);
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let racers: Vec<_> = ["a", "b", "c", "d"]
            .into_iter()
            .map(|prefix| {
                let vfs: Arc<dyn Vfs> = Arc::new(mem.another_process());
                let (root, barrier) = (root.clone(), Arc::clone(&barrier));
                std::thread::spawn(move || {
                    let (store, _) = Store::open(vfs, root, counted(prefix).0).unwrap();
                    barrier.wait();
                    store.store_nonce().unwrap()
                })
            })
            .collect();
        let nonces: Vec<Nonce> = racers
            .into_iter()
            .map(|racer| racer.join().unwrap())
            .collect();
        assert!(
            nonces.iter().all(|nonce| *nonce == nonces[0]),
            "round {round}: {nonces:?}"
        );
    }
}

#[test]
fn a_handle_without_a_nonce_source_cannot_set_it_but_reads_it_once_set() {
    let (vfs, root) = fresh_root();
    let unsourced = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    assert!(matches!(
        unsourced.store_nonce(),
        Err(StoreError::NoNonceSource)
    ));
    assert!(!vfs.exists(&root.join(RECORD)).unwrap());
    let (sourced, _) = Store::open(Arc::clone(&vfs), root.clone(), counted("set").0).unwrap();
    let nonce = sourced.store_nonce().unwrap();
    assert_eq!(unsourced.store_nonce().unwrap(), nonce);
}

#[test]
fn a_read_only_handle_is_refused() {
    let (vfs, root) = fresh_root();
    drop(
        Store::init(
            Arc::clone(&vfs),
            root.clone(),
            test_config(),
            counted("w").0,
        )
        .unwrap(),
    );
    let reader = Store::open_read_only(vfs, root, counted("r").0).unwrap();
    assert!(matches!(
        reader.store_nonce(),
        Err(StoreError::ReadOnly { .. })
    ));
}

#[test]
fn a_nonce_whose_record_was_not_made_durable_is_not_returned_and_a_crash_unsets_it() {
    let mem = MemFs::new();
    let root = PathBuf::from("store");
    mem.create_dir_all(&root).unwrap();
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Fsync,
        PathMatcher::exact(&root),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
    let store = Store::init(vfs, root.clone(), test_config(), counted("lost").0).unwrap();
    gate.open();
    assert!(store.store_nonce().is_err());
    drop(store);
    let mem = mem.simulate_restart();
    assert!(!mem.exists(&root.join(RECORD)).unwrap());
    let (reopened, _) = Store::open(
        Arc::new(mem.another_process()),
        root.clone(),
        counted("kept").0,
    )
    .unwrap();
    assert!(
        reopened
            .store_nonce()
            .unwrap()
            .as_str()
            .starts_with("kept-"),
        "the lost value is never seen"
    );
}

#[test]
fn a_record_left_undurable_is_made_durable_by_the_next_call() {
    let mem = MemFs::new();
    let root = PathBuf::from("store");
    mem.create_dir_all(&root).unwrap();
    let gate = FaultGate::closed();
    let policy = FaultPolicy::new().inject_gated(
        FaultOp::Fsync,
        PathMatcher::exact(&root),
        Fault::io_error(),
        When::Once,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(mem.another_process(), policy));
    let store = Store::init(vfs, root.clone(), test_config(), counted("left").0).unwrap();
    gate.open();
    assert!(store.store_nonce().is_err());
    let nonce = store.store_nonce().unwrap();
    drop(store);
    let mem = mem.simulate_restart();
    let (reopened, _) = Store::open(
        Arc::new(mem.another_process()),
        root.clone(),
        counted("other").0,
    )
    .unwrap();
    assert_eq!(reopened.store_nonce().unwrap(), nonce);
}

#[test]
fn a_returned_nonce_survives_a_crash() {
    let mem = MemFs::new();
    let (store, root) = store_on(&mem, "durable");
    let nonce = store.store_nonce().unwrap();
    drop(store);
    let mem = mem.simulate_restart();
    let (reopened, _) =
        Store::open(Arc::new(mem.another_process()), root, counted("other").0).unwrap();
    assert_eq!(reopened.store_nonce().unwrap(), nonce);
}

#[test]
fn a_store_made_again_at_its_path_has_another_nonce() {
    let (vfs, root) = fresh_root();
    let first = Store::init(
        Arc::clone(&vfs),
        root.clone(),
        test_config(),
        counted("one").0,
    )
    .unwrap();
    let nonce = first.store_nonce().unwrap();
    drop(first);
    vfs.remove_dir_all(&root).unwrap();
    vfs.create_dir_all(&root).unwrap();
    let again = Store::init(Arc::clone(&vfs), root, test_config(), counted("two").0).unwrap();
    assert_ne!(again.store_nonce().unwrap(), nonce);
}

#[test]
fn a_record_that_is_not_a_nonce_is_refused() {
    let (vfs, root) = fresh_root();
    let store = Store::init(
        Arc::clone(&vfs),
        root.clone(),
        test_config(),
        counted("m").0,
    )
    .unwrap();
    for bad in [&b""[..], b"not a nonce", b"has.dot", &[0xff, 0xfe]] {
        vfs.write(&root.join(RECORD), bad).unwrap();
        assert!(
            matches!(store.store_nonce(), Err(StoreError::MalformedMarker { .. })),
            "{bad:?}"
        );
    }
}
