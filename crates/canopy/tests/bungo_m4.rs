// Copyright (c) Michael Grier.
//! CANOPY-M4 recovery-stress tests over the fault-injecting store harness
//! (CHAOS-2..CHAOS-4, `D55`/`D56`).
//!
//! The first group (CHAOS-2) faults the phase boundaries of a transaction apply
//! and proves each one is recoverable on reopen.
//!
//! The second group (CHAOS-3) faults the recovery replay itself (`D46`/`D53`/`D56`,
//! TXN-4): after a crash leaves the log mid-transaction, reopen is driven over a
//! corrupt or torn log tail and over read/rename faults during the replay. A torn
//! (unterminated) tail is discarded and recovery completes idempotently; a corrupt
//! (complete-but-invalid) tail or a hard read/rename fault surfaces a typed error
//! and the process stays up rather than looping to force convergence -- and a
//! recovery interrupted by a transient fault converges on a later reopen.
//!
//! The third group (CHAOS-4) is a checked-arithmetic audit of the store paths
//! (`D56`/`D79`): it confirms the count-derived arithmetic (the replica-count loops
//! and `Vec::with_capacity`) is bounded and checked, so a malformed or extreme
//! replica count -- in a caller's config or a tampered genesis marker -- surfaces
//! a typed error ([`StoreError::InvalidPointerReplicaCount`] at config time,
//! [`StoreError::MalformedMarker`] at open time) rather than an unbounded
//! allocation, an overflow panic, or a scan that never terminates.
//!
//! The crash model is the D74 one the harness documents: a scripted fault aborts
//! a real operation part-way, then the store is reopened to run recovery over the
//! state left behind -- never [`simulate_crash`](acadia::MemFs::simulate_crash).

mod common;

use acadia::{Fault, Vfs, VfsErrorKind, When};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, MAX_REPLICA_COUNT, NodePath, Nonce, ShireId,
    StoreError, Swap, SwapSet, read_once,
};
use common::{Chaos, Script};

/// The single node these tests write to.
fn node() -> NodePath {
    NodePath::parse("record").unwrap()
}

/// The single document name.
fn doc() -> DocumentName {
    DocumentName::new("d.json").unwrap()
}

/// A document body from `s`.
fn body(s: &str) -> DocumentBody {
    DocumentBody::new(s.as_bytes().to_vec())
}

/// A transaction id from `s`.
fn nonce(s: &str) -> Nonce {
    Nonce::new(s).unwrap()
}

/// A one-swap transaction writing `b` to [`node`]/[`doc`].
fn single(b: &DocumentBody) -> SwapSet {
    SwapSet::single(Swap::new(node(), doc(), b.clone()))
}

// ---- CHAOS-2: transaction phase-boundary faults (D53) ------------------------

#[test]
fn an_fsync_failure_mid_commit_aborts_apply_and_reopen_recovers() {
    let script = Script::new().fail_commit_fsync(0, &node(), Fault::io_error(), When::Once);
    let (chaos, store) = Chaos::open(3, script);
    let err = store
        .apply_transaction(&single(&body("v2")), &nonce("t1"))
        .unwrap_err();
    assert!(matches!(err, StoreError::Vfs(_)));
    // The prepare record is durable, so recovery rolls the transaction forward.
    let recovered = chaos.reopen(store).unwrap();
    assert_eq!(
        read_once(&recovered, &node(), &doc()).unwrap(),
        Some(body("v2"))
    );
}

#[test]
fn failing_begin_leaves_the_log_empty_and_commits_nothing() {
    let script = Script::new().fail_begin(Fault::io_error());
    let (chaos, store) = Chaos::open(3, script);
    let err = store
        .apply_transaction(&single(&body("v")), &nonce("t"))
        .unwrap_err();
    assert!(matches!(err, StoreError::CommitLog(_)));
    // A failed `begin` append persists nothing, so the log is absent or empty.
    match chaos.mem().read(&chaos.layout().commit_log()) {
        Err(e) => assert_eq!(e.kind(), VfsErrorKind::NotFound),
        Ok(bytes) => assert!(bytes.is_empty(), "log should be empty after a failed begin"),
    }
    let recovered = chaos.reopen(store).unwrap();
    assert_eq!(read_once(&recovered, &node(), &doc()).unwrap(), None);
}

#[test]
fn failing_prepare_rolls_the_transaction_back_on_reopen() {
    let script = Script::new().fail_prepare(Fault::io_error());
    let (chaos, store) = Chaos::open(3, script);
    let err = store
        .apply_transaction(&single(&body("v2")), &nonce("t"))
        .unwrap_err();
    assert!(matches!(err, StoreError::CommitLog(_)));
    // Only `begin` reached the log, so recovery rolls back: nothing is committed.
    let recovered = chaos.reopen(store).unwrap();
    assert_eq!(read_once(&recovered, &node(), &doc()).unwrap(), None);
}

#[test]
fn failing_the_prepare_fsync_rolls_forward_on_reopen() {
    let script = Script::new().fail_prepare_fsync(Fault::io_error());
    let (chaos, store) = Chaos::open(3, script);
    let err = store
        .apply_transaction(&single(&body("v2")), &nonce("t"))
        .unwrap_err();
    assert!(matches!(err, StoreError::CommitLog(_)));
    // The `prepare` bytes were appended before its fsync failed, so the commit point
    // is on disk and recovery rolls the transaction forward.
    let recovered = chaos.reopen(store).unwrap();
    assert_eq!(
        read_once(&recovered, &node(), &doc()).unwrap(),
        Some(body("v2"))
    );
}

#[test]
fn failing_end_still_lands_the_transaction_via_recovery() {
    let script = Script::new().fail_end(Fault::io_error());
    let (chaos, store) = Chaos::open(3, script);
    let err = store
        .apply_transaction(&single(&body("v2")), &nonce("t"))
        .unwrap_err();
    assert!(matches!(err, StoreError::CommitLog(_)));
    // Every swap committed before `end` failed; recovery re-runs the idempotent
    // commit and ends the transaction, so the document is present.
    let recovered = chaos.reopen(store).unwrap();
    assert_eq!(
        read_once(&recovered, &node(), &doc()).unwrap(),
        Some(body("v2"))
    );
}

#[test]
fn the_same_script_is_deterministic_across_runs() {
    fn run() -> (bool, Option<DocumentBody>) {
        let script = Script::new().fail_prepare(Fault::io_error());
        let (chaos, store) = Chaos::open(3, script);
        let aborted = store
            .apply_transaction(&single(&body("v2")), &nonce("t"))
            .is_err();
        let recovered = chaos.reopen(store).unwrap();
        let landed = read_once(&recovered, &node(), &doc()).unwrap();
        (aborted, landed)
    }
    let first = run();
    let second = run();
    assert_eq!(first, second, "the scripted fault must be reproducible");
    assert_eq!(first, (true, None));
}

// ---- CHAOS-4: faults during the recovery replay itself (TXN-4) ----------------

#[test]
fn a_corrupt_log_tail_makes_recovery_surface_a_typed_error() {
    let (chaos, store) = Chaos::open_clean(3);
    store
        .apply_transaction(&single(&body("v1")), &nonce("t1"))
        .unwrap();

    // What the earlier transaction committed, as it stands before the corruption. The log
    // stays corrupt, so the store can never be reopened to read through -- comparing the
    // entry's own bytes is the only way left to say recovery left it alone.
    let entry = chaos.layout().committed(0, &node(), &doc());
    let before = chaos.mem().read(&entry).unwrap();

    // A crash that leaves a complete-but-invalid trailing line is corruption, not a
    // torn tail: append a whole garbage line to the log.
    let log = chaos.layout().commit_log();
    chaos.fs().append(&log, b"corrupt tail not json\n").unwrap();

    // Recovery reads the log, rejects the bad line, and surfaces a typed error --
    // it does not panic and does not silently drop the line.
    let err = chaos.reopen(store).unwrap_err();
    assert!(matches!(err, StoreError::CommitLog(_)));

    assert_eq!(chaos.mem().read(&entry).unwrap(), before);
}

#[test]
fn a_torn_log_tail_is_discarded_and_recovery_completes_idempotently() {
    let (chaos, store) = Chaos::open_clean(3);
    store
        .apply_transaction(&single(&body("v1")), &nonce("t1"))
        .unwrap();

    // A crash mid-append leaves an unterminated trailing line (no newline): the
    // torn-tail discard drops it, so recovery sees only the whole records.
    let log = chaos.layout().commit_log();
    chaos.fs().append(&log, b"{\"type\":\"begi").unwrap();

    let recovered = chaos.reopen(store).unwrap();
    assert_eq!(
        read_once(&recovered, &node(), &doc()).unwrap(),
        Some(body("v1"))
    );
}

#[test]
fn a_corrupt_tail_over_a_prepared_transaction_refuses_rather_than_forcing_convergence() {
    // `fail_end` leaves the log at `begin` + `prepare` with the swaps already
    // committed: a transaction recovery would otherwise roll forward and end.
    let script = Script::new().fail_end(Fault::io_error());
    let (chaos, store) = Chaos::open(3, script);
    let err = store
        .apply_transaction(&single(&body("v")), &nonce("t1"))
        .unwrap_err();
    assert!(matches!(err, StoreError::CommitLog(_)));

    // Corrupt the tail after the durable `prepare`.
    let log = chaos.layout().commit_log();
    chaos.fs().append(&log, b"corrupt tail not json\n").unwrap();

    // Recovery refuses with a typed error instead of forcing the prepared
    // transaction forward: no `end` record is ever written.
    let err = chaos.reopen(store).unwrap_err();
    assert!(matches!(err, StoreError::CommitLog(_)));
    let bytes = chaos.fs().read(&log).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("\"type\":\"end\""),
        "recovery must not force the transaction to end over a corrupt tail"
    );
}

#[test]
fn a_read_error_during_recovery_surfaces_a_typed_error() {
    // Store creation never reads the log and the clean apply only appends, so the
    // reopen's recovery read is the first matching log read; `Once` faults it.
    let script = Script::new().fail_log_read(Fault::io_error(), When::Once);
    let (chaos, store) = Chaos::open(3, script);
    store
        .apply_transaction(&single(&body("v1")), &nonce("t1"))
        .unwrap();

    let err = chaos.reopen(store).unwrap_err();
    assert!(matches!(err, StoreError::CommitLog(_)));

    // What was committed is intact; only the transient recovery read failed, so a second
    // reopen (the fault now spent) both succeeds and still holds the body.
    let reopened = chaos.reopen(None).unwrap();
    assert_eq!(
        read_once(&reopened, &node(), &doc()).unwrap(),
        Some(body("v1"))
    );
}

#[test]
fn a_rename_fault_during_recovery_surfaces_a_typed_error_then_a_retry_converges() {
    // Fail partition 0's swap rename on its first two occurrences: once during the
    // apply (aborting it after the durable `prepare`) and once during the first
    // reopen's roll-forward (aborting recovery). The third occurrence -- the second
    // reopen -- succeeds.
    let txn = nonce("t1");
    let script = Script::new().fail_swap_rename(0, &txn, &doc(), Fault::io_error(), When::Times(2));
    let (chaos, store) = Chaos::open(3, script);

    let err = store
        .apply_transaction(&single(&body("v")), &txn)
        .unwrap_err();
    assert!(matches!(err, StoreError::Vfs(_)));

    // Recovery hits the rename fault and surfaces a typed error -- the process stays
    // up rather than looping to force convergence.
    let err = chaos.reopen(store).unwrap_err();
    assert!(matches!(err, StoreError::Vfs(_)));

    // A subsequent reopen (the fault now spent) completes the roll-forward
    // idempotently and converges every partition on the new body. `None`: the failed
    // attempt above returned no handle, so nothing holds the root.
    let recovered = chaos.reopen(None).unwrap();
    assert_eq!(
        read_once(&recovered, &node(), &doc()).unwrap(),
        Some(body("v"))
    );
}

#[test]
fn recovery_is_idempotent_across_repeated_reopens() {
    // A mid-commit fsync fault leaves a durable `prepare` with the renames pending,
    // so the first reopen rolls the transaction forward.
    let script = Script::new().fail_commit_fsync(0, &node(), Fault::io_error(), When::Once);
    let (chaos, store) = Chaos::open(3, script);
    let err = store
        .apply_transaction(&single(&body("v2")), &nonce("t1"))
        .unwrap_err();
    assert!(matches!(err, StoreError::Vfs(_)));

    let first = chaos.reopen(store).unwrap();
    assert_eq!(
        read_once(&first, &node(), &doc()).unwrap(),
        Some(body("v2"))
    );

    // Re-running recovery is a no-op: the transaction is already ended, so a second
    // reopen leaves the same converged state and queues no repair.
    let second = chaos.reopen(first).unwrap();
    assert_eq!(
        read_once(&second, &node(), &doc()).unwrap(),
        Some(body("v2"))
    );
}

// ---- CHAOS-4: checked-arithmetic audit of the store paths (D79) ----------------

/// A genesis config for the arithmetic-boundary tests.
fn config() -> GenesisConfig {
    GenesisConfig::new(ShireId::new("chaos5-shire").unwrap(), 1)
}

#[test]
fn an_extreme_replica_count_in_the_config_is_a_typed_error_not_an_allocation() {
    // The valid boundary: 1 and the ceiling itself (both odd) are accepted, so the
    // ceiling never narrows a realistic replication width.
    assert!(config().with_pointer_replica_count(1).is_ok());
    assert!(
        config()
            .with_pointer_replica_count(MAX_REPLICA_COUNT)
            .is_ok()
    );

    // Zero, even, one past the ceiling, and the extreme `usize` each surface a typed
    // `InvalidPointerReplicaCount` -- never a panic and never an unbounded directory /
    // allocation loop, because the count can only ever be a bounded value.
    for count in [0, 2, MAX_REPLICA_COUNT + 2, usize::MAX] {
        assert!(
            matches!(
                config().with_pointer_replica_count(count),
                Err(StoreError::InvalidPointerReplicaCount { count: c }) if c == count
            ),
            "pointer replica count {count} must surface a typed error"
        );
    }
}

#[test]
fn an_extreme_replica_count_in_the_marker_is_rejected_as_malformed() {
    let (chaos, _store) = Chaos::open_clean(3);

    // Tamper the on-disk marker to record an absurd replica count, bypassing the
    // config-time guard. The `MAX_REPLICA_COUNT` ceiling is checked before the replica
    // scan and any `Vec::with_capacity`, so open surfaces a typed `MalformedMarker`
    // instead of acting on the extreme count.
    let marker = chaos.layout().marker();
    let text = String::from_utf8(chaos.mem().read(&marker).unwrap()).unwrap();
    let tampered = text.replace(
        "\"pointer_replica_count\": 3",
        "\"pointer_replica_count\": 1000000001",
    );
    assert_ne!(
        tampered, text,
        "the marker must carry the pointer_replica_count field"
    );
    chaos.mem().write(&marker, tampered.as_bytes()).unwrap();

    let err = chaos.reopen(None).unwrap_err();
    assert!(matches!(err, StoreError::MalformedMarker { .. }));
}
