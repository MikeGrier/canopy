// Copyright (c) Michael Grier.

//! `CVS-18` integration test: scrub and repair over both trees, on the verification trigger
//! (`D316`, `D320`, `D326`, `D331`).
//!
//! The assertion carried here from `CVS-9` is the one that file could not make: **a repair
//! re-replicates from a verified-good copy rather than from a majority vote.** It is tested
//! the only way that distinguishes the two -- by corrupting a *majority* of the replicas. A
//! vote would rebuild from the corrupt majority and destroy the document; verification
//! rebuilds from the single intact copy, because a blob is convicted by examining it alone
//! (`D316`).
//!
//! The pointer half tests the opposite rule. A namespace replica that parses but trails the
//! majority is **behind, not broken** (`D320`), and must survive a scrub untouched -- the
//! transaction completing is what converges it. Only a replica that fails to parse is rebuilt.

use std::path::PathBuf;
use std::sync::Arc;

use acadia::{MemFs, Vfs};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, QUARANTINE_LIMIT, ShireId, Store,
    StoreOptions, Swap, SwapSet,
};

fn node() -> NodePath {
    NodePath::parse("tickets/alpha").unwrap()
}

fn doc() -> DocumentName {
    DocumentName::new("record.json").unwrap()
}

/// A distinct nonce per call, which is all a write or a repair needs of one (`D61`).
fn counting_nonces() -> impl FnMut() -> Nonce + Send + 'static {
    let mut issued = 0u32;
    move || {
        issued += 1;
        Nonce::new(&format!("M{issued}")).unwrap()
    }
}

/// A store holding one committed document, plus the backend for raw corruption.
fn seeded(text: &str) -> (Arc<dyn Vfs>, Store) {
    let vfs: Arc<dyn Vfs> = Arc::new(MemFs::new());
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        vfs.clone(),
        root,
        GenesisConfig::new(ShireId::new("cvs18-shire").unwrap(), 3),
        StoreOptions::with_nonce_source(counting_nonces()),
    )
    .expect("init");
    store
        .apply_transaction(&SwapSet::single(Swap::new(
            node(),
            doc(),
            DocumentBody::new(text.as_bytes().to_vec()),
        )))
        .expect("the seed commits");
    (vfs, store)
}

/// Where replica `index` keeps this document's blob.
fn blob_path(store: &Store, index: usize) -> PathBuf {
    let reference = store
        .read_pointer(&node(), &doc())
        .expect("a pointer")
        .pointer()
        .target();
    store
        .root()
        .join("files")
        .join(format!("p{index}"))
        .join(reference.relative_path())
}

/// Where replica `index` keeps this document's namespace entry.
fn pointer_path(store: &Store, index: usize) -> PathBuf {
    store
        .root()
        .join("namespace")
        .join(format!("p{index}"))
        .join(node().relative_path())
        .join(doc().as_str())
}

fn quarantine(store: &Store, tree: &str, index: usize) -> PathBuf {
    store
        .root()
        .join(tree)
        .join(format!("p{index}"))
        .join(".quarantine")
}

fn quarantined(vfs: &Arc<dyn Vfs>, store: &Store, tree: &str, index: usize) -> Vec<String> {
    let mut names = match vfs.read_dir(&quarantine(store, tree, index)) {
        Ok(entries) => entries
            .into_iter()
            .map(|entry| entry.file_name.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// What the document resolves to through the namespace, following its pointer.
fn resolved(store: &Store) -> Vec<u8> {
    let read = store.read_pointer(&node(), &doc()).expect("a pointer");
    store
        .read_blob(&read.pointer().target())
        .expect("the blob")
        .into_bytes()
}

#[test]
fn a_repair_rebuilds_from_the_one_intact_copy_even_when_the_majority_is_corrupt() {
    // The assertion `CVS-9` was written for and could not make. Two of three replicas are
    // corrupted, so a majority vote would "win" with garbage. Verification convicts each bad
    // copy on its own and rebuilds from the survivor.
    let (vfs, store) = seeded("the real body");
    for index in [0, 1] {
        vfs.write(&blob_path(&store, index), b"not the real body")
            .unwrap();
    }

    let (report, _) = store.scrub_content().expect("the scrub completes");

    assert_eq!(report.documents(), 1);
    assert_eq!(
        report.blob_repairs().len(),
        1,
        "the blob should be repaired"
    );
    assert!(
        report.unrepairable().is_empty(),
        "one intact copy is enough to rebuild from"
    );
    assert_eq!(
        resolved(&store),
        b"the real body".to_vec(),
        "the corrupt majority was allowed to win"
    );
    for index in 0..store.blob_replica_count() {
        assert_eq!(
            vfs.read(&blob_path(&store, index)).unwrap(),
            b"the real body".to_vec(),
            "replica {index} was not rebuilt"
        );
    }
}

#[test]
fn a_repair_keeps_the_corrupt_bytes_before_replacing_them() {
    // `D326`: the repair is the step that would otherwise destroy the only evidence of what
    // the corruption looked like.
    let (vfs, store) = seeded("the real body");
    let corrupt = b"garbage that must be kept".to_vec();
    vfs.write(&blob_path(&store, 1), &corrupt).unwrap();

    let (report, _) = store.scrub_content().expect("scrub");

    let (_, outcome) = &report.blob_repairs()[0];
    assert_eq!(outcome.replaced(), &[1]);
    assert_eq!(outcome.sequestered(), &[1]);
    assert!(outcome.discarded().is_empty());

    let kept = quarantined(&vfs, &store, "files", 1);
    assert_eq!(kept.len(), 1, "the corrupt copy was not kept: {kept:?}");
    let bytes = vfs
        .read(&quarantine(&store, "files", 1).join(&kept[0]))
        .expect("the sequestered copy is readable");
    assert_eq!(bytes, corrupt, "the sequestered bytes are not the bad ones");

    // And the quarantine is per replica, which is what makes clustering legible.
    assert!(quarantined(&vfs, &store, "files", 0).is_empty());
    assert!(quarantined(&vfs, &store, "files", 2).is_empty());
}

#[test]
fn a_missing_replica_is_rebuilt_and_nothing_is_sequestered() {
    // There is nothing to preserve about absence, so the quarantine stays empty.
    let (vfs, store) = seeded("the real body");
    vfs.remove_file(&blob_path(&store, 2)).unwrap();

    let (report, _) = store.scrub_content().expect("scrub");
    let (_, outcome) = &report.blob_repairs()[0];

    assert_eq!(outcome.replaced(), &[2]);
    assert!(outcome.sequestered().is_empty());
    assert!(quarantined(&vfs, &store, "files", 2).is_empty());
    assert_eq!(vfs.read(&blob_path(&store, 2)).unwrap(), b"the real body");
}

#[test]
fn a_blob_whose_every_copy_is_corrupt_is_reported_rather_than_guessed() {
    let (vfs, store) = seeded("the real body");
    for index in 0..store.blob_replica_count() {
        vfs.write(&blob_path(&store, index), b"all gone").unwrap();
    }

    let (report, _) = store.scrub_content().expect("scrub");

    assert!(report.blob_repairs().is_empty());
    assert_eq!(
        report.unrepairable().len(),
        1,
        "a blob with no intact copy must be reported"
    );
    // Nothing was invented, and nothing was moved aside: with no source to rebuild from,
    // sequestering would only turn a diagnosable corruption into an absence.
    for index in 0..store.blob_replica_count() {
        assert_eq!(vfs.read(&blob_path(&store, index)).unwrap(), b"all gone");
        assert!(quarantined(&vfs, &store, "files", index).is_empty());
    }
}

#[test]
fn an_unparseable_pointer_replica_is_rebuilt_and_kept() {
    let (vfs, store) = seeded("the real body");
    let garbage = b"this is not a pointer".to_vec();
    vfs.write(&pointer_path(&store, 2), &garbage).unwrap();

    let (report, _) = store.scrub_content().expect("scrub");

    let (subject, outcome) = &report.pointer_repairs()[0];
    assert!(subject.contains("RECORD.JSON"));
    assert_eq!(outcome.replaced(), &[2]);
    assert_eq!(outcome.sequestered(), &[2]);

    let kept = quarantined(&vfs, &store, "namespace", 2);
    assert_eq!(kept.len(), 1);
    assert_eq!(
        vfs.read(&quarantine(&store, "namespace", 2).join(&kept[0]))
            .unwrap(),
        garbage
    );

    // Every replica now agrees again.
    let read = store.read_pointer(&node(), &doc()).expect("a pointer");
    assert!(read.is_unanimous(), "the namespace was not made whole");
}

#[test]
fn a_stale_pointer_replica_is_left_exactly_as_it_is() {
    // `D320`'s rule, and the one a vote-driven repair would get wrong. Replica 2 is made to
    // hold a *valid* pointer that is simply not the current one. It is behind, not broken,
    // and the transaction completing is what converges it -- never the repair.
    let (vfs, store) = seeded("v1");
    let stale = vfs.read(&pointer_path(&store, 2)).unwrap();

    store
        .apply_transaction(&SwapSet::single(Swap::new(
            node(),
            doc(),
            DocumentBody::new(b"v2".to_vec()),
        )))
        .expect("the second write commits");

    // Put the old, still-valid pointer back on one replica.
    vfs.write(&pointer_path(&store, 2), &stale).unwrap();
    let read = store.read_pointer(&node(), &doc()).expect("a pointer");
    assert_eq!(read.stale(), &[2], "the fixture did not create staleness");

    let (report, _) = store.scrub_content().expect("scrub");

    assert!(
        report.pointer_repairs().is_empty(),
        "a stale replica was repaired, which is the log's job and not the repair's"
    );
    assert_eq!(
        vfs.read(&pointer_path(&store, 2)).unwrap(),
        stale,
        "the stale replica was rewritten"
    );
    assert!(quarantined(&vfs, &store, "namespace", 2).is_empty());

    // Left alone, but not passed off as healthy: staleness is reported, the document is not
    // counted whole, and the scrub is not clean.
    assert_eq!(report.stale().len(), 1);
    assert_eq!(report.stale()[0].1, vec![2]);
    assert_eq!(report.whole(), 0);
    assert!(!report.is_clean(), "a stale replica was reported as clean");

    // The rule lives in two places and the scrub's guard is only one of them: with nothing
    // else wrong, the scrub never calls the repair at all, so the assertions above would pass
    // against a repair that happily rewrites stale replicas. Drive it directly to pin the
    // rule where it actually is.
    let (outcome, _) = store
        .repair_pointer(&node(), &doc(), &read)
        .expect("repair");
    assert!(
        outcome.is_noop(),
        "repair_pointer rewrote a replica that was merely behind: {outcome:?}"
    );
    assert_eq!(
        vfs.read(&pointer_path(&store, 2)).unwrap(),
        stale,
        "repair_pointer rewrote the stale replica"
    );
}

#[test]
fn a_read_reports_damage_and_the_drain_repairs_more_than_the_read_examined() {
    // `D317` makes every read a partial probe: it stops at the first copy that verifies, so
    // it can only report the replicas it tried on the way there. `D339` therefore records the
    // blob's *identity* rather than the read's tally, and the drain re-examines -- which is
    // what lets it repair a replica the read never looked at.
    let (vfs, store) = seeded("the real body");
    let replicas = store.blob_replica_count();
    let reference = store
        .read_pointer(&node(), &doc())
        .expect("a pointer")
        .pointer()
        .target();

    // The replica rotation prefers, and so the one a read tries first.
    let first_choice = store
        .read_blob(&reference)
        .expect("a good copy exists")
        .served_by();
    // Rot the preferred replica so the read must step over it, and one *behind* the copy that
    // will serve, which the read will therefore never examine.
    let unexamined = (first_choice + 2) % replicas;
    for index in [first_choice, unexamined] {
        vfs.write(&blob_path(&store, index), b"rotted").unwrap();
    }

    assert_eq!(store.repair_backlog(), (0, 0), "nothing reported yet");

    let read = store
        .read_blob(&reference)
        .expect("an intact copy still serves");
    assert_eq!(
        read.corrupt(),
        &[first_choice],
        "the read stepped over exactly one bad replica and stopped"
    );

    let body = store
        .read_document(node(), &doc())
        .expect("the read serves");
    assert!(matches!(body, canopy::Read::Found(_)));
    assert_eq!(
        store.repair_backlog(),
        (1, 0),
        "the read reported the blob it found damaged"
    );

    let (report, _) = store.repair_reported_damage().expect("the drain repairs");
    assert_eq!(store.repair_backlog(), (0, 0), "the drain emptied it");
    assert_eq!(report.examined(), 1);

    let mut expected = vec![first_choice, unexamined];
    expected.sort_unstable();
    let (_, outcome) = &report.repairs()[0];
    assert_eq!(
        outcome.replaced(),
        expected.as_slice(),
        "every damaged replica, including the one the read never tried"
    );
    for index in outcome.replaced() {
        assert_eq!(
            vfs.read(&blob_path(&store, *index)).unwrap(),
            b"the real body".to_vec()
        );
    }
}

#[test]
fn a_read_that_finds_every_replica_intact_reports_nothing() {
    // The backlog must not fill up on healthy reads: a finding is a statement that something
    // is wrong, and a store that reports one for every read would bury the real ones.
    let (_vfs, store) = seeded("the real body");

    for _ in 0..8 {
        store.read_document(node(), &doc()).expect("a healthy read");
    }

    assert_eq!(store.repair_backlog(), (0, 0));
}

#[test]
fn a_full_quarantine_still_repairs_and_says_what_it_dropped() {
    // Redundancy is correctness and the kept bytes are diagnostics, so when the two compete
    // the bytes lose -- but never silently.
    let (vfs, store) = seeded("the real body");

    // Fill replica 1's quarantine to its limit with unrelated files.
    let full = quarantine(&store, "files", 1);
    vfs.create_dir_all(&full).unwrap();
    for i in 0..QUARANTINE_LIMIT {
        vfs.write(&full.join(format!("old-{i}")), b"x").unwrap();
    }

    vfs.write(&blob_path(&store, 1), b"garbage").unwrap();
    let (report, _) = store.scrub_content().expect("scrub");
    let (_, outcome) = &report.blob_repairs()[0];

    assert_eq!(outcome.replaced(), &[1], "the repair must still happen");
    assert!(outcome.sequestered().is_empty());
    assert_eq!(
        outcome.discarded(),
        &[1],
        "dropping the evidence must be reported, not silent"
    );
    assert_eq!(
        vfs.read(&blob_path(&store, 1)).unwrap(),
        b"the real body".to_vec()
    );
    assert_eq!(
        quarantined(&vfs, &store, "files", 1).len(),
        QUARANTINE_LIMIT,
        "the quarantine grew past its limit"
    );
}

#[test]
fn a_healthy_store_scrubs_clean_and_changes_nothing() {
    let (vfs, store) = seeded("the real body");
    let before: Vec<Vec<u8>> = (0..store.blob_replica_count())
        .map(|i| vfs.read(&blob_path(&store, i)).unwrap())
        .collect();

    let (report, _) = store.scrub_content().expect("scrub");

    assert!(report.is_clean(), "a healthy store reported work to do");
    assert_eq!(report.documents(), 1);
    assert_eq!(report.whole(), 1);
    for (index, bytes) in before.iter().enumerate() {
        assert_eq!(&vfs.read(&blob_path(&store, index)).unwrap(), bytes);
    }
}

/// Rots the replica a read would try first, so the next read must step over it.
fn rot_the_preferred_replica(vfs: &Arc<dyn Vfs>, store: &Store) -> usize {
    let reference = store
        .read_pointer(&node(), &doc())
        .expect("a pointer")
        .pointer()
        .target();
    let first_choice = store
        .read_blob(&reference)
        .expect("a good copy exists")
        .served_by();
    vfs.write(&blob_path(store, first_choice), b"rotted")
        .unwrap();
    first_choice
}

#[test]
fn the_store_repairs_reported_damage_when_asked() {
    // `D317` argued that rotation makes every read a probe. This is the other half of that
    // argument: something acts on what the probe finds, and under `D339` it is a writer,
    // because repairing is a write and needs a nonce.
    let (vfs, store) = seeded("the real body");
    let rotted = rot_the_preferred_replica(&vfs, &store);

    store
        .read_document(node(), &doc())
        .expect("the read serves");
    assert_eq!(store.repair_backlog().0, 1, "the read reported the damage");
    assert_eq!(
        vfs.read(&blob_path(&store, rotted)).unwrap(),
        b"rotted".to_vec(),
        "and repaired nothing itself -- a read never writes"
    );

    let (report, nonces) = store.repair_reported_damage().expect("the drain repairs");
    assert_eq!(nonces.len(), 1, "one repair, under one nonce");

    assert_eq!(report.examined(), 1);
    assert_eq!(report.repairs().len(), 1);
    assert_eq!(
        vfs.read(&blob_path(&store, rotted)).unwrap(),
        b"the real body".to_vec()
    );
    assert_eq!(store.repair_backlog(), (0, 0));
}
