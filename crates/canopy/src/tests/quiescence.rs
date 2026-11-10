// Copyright (c) Michael Grier.
// Split from tests.rs at 2a4dd8f.

//! TXN-3: quiescence reset of the commit log (`D53`).

use super::*;

// -- Quiescence reset (TXN-3, D53) ---------------------------------------------

#[test]
fn reset_empties_a_populated_log() {
    let (_vfs, log) = fresh_log();
    log.append(&begin(&nonce("t1"))).unwrap();
    log.append(&LogRecord::Prepare {
        txn: nonce("t1"),
        prepared: prepared(&sample_swaps()),
    })
    .unwrap();
    log.append(&LogRecord::End { txn: nonce("t1") }).unwrap();
    assert_eq!(log.read().unwrap().len(), 3);
    log.reset().unwrap();
    assert!(
        log.read().unwrap().is_empty(),
        "reset truncates the log to no records"
    );
}

#[test]
fn reset_truncates_the_file_to_zero_bytes() {
    let (vfs, log) = fresh_log();
    log.append(&begin(&nonce("t1"))).unwrap();
    assert!(vfs.metadata(log.path()).unwrap().len > 0);
    log.reset().unwrap();
    assert_eq!(
        vfs.metadata(log.path()).unwrap().len,
        0,
        "reset leaves the file in place at zero length, not unlinked"
    );
    assert!(
        vfs.exists(log.path()).unwrap(),
        "the file is truncated, not removed"
    );
}

#[test]
fn reset_on_an_absent_log_is_a_no_op() {
    let (vfs, log) = fresh_log();
    assert!(!vfs.exists(log.path()).unwrap());
    log.reset().unwrap();
    assert!(log.read().unwrap().is_empty());
    assert!(
        !vfs.exists(log.path()).unwrap(),
        "resetting an absent log does not create it"
    );
}

#[test]
fn reset_on_an_already_empty_log_is_idempotent() {
    let (_vfs, log) = fresh_log();
    log.append(&begin(&nonce("t1"))).unwrap();
    log.reset().unwrap();
    // A second reset over the already-empty log still succeeds and stays empty.
    log.reset().unwrap();
    assert!(log.read().unwrap().is_empty());
}

#[test]
fn store_quiesce_commit_log_empties_the_log() {
    let (_vfs, store) = fresh_store();
    let swaps = SwapSet::single(Swap::new(name("record"), doc("record.json"), body(b"x")));
    store.apply_transaction(&swaps, &nonce("apply1")).unwrap();
    assert!(!store.commit_log().read().unwrap().is_empty());
    store.quiesce_commit_log().unwrap();
    assert!(
        store.commit_log().read().unwrap().is_empty(),
        "quiescing the store empties its commit log"
    );
}

#[test]
fn commit_log_is_empty_reports_the_recovery_journal_state() {
    let (_vfs, store) = fresh_store();
    assert!(
        store.commit_log_is_empty().unwrap(),
        "a fresh store's recovery journal is empty"
    );
    let swaps = SwapSet::single(Swap::new(name("seed"), doc("seed.json"), body(b"seed")));
    store.apply_transaction(&swaps, &nonce("seed1")).unwrap();
    assert!(
        !store.commit_log_is_empty().unwrap(),
        "an applied transaction populates the journal"
    );
    store.quiesce_commit_log().unwrap();
    assert!(
        store.commit_log_is_empty().unwrap(),
        "quiescence empties the journal, the O(1)-clean-start observable"
    );
}

#[test]
fn monitor_resets_the_log_at_quiescence() {
    let (_vfs, store) = fresh_store();
    // Seed the log with a committed transaction's begin/prepare/end triple.
    let seed = SwapSet::single(Swap::new(name("seed"), doc("seed.json"), body(b"seed")));
    store.apply_transaction(&seed, &nonce("seed1")).unwrap();
    assert!(!store.commit_log().read().unwrap().is_empty());
    // A monitor write drives the actor to quiescence, which resets the log. The reset
    // is sequenced ahead of the write's reply, so it has happened once `write` returns.
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    monitor
        .write(name("record"), doc("record.json"), body(b"payload"))
        .unwrap();
    assert!(
        monitor.store().commit_log().read().unwrap().is_empty(),
        "the monitor truncates the log once its queue drains"
    );
}

#[test]
fn monitor_quiesce_truncates_the_log_on_demand() {
    let (_vfs, store) = fresh_store();
    // Seed the log with a committed transaction so there is something to truncate, then
    // spawn the monitor: its actor blocks on an empty queue and truncates nothing yet.
    let seed = SwapSet::single(Swap::new(name("seed"), doc("seed.json"), body(b"seed")));
    store.apply_transaction(&seed, &nonce("seed1")).unwrap();
    assert!(!store.commit_log().read().unwrap().is_empty());
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();

    // An explicit quiesce drives the actor to its quiescent moment on demand -- with no
    // write to trigger the automatic drain reset -- and truncates the D53 log.
    monitor.quiesce().unwrap();
    assert!(
        monitor.store().commit_log().read().unwrap().is_empty(),
        "an on-demand quiesce truncates the D53 commit log"
    );

    // The actor keeps serving after a quiesce: a subsequent write still applies.
    monitor
        .write(name("record"), doc("record.json"), body(b"after"))
        .unwrap();
    assert_eq!(
        monitor
            .store()
            .read_body(name("record"), &doc("record.json"))
            .unwrap(),
        Some(body(b"after"))
    );
}

#[test]
fn quiescence_reset_preserves_committed_data() {
    let (_vfs, store) = fresh_store();
    // A committed transaction lands `seed` and populates the log.
    let seed = SwapSet::single(Swap::new(name("seed"), doc("seed.json"), body(b"kept")));
    store.apply_transaction(&seed, &nonce("seed1")).unwrap();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    // A monitor write lands `record` and drains the queue, resetting the log.
    monitor
        .write(name("record"), doc("record.json"), body(b"fresh"))
        .unwrap();
    // The log is discardable at quiescence: emptying it loses no committed data, which
    // lives in the content and namespace trees.
    assert!(monitor.store().commit_log().read().unwrap().is_empty());
    assert_eq!(
        monitor
            .store()
            .read_body(name("seed"), &doc("seed.json"))
            .unwrap(),
        Some(body(b"kept"))
    );
    assert_eq!(
        monitor
            .store()
            .read_body(name("record"), &doc("record.json"))
            .unwrap(),
        Some(body(b"fresh"))
    );
}

#[test]
fn monitor_write_on_an_absent_log_stays_a_no_op_reset() {
    let (vfs, store) = fresh_store();
    let log_path = store.commit_log().path().to_path_buf();
    let monitor = WriteMonitor::spawn(store, counting_nonce_source()).unwrap();
    // The monitor's write path does not itself write the log, so a fresh store's log
    // stays absent across the quiescence reset (an absent log resets to a no-op).
    monitor
        .write(name("record"), doc("record.json"), body(b"payload"))
        .unwrap();
    let read = monitor
        .store()
        .read_body(name("record"), &doc("record.json"))
        .unwrap();
    assert_eq!(read, Some(body(b"payload")));
    assert!(monitor.store().commit_log().read().unwrap().is_empty());
    assert!(
        !vfs.exists(&log_path).unwrap(),
        "resetting an absent log never materializes it"
    );
}
