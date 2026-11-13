// Copyright (c) Michael Grier.

//! `D339`/`D341`: the repair backlog -- what a read reports, and what happens when it fills.

use super::*;

/// A distinct blob reference per `seed`, without touching the disk.
///
/// The backlog stores identities, so a test about its bookkeeping needs distinct names and
/// nothing behind them.
fn distinct_reference(store: &Store, seed: usize) -> crate::naming::BlobRef {
    let name = BlobName::new(
        PathHash::of(&node_of("TICKETS/A"), &doc_of(&format!("d{seed}.json"))),
        ContentDigest::of(b"whatever"),
        Artifact::Body,
    );
    reference(store, &name)
}

#[test]
fn a_blob_already_waiting_is_not_recorded_twice() {
    // A hot document with a rotted replica is read over and over. Without this, one blob
    // would fill the backlog and evict every other finding.
    let (_vfs, store) = fresh_store();
    let reference = distinct_reference(&store, 0);

    for _ in 0..50 {
        store.note_damaged_blob(reference);
    }

    assert_eq!(store.repair_backlog(), (1, 0));
}

#[test]
fn a_full_backlog_evicts_the_oldest_finding_and_keeps_the_newest() {
    // `D341`: entries are ordered by when the damage was first seen, so the oldest is the
    // one most likely already repaired and the newest the one most likely still true.
    // Refusing the newest -- the obvious alternative -- would keep the less useful end.
    let (_vfs, store) = fresh_store();
    let overflow = 4;
    let total = crate::store::REPAIR_BACKLOG_LIMIT + overflow;

    for seed in 0..total {
        store.note_damaged_blob(distinct_reference(&store, seed));
    }

    assert_eq!(
        store.repair_backlog(),
        (crate::store::REPAIR_BACKLOG_LIMIT, overflow as u64),
        "held at the limit, and said how many it let go"
    );

    let waiting = store.take_repair_backlog();
    let oldest_evicted = distinct_reference(&store, 0);
    let newest_kept = distinct_reference(&store, total - 1);
    assert!(
        !waiting.contains(&oldest_evicted),
        "the oldest finding was the one evicted"
    );
    assert!(
        waiting.contains(&newest_kept),
        "the newest finding was kept"
    );
}

#[test]
fn draining_the_backlog_empties_it() {
    let (_vfs, store) = fresh_store();
    for seed in 0..3 {
        store.note_damaged_blob(distinct_reference(&store, seed));
    }
    assert_eq!(store.repair_backlog().0, 3);

    let taken = store.take_repair_backlog();

    assert_eq!(taken.len(), 3);
    assert_eq!(store.repair_backlog(), (0, 0));
}

#[test]
fn a_drain_that_fails_part_way_puts_its_suspects_back() {
    // The drain takes the whole backlog before examining any of it, so an error on the
    // first suspect would otherwise forget every finding that was waiting.
    let gate = acadia::FaultGate::closed();
    let policy = acadia::FaultPolicy::new().inject_gated(
        acadia::FaultOp::Read,
        acadia::PathMatcher::has_component(BLOB_TREE_DIR),
        acadia::Fault::io_error(),
        acadia::When::Always,
        gate.clone(),
    );
    let vfs: Arc<dyn Vfs> = Arc::new(acadia::FaultyFs::new(acadia::MemFs::new(), policy));
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init_unsourced(vfs, root, test_config()).expect("init");

    for seed in 0..3 {
        store.note_damaged_blob(distinct_reference(&store, seed));
    }
    gate.open();

    let mut counter = 0;
    store
        .repair_reported_damage_with(|| {
            counter += 1;
            nonce_of(&format!("d{counter}"))
        })
        .expect_err("every blob read fails");

    assert_eq!(
        store.repair_backlog(),
        (3, 0),
        "the failed drain restored every suspect it had taken"
    );
}
