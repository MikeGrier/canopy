// Copyright (c) Michael Grier.
// Split from blob.rs at 844ebb37.

//! `CVS-7`: blob read -- verification, fall-through, and replica rotation.

use super::*;

// -- reading (`D316`, `D317`, `CVS-7`) -----------------------------------------

#[test]
fn a_read_returns_the_bytes_that_were_ingested() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    let name = ingest(&store, b"round trip", "n1");
    let read = store.read_blob(&reference(&store, &name)).expect("read");

    assert_eq!(read.bytes(), b"round trip");
    assert!(read.was_first_choice(), "an intact store needs no fallback");
}

#[test]
fn a_read_derives_nothing_and_enumerates_nothing() {
    // `D321`: the reference carries the shard and the name, so a read that was handed one
    // for a document it has never seen still finds the blob. If the read recomputed
    // anything from the node and document names, it could not -- it is not given them.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    let name = ingest(&store, b"no derivation", "n1");
    let path = name.relative_path(store.shard_width());
    let parsed = crate::naming::BlobRef::parse(&path).expect("parse");

    assert_eq!(
        store.read_blob(&parsed).expect("read").bytes(),
        b"no derivation"
    );
}

#[test]
fn a_corrupt_replica_is_identified_by_itself_and_the_read_falls_through() {
    // `D316`: a bad copy is recognised by examining it -- no fan-out, no comparison against
    // another replica, no vote. Corrupting a majority changes nothing, which is the
    // property a quorum read does not have.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"one good copy", "n1");
    let width = store.shard_width();
    // Rot everything the read will try *before* its last choice, so the fall-through is
    // forced rather than incidental. Which partitions those are depends on the rotation,
    // not on their index -- a test that assumed partition 0 was tried first would be
    // asserting an accident.
    let order = rotation_order(&store, &name);
    for &index in &order[..order.len() - 1] {
        vfs.write(&blob_path(&root, index, &name, width), b"rotted")
            .unwrap();
    }

    let read = store.read_blob(&reference(&store, &name)).expect("read");
    assert_eq!(read.bytes(), b"one good copy");
    assert_eq!(read.served_by(), *order.last().unwrap());
    assert_eq!(read.corrupt(), &order[..order.len() - 1]);
    assert!(read.missing().is_empty());
}

#[test]
fn a_read_stops_at_the_first_copy_that_verifies() {
    // The complement of the fall-through test: verification is per-copy, so a read that
    // finds its preferred replica intact never touches the others -- including ones that
    // are corrupt. There is no fan-out (`D316`).
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"first choice", "n1");
    let width = store.shard_width();
    let order = rotation_order(&store, &name);
    for &index in &order[1..] {
        vfs.write(&blob_path(&root, index, &name, width), b"rotted")
            .unwrap();
    }

    let read = store.read_blob(&reference(&store, &name)).expect("read");
    assert_eq!(read.bytes(), b"first choice");
    assert_eq!(read.served_by(), order[0]);
    assert!(
        read.was_first_choice(),
        "a corrupt copy the read never looked at must not be reported"
    );
}

#[test]
fn a_missing_replica_is_stepped_over_and_reported_separately_from_a_corrupt_one() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"mixed faults", "n1");
    let width = store.shard_width();
    let order = rotation_order(&store, &name);
    vfs.remove_file(&blob_path(&root, order[0], &name, width))
        .unwrap();
    vfs.write(&blob_path(&root, order[1], &name, width), b"rot")
        .unwrap();

    let read = store.read_blob(&reference(&store, &name)).expect("read");
    assert_eq!(read.bytes(), b"mixed faults");
    assert_eq!(read.missing(), &[order[0]]);
    assert_eq!(read.corrupt(), &[order[1]]);
    assert_eq!(read.served_by(), order[2]);
}

/// The order this store will try partitions for `name` at the current epoch.
fn rotation_order(store: &Store, name: &BlobName) -> Vec<usize> {
    let replicas = store.blob_replica_count();
    // `blob_epoch` advances the counter, so read it once and rebuild the same arithmetic
    // the read will use rather than calling it repeatedly.
    let epoch = store.blob_epoch();
    let start = (name.path_hash().rotation_seed().wrapping_add(epoch) % replicas as u64) as usize;
    (0..replicas)
        .map(|step| (start + step) % replicas)
        .collect()
}

#[test]
fn every_replica_corrupt_is_reported_apart_from_every_replica_missing() {
    // The two failures have different remedies, so the error distinguishes them rather
    // than collapsing both into "unavailable".
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    let width = store.shard_width();

    let rotted = ingest(&store, b"all rotted", "n1");
    for index in 0..store.blob_replica_count() {
        vfs.write(&blob_path(&root, index, &rotted, width), b"x")
            .unwrap();
    }
    match store.read_blob(&reference(&store, &rotted)) {
        Err(StoreError::BlobUnavailable {
            missing, corrupt, ..
        }) => {
            assert_eq!(missing, 0);
            assert_eq!(corrupt, 3);
        }
        other => panic!("expected an unavailable blob, got {other:?}"),
    }

    let gone = ingest(&store, b"all gone", "n2");
    for index in 0..store.blob_replica_count() {
        vfs.remove_file(&blob_path(&root, index, &gone, width))
            .unwrap();
    }
    match store.read_blob(&reference(&store, &gone)) {
        Err(StoreError::BlobUnavailable {
            missing, corrupt, ..
        }) => {
            assert_eq!(missing, 3);
            assert_eq!(corrupt, 0);
        }
        other => panic!("expected an unavailable blob, got {other:?}"),
    }
}

#[test]
fn verification_costs_no_extra_read_because_the_expectation_is_in_the_name() {
    // Not a timing assertion -- a structural one. The digest a read checks against comes
    // from the reference it was handed, so a read of a blob whose file has been replaced
    // fails on content rather than on some second lookup.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");

    let name = ingest(&store, b"expected", "n1");
    let reference = reference(&store, &name);
    assert_eq!(reference.name().digest(), ContentDigest::of(b"expected"));

    let width = store.shard_width();
    for index in 0..store.blob_replica_count() {
        vfs.write(&blob_path(&root, index, &name, width), b"substituted")
            .unwrap();
    }
    assert!(matches!(
        store.read_blob(&reference),
        Err(StoreError::BlobUnavailable { corrupt: 3, .. })
    ));
}

// -- rotation (`D317`) ---------------------------------------------------------

#[test]
fn selection_is_stable_within_an_epoch() {
    // The reason the epoch is coarse: two reads of one document inside a write window
    // must not land on different replicas and see the value move backwards.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    let name = ingest(&store, b"stable", "n1");
    let reference = reference(&store, &name);

    let first = store.read_blob(&reference).unwrap().served_by();
    for _ in 0..64 {
        assert_eq!(
            store.read_blob(&reference).unwrap().served_by(),
            first,
            "selection moved inside one epoch"
        );
    }
}

#[test]
fn selection_moves_on_when_the_epoch_does_and_sweeps_every_replica() {
    // Rotation is what makes every read a probe: without it, verification would re-check
    // the same copy forever and a rotted replica would survive until the next scrub.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    let name = ingest(&store, b"swept", "n1");
    let reference = reference(&store, &name);
    let replicas = store.blob_replica_count();

    let mut seen = std::collections::BTreeSet::new();
    // Enough reads to cross every epoch boundary a full sweep needs.
    for _ in 0..(crate::store::BLOB_EPOCH_READS as usize * replicas + replicas) {
        seen.insert(store.read_blob(&reference).unwrap().served_by());
    }
    assert_eq!(
        seen.len(),
        replicas,
        "every replica must be probed over time, saw {seen:?}"
    );
}

#[test]
fn two_documents_do_not_all_prefer_the_same_replica() {
    // Selection is spread by the path hash, so load does not pile onto one partition.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    let mut served = std::collections::BTreeSet::new();
    for i in 0..32 {
        let name = store
            .ingest_blob(
                &node_of("TICKETS/A"),
                &doc_of(&format!("record{i}.json")),
                Artifact::Body,
                &b"body"[..],
                &nonce_of(&format!("n{i}")),
            )
            .unwrap();
        served.insert(
            store
                .read_blob(&reference(&store, &name))
                .unwrap()
                .served_by(),
        );
    }
    assert!(
        served.len() > 1,
        "every document preferred the same partition: {served:?}"
    );
}

#[test]
fn clones_of_a_store_share_one_epoch() {
    // Clones are one store, not several: a private counter per clone would never reach an
    // epoch boundary and rotation would stop sweeping.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    let clone = store.clone();

    let before = store.blob_epoch();
    for _ in 0..crate::store::BLOB_EPOCH_READS {
        let _ = clone.blob_epoch();
    }
    assert!(
        store.blob_epoch() > before,
        "reads through a clone did not advance the shared epoch"
    );
}

// -- references ----------------------------------------------------------------

#[test]
fn a_reference_whose_shard_disagrees_with_its_name_is_refused() {
    // `D321`'s cheapest cross-check, and the only one available before any I/O: the shard
    // and the name carry the same information, so a disagreement means the reference was
    // assembled wrongly or damaged in transit.
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    let name = ingest(&store, b"cross-checked", "n1");

    let wrong = format!("ZZ/{}", name.file_name());
    assert_eq!(
        crate::naming::BlobRef::parse(&wrong),
        Err(crate::naming::BlobNameError::ShardMismatch)
    );
    assert_eq!(
        crate::naming::BlobRef::parse(&name.file_name()),
        Err(crate::naming::BlobNameError::MissingShard)
    );
}

#[test]
fn a_reference_round_trips_through_its_path() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    let name = ingest(&store, b"round trip", "n1");

    let path = name.relative_path(store.shard_width());
    let parsed = crate::naming::BlobRef::parse(&path).expect("parse");
    assert_eq!(parsed.relative_path(), path);
    assert_eq!(parsed.name(), name);
}

#[test]
fn a_repair_naming_a_replica_the_store_does_not_have_is_refused() {
    let (vfs, root) = fresh_root();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");
    let name = ingest(&store, b"in range", "n1");
    let count = store.blob_replica_count();

    for (missing, corrupt) in [(vec![count], vec![]), (vec![], vec![count + 3])] {
        let refused = store
            .repair_blob_as(
                &reference(&store, &name),
                b"in range",
                &missing,
                &corrupt,
                &nonce_of("r1"),
            )
            .expect_err("an out-of-range index is refused");
        match refused {
            StoreError::ReplicaIndexOutOfRange {
                index,
                count: reported,
            } => {
                assert!(index >= count);
                assert_eq!(reported, count);
            }
            other => panic!("expected ReplicaIndexOutOfRange, got {other:?}"),
        }
    }
    assert!(
        store
            .verify_blob(&reference(&store, &name))
            .unwrap()
            .is_whole()
    );
}

#[test]
fn a_sequestered_copy_survives_a_crash() {
    // The quarantine is created by the repair that first needs it; its own name must be made
    // durable, or a crash loses evidence the repair reported as kept.
    let mem = Arc::new(acadia::MemFs::new());
    let vfs: Arc<dyn Vfs> = mem.clone();
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(vfs.clone(), root.clone(), test_config()).expect("init");
    let name = ingest(&store, b"good bytes", "n1");
    let width = store.shard_width();
    vfs.write(&blob_path(&root, 1, &name, width), b"rotten")
        .unwrap();

    let outcome = store
        .repair_blob_as(
            &reference(&store, &name),
            b"good bytes",
            &[],
            &[1],
            &nonce_of("r1"),
        )
        .unwrap();
    assert_eq!(outcome.sequestered(), [1]);

    mem.simulate_crash();
    let quarantine = root
        .join(BLOB_TREE_DIR)
        .join("p1")
        .join(crate::store::QUARANTINE_DIR);
    let kept = vfs.read_dir(&quarantine).expect("the quarantine survived");
    assert_eq!(kept.len(), 1);
}

#[test]
fn a_blob_repair_is_visible_to_the_modification_count_while_and_after_it_runs() {
    // A repair moves replicas a reader can see, so a reader straddling it must not be told
    // nothing happened.
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let samples: Arc<std::sync::Mutex<Vec<u64>>> = Arc::default();
    let handle: Arc<std::sync::OnceLock<Store>> = Arc::default();
    let gate = FaultGate::closed();
    let (recorded, probe) = (Arc::clone(&samples), Arc::clone(&handle));
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::file_name_prefix("blob.tmp."),
            Fault::observe_with(move |_, _| {
                if let Some(store) = probe.get() {
                    recorded.lock().unwrap().push(store.modifications());
                }
            }),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("repair-count");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    let _ = handle.set(store.clone());
    let name = ingest(&store, b"good bytes", "n1");
    vfs.remove_file(&blob_path(&root, 1, &name, store.shard_width()))
        .unwrap();

    let before = store.modifications();
    gate.open();
    let outcome = store
        .repair_blob_as(
            &reference(&store, &name),
            b"good bytes",
            &[1],
            &[],
            &nonce_of("r1"),
        )
        .unwrap();
    gate.close();
    let after = store.modifications();

    assert_eq!(outcome.replaced(), [1]);
    let samples = samples.lock().unwrap().clone();
    assert_eq!(samples.len(), 1);
    assert!(!Store::undisturbed(before, samples[0]));
    assert!(
        !Store::undisturbed(samples[0], samples[0]),
        "odd while repairing"
    );
    assert!(!Store::undisturbed(before, after));
    assert!(Store::undisturbed(after, after));
}

#[test]
fn a_failed_blob_repair_still_moves_the_count_and_leaves_it_settled() {
    use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, PathMatcher, When};
    let gate = FaultGate::closed();
    let vfs: Arc<dyn Vfs> = Arc::new(FaultyFs::new(
        acadia::MemFs::new(),
        FaultPolicy::new().inject_gated(
            FaultOp::Rename,
            PathMatcher::file_name_prefix("blob.tmp."),
            Fault::io_error(),
            When::Always,
            gate.clone(),
        ),
    ));
    let root = PathBuf::from("repair-fails");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(Arc::clone(&vfs), root.clone(), test_config()).unwrap();
    let name = ingest(&store, b"good bytes", "n1");
    vfs.remove_file(&blob_path(&root, 1, &name, store.shard_width()))
        .unwrap();

    let before = store.modifications();
    gate.open();
    store
        .repair_blob_as(
            &reference(&store, &name),
            b"good bytes",
            &[1],
            &[],
            &nonce_of("r1"),
        )
        .expect_err("the replacement rename fails");
    gate.close();
    let after = store.modifications();

    assert!(
        !Store::undisturbed(before, after),
        "a partial repair still moved the count"
    );
    assert!(
        Store::undisturbed(after, after),
        "the count is settled after the failure"
    );
}

#[test]
fn a_repair_at_another_shard_width_is_refused_before_anything_moves() {
    // Repairing the store's own path would report success for a reference whose reads still
    // look elsewhere, and a corrupt copy would be sequestered from one path but replaced at another.
    let (vfs, store) = fresh_store();
    let root = store.root().to_path_buf();
    let name = ingest(&store, b"good bytes", "n1");
    let own = blob_path(&root, 0, &name, store.shard_width());
    vfs.write(&own, b"rotted").unwrap();
    let before = store.modifications();

    for width in [1, store.shard_width() + 1] {
        let foreign = crate::BlobRef::new(name, width).unwrap();
        let refused = store.repair_blob_as(&foreign, b"good bytes", &[1], &[0], &nonce_of("r1"));
        assert!(
            matches!(
                refused,
                Err(StoreError::ReferenceShardWidth { expected, found })
                    if expected == store.shard_width() && found == width
            ),
            "width {width}: got {refused:?}"
        );
    }
    assert_eq!(
        vfs.read(&own).unwrap(),
        b"rotted",
        "the corrupt copy was not touched"
    );
    assert!(Store::undisturbed(before, store.modifications()));
    let verification = store.verify_blob(&reference(&store, &name)).unwrap();
    assert_eq!(verification.corrupt(), [0]);
}
