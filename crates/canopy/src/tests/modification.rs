// Copyright (c) Michael Grier.

//! The modification count brackets whole logical operations (`D335`): a sampler never sees a
//! settled count between the steps of one transaction or one write, and a transaction that
//! ends unresolved leaves the count odd.

use super::*;
use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};

/// A store whose namespace renames each record the modification count at that moment.
fn sampling_store(root: &str) -> (Store, FaultGate, Arc<std::sync::Mutex<Vec<u64>>>) {
    let samples: Arc<std::sync::Mutex<Vec<u64>>> = Arc::default();
    let handle: Arc<std::sync::OnceLock<Store>> = Arc::default();
    let gate = FaultGate::closed();
    let (recorded, store_for_probe) = (Arc::clone(&samples), Arc::clone(&handle));
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::has_component(NAMESPACE_TREE_DIR),
            Fault::observe_with(move |_, _| {
                if let Some(store) = store_for_probe.get() {
                    recorded.lock().unwrap().push(store.modifications());
                }
            }),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from(root);
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(vfs, root, test_config()).unwrap();
    let _ = handle.set(store.clone());
    (store, gate, samples)
}

#[test]
fn a_transaction_reads_as_one_modification_across_all_its_swaps() {
    let (store, gate, samples) = sampling_store("txn-bracket");
    let swaps = SwapSet::new([
        Swap::new(name("alpha"), doc("one"), body(b"1")),
        Swap::new(name("beta"), doc("two"), body(b"2")),
    ])
    .unwrap();

    let before = store.modifications();
    gate.open();
    let outcome = store.apply_transaction_as(&swaps, &nonce("t1")).unwrap();
    gate.close();
    let after = store.modifications();

    assert!(outcome.is_committed());
    let samples = samples.lock().unwrap().clone();
    assert!(
        samples.len() >= 2 * store.pointer_replica_count(),
        "every replica of both swaps was sampled: {samples:?}"
    );
    for mid in &samples {
        assert!(
            !Store::undisturbed(*mid, *mid),
            "a sample between two swaps of one transaction read as settled: {mid}"
        );
        assert_eq!(
            *mid,
            before + 1,
            "the whole transaction is one modification"
        );
    }
    assert_eq!(
        after,
        before + 2,
        "one opening and one closing raise, however many swaps"
    );
}

#[test]
fn an_unlogged_write_reads_as_one_modification_from_ingest_to_collection() {
    let (store, gate, samples) = sampling_store("write-bracket");
    store
        .write_document_replicated_as(name("alpha"), &doc("one"), &body(b"first"), &nonce("w0"))
        .unwrap();

    let before = store.modifications();
    gate.open();
    store
        .write_document_replicated_as(name("alpha"), &doc("one"), &body(b"second"), &nonce("w1"))
        .unwrap();
    gate.close();
    let after = store.modifications();

    let samples = samples.lock().unwrap().clone();
    assert_eq!(samples.len(), store.pointer_replica_count());
    assert!(samples.iter().all(|mid| *mid == before + 1), "{samples:?}");
    assert_eq!(after, before + 2);
}

#[test]
fn nested_modifications_move_the_count_only_at_the_outermost_edges() {
    let (_vfs, store) = fresh_store();
    let start = store.modifications();
    {
        let _outer = store.begin_modification();
        assert_eq!(store.modifications(), start + 1);
        {
            let _inner = store.begin_modification();
            assert_eq!(store.modifications(), start + 1);
        }
        assert_eq!(
            store.modifications(),
            start + 1,
            "an inner modification ending must not restore even parity early"
        );
    }
    assert_eq!(store.modifications(), start + 2);
    assert!(Store::undisturbed(
        store.modifications(),
        store.modifications()
    ));
}

#[test]
fn successive_operations_each_move_the_count_by_two() {
    let (_vfs, store) = fresh_store();
    let start = store.modifications();
    for i in 0..10 {
        store
            .write_document_replicated_as(
                name("alpha"),
                &doc("one"),
                &body(format!("v{i}").as_bytes()),
                &nonce(&format!("s{i}")),
            )
            .unwrap();
        assert_eq!(store.modifications(), start + 2 * (i + 1));
    }
}

#[test]
fn a_rejected_transaction_still_reads_as_settled_afterwards() {
    let (_vfs, store) = fresh_store();
    let swaps = SwapSet::new([Swap::conditional(
        name("alpha"),
        doc("one"),
        body(b"x"),
        crate::ContentDigest::of(b"not what is there"),
    )])
    .unwrap();
    let before = store.modifications();
    let outcome = store.apply_transaction_as(&swaps, &nonce("r1")).unwrap();
    assert!(outcome.is_rejected());
    let after = store.modifications();
    assert!(Store::undisturbed(after, after));
    assert!(after >= before);
}

#[test]
fn a_transaction_that_ends_unresolved_leaves_the_count_odd() {
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            namespace_tree_staging(),
            Fault::io_error(),
            When::Once,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("unresolved");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(vfs, root, test_config()).unwrap();
    let swaps = SwapSet::new([Swap::new(name("alpha"), doc("pin"), body(b"committed"))]).unwrap();

    gate.open();
    assert!(store.apply_transaction_as(&swaps, &nonce("u1")).is_err());
    gate.close();
    assert!(store.recovery_required());

    let after = store.modifications();
    assert!(
        !Store::undisturbed(after, after),
        "an unresolved transaction must not read as settled: {after}"
    );
    // A later modification under the fence must not flip it back to even either.
    {
        let _repair = store.begin_modification();
    }
    assert_eq!(store.modifications(), after);
}
