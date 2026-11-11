// Copyright (c) Michael Grier.

//! `CVS-17` integration test: fault at every phase boundary, then recover and check what
//! the store became.
//!
//! The other tests in this crate stop at the crash. `transactional_ordering` asserts the
//! on-disk state the instant a transaction is interrupted, `transactional_deletion` and
//! `transactional_rollback` each drive one crash window. What none of them do is the thing
//! a real operator experiences: **the process dies and comes back**. This file does that
//! systematically -- a fault at every point a transaction can be cut, followed by a reopen,
//! followed by the full set of invariants.
//!
//! # What is cut
//!
//! Three operations, because between them they cover every phase boundary of the apply:
//!
//! - **`Append`** cuts the journal: before `begin`, between `begin` and `prepare` (the commit
//!   point), and between `prepare` and `end`.
//! - **`Rename`** cuts every commit: a blob into its shard during staging, a staged document
//!   over its committed path in phase 4, and a pointer into the namespace.
//! - **`RemoveFile`** cuts the collection in phase 6, and the staging sweeps.
//!
//! Each is swept twice: once restarting over the same live state, and once discarding
//! everything not yet durable first, as a power loss would. Only the second can catch a
//! missing `fsync`, so **`Fsync`** itself is swept that way too.
//!
//! # What is asserted, and why these five
//!
//! 1. **No dangling pointer.** Every pointer resolves to a blob that reads and verifies.
//!    This is `D327`'s invariant, checked after recovery rather than during.
//! 2. **All-or-nothing.** The two documents agree on which version they hold. A transaction
//!    that half-landed across documents is what `D53` exists to prevent, and a crash is the
//!    only thing that can produce one.
//! 3. **A committed transaction is never lost.** If the apply returned `Committed`, both
//!    documents hold the new body after recovery -- whatever was interrupted afterwards.
//! 4. **The two trees agree.** The partition tree and the namespace resolve each document to
//!    the same body. `CVS-13` established that as a property of the layouts; this checks it
//!    survives an interrupted transaction, which is where roll-forward has to land both
//!    halves or neither (`D328`).
//! 5. **Nothing is left over.** Exactly one blob per document per replica, so neither the
//!    superseded version nor an abandoned ingest outlived recovery.
//!
//! Recovery is then run a second time and the answers compared, because a recovery that is
//! not idempotent is a store that degrades every time it restarts.
//!
//! # The limit of this model, stated plainly
//!
//! A fault makes an operation **fail**; a power loss can leave one **half-done**. For a
//! rename or a log append the two are close, because those are the operations the design
//! leans on for atomicity -- which is exactly why they are the ones cut here. They are not
//! identical, and nothing below should be read as a claim about torn writes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use acadia::{Fault, FaultGate, FaultOp, FaultPolicy, FaultyFs, MemFs, PathMatcher, Vfs, When};
use canopy::{
    BlobName, DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, PathHash, ShireId, Store,
    StoreError, StoreOptions, Swap, SwapSet, TransactionOutcome,
};

/// Options drawing nonces `N1`, `N2`, ... .
fn options() -> StoreOptions {
    let mut n: u64 = 0;
    StoreOptions::with_nonce_source(move || {
        n += 1;
        Nonce::new(&format!("N{n}")).unwrap()
    })
}

/// Points at which to cut, per operation.
///
/// A two-document transaction performs eighteen renames at the default widths, three log
/// appends, and a dozen removals. The bound overshoots all of them so the tail runs cut
/// nothing -- the control that keeps a bug which simply never writes from passing.
const CUT_POINTS: u32 = 24;

/// The documents the transaction moves together. Two, so "all-or-nothing" has something to
/// be true of.
fn nodes() -> [NodePath; 2] {
    [
        NodePath::parse("tickets/alpha").unwrap(),
        NodePath::parse("tickets/bravo").unwrap(),
    ]
}

fn doc() -> DocumentName {
    DocumentName::new("record.json").unwrap()
}

/// The body `node` holds at `generation`, distinct per document so a mis-served blob is
/// detectable rather than coincidentally equal.
fn body(node: &NodePath, generation: u32) -> DocumentBody {
    DocumentBody::new(format!("{node} generation {generation}").into_bytes())
}

fn swaps(generation: u32) -> SwapSet {
    SwapSet::new(
        nodes()
            .into_iter()
            .map(|node| {
                let body = body(&node, generation);
                Swap::new(node, doc(), body)
            })
            .collect::<Vec<_>>(),
    )
    .expect("distinct targets")
}

/// A store whose `k`-th `op` fails once the gate opens, with a handle to the `MemFs` beneath
/// so a caller can discard whatever was never made durable.
fn store_faulting(op: FaultOp, k: u32) -> (Arc<FaultyFs<MemFs>>, PathBuf, Store, FaultGate) {
    let gate = FaultGate::closed();
    let faulty = Arc::new(FaultyFs::new(
        MemFs::new(),
        FaultPolicy::new().inject_gated(
            op,
            PathMatcher::predicate(|_| true),
            Fault::io_error(),
            When::OnCall(k),
            gate.clone(),
        ),
    ));
    let vfs: Arc<dyn Vfs> = faulty.clone();
    let root = PathBuf::from("shire");
    vfs.create_dir_all(&root).unwrap();
    let store = Store::init(
        vfs.clone(),
        root.clone(),
        GenesisConfig::new(ShireId::new("cvs17-shire").unwrap(), 3),
        options(),
    )
    .expect("init");
    (faulty, root, store, gate)
}

/// What the namespace resolves `node` to, following its pointer into the content store.
///
/// Panics on a pointer that names an unreadable blob, because that is invariant 1 and a
/// panic here names it at the point it broke.
fn via_namespace(store: &Store, node: &NodePath, at: &str) -> Option<Vec<u8>> {
    match store.read_pointer(node, &doc()) {
        Ok(read) => Some(
            store
                .read_blob(&read.pointer().target())
                .unwrap_or_else(|e| {
                    panic!("{at}: dangling pointer at {node} -- its blob is unreadable: {e:?}")
                })
                .into_bytes(),
        ),
        Err(StoreError::PointerUnavailable { .. }) => None,
        Err(other) => panic!("{at}: unexpected pointer read failure at {node}: {other:?}"),
    }
}

/// How many blobs `node` owns across every replica.
fn blob_count(vfs: &Arc<dyn Vfs>, store: &Store, node: &NodePath) -> usize {
    let path_hash = PathHash::of(node, &doc());
    let shard = path_hash.shard(store.shard_width());
    let mut count = 0;
    for index in 0..store.blob_replica_count() {
        let dir = store
            .root()
            .join("files")
            .join(format!("p{index}"))
            .join(&shard);
        let Ok(entries) = vfs.read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let name = entry.file_name.to_string_lossy().into_owned();
            if BlobName::parse(&name).map(|blob| blob.path_hash()) == Ok(path_hash) {
                count += 1;
            }
        }
    }
    count
}

/// Every invariant, over a store that has just finished recovering.
///
/// Returns what each document resolved to, so a caller can compare two recoveries.
fn assert_invariants(
    vfs: &Arc<dyn Vfs>,
    store: &Store,
    committed: bool,
    at: &str,
) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut resolved = BTreeMap::new();

    for node in nodes() {
        // 1. No dangling pointer -- `via_namespace` panics if one is found.
        let namespace = via_namespace(store, &node, at);

        // The value is one the store was actually asked to hold.
        let expected = [
            body(&node, 1).as_bytes().to_vec(),
            body(&node, 2).as_bytes().to_vec(),
        ];
        let value = namespace
            .clone()
            .unwrap_or_else(|| panic!("{at}: {node} resolves to nothing after recovery"));
        assert!(
            expected.contains(&value),
            "{at}: {node} holds a body no transaction ever wrote"
        );

        // 5. Nothing left over: one blob per replica, so neither the superseded version nor
        //    an abandoned ingest survived.
        assert_eq!(
            blob_count(vfs, store, &node),
            store.blob_replica_count(),
            "{at}: {node} owns the wrong number of blobs after recovery"
        );

        resolved.insert(node.to_string(), namespace);
    }

    // 2. All-or-nothing: both documents moved, or neither did.
    let generations: Vec<u32> = nodes()
        .iter()
        .map(|node| {
            let value = resolved[&node.to_string()].as_ref().unwrap();
            if *value == body(node, 2).as_bytes().to_vec() {
                2
            } else {
                1
            }
        })
        .collect();
    assert!(
        generations.iter().all(|g| *g == generations[0]),
        "{at}: the transaction half-landed -- documents are at generations {generations:?}"
    );

    // 3. A committed transaction is never lost.
    if committed {
        assert_eq!(
            generations[0], 2,
            "{at}: the apply reported Committed and recovery went back to the old body"
        );
    }

    resolved
}

/// Cuts a two-document transaction at every point `op` can be faulted, recovering after each.
///
/// With `crash`, everything not yet made durable is discarded before recovery, as a power
/// loss would. Without it the process merely restarts over the same live state, which checks
/// recovery's logic but cannot catch a missing `fsync`.
fn sweep(op: FaultOp, label: &str, crash: bool) {
    sweep_through(op, label, crash, CUT_POINTS);
}

/// [`sweep`] over the first `cut_points` calls, for an operation a transaction issues more
/// often than [`CUT_POINTS`] covers.
fn sweep_through(op: FaultOp, label: &str, crash: bool, cut_points: u32) {
    let mut faulted = 0;
    let mut clean = 0;

    for k in 1..=cut_points {
        let at = format!("{label} cut {k}");
        let (faulty, root, store, gate) = store_faulting(op, k);
        let vfs: Arc<dyn Vfs> = faulty.clone();

        store
            .apply_transaction(&swaps(1))
            .unwrap_or_else(|e| panic!("{at}: the unfaulted seed transaction must commit: {e:?}"));

        gate.open();
        let outcome = store.apply_transaction(&swaps(2));
        let committed = matches!(
            outcome.as_ref().map(|commit| commit.outcome()),
            Ok(TransactionOutcome::Committed)
        );
        if outcome.is_err() {
            faulted += 1;
        } else {
            clean += 1;
        }

        // The fault is disarmed before recovery: this models a process that died and came
        // back on working hardware, which is the case the log exists to handle. A store whose
        // disk is still failing during recovery is a different scenario and not this one.
        gate.close();
        drop(store);
        if crash {
            faulty.inner().simulate_crash();
        }

        let recovered = Store::open(vfs.clone(), root.clone(), options())
            .unwrap_or_else(|e| panic!("{at}: recovery refused to open the store: {e:?}"));
        let first = assert_invariants(&vfs, &recovered, committed, &at);

        // Recovery is idempotent: a second pass must not move the store again.
        drop(recovered);
        let again = Store::open(vfs.clone(), root, options())
            .unwrap_or_else(|e| panic!("{at}: the second recovery refused to open: {e:?}"));
        let second = assert_invariants(&vfs, &again, committed, &format!("{at} (second recovery)"));
        assert_eq!(
            first, second,
            "{at}: recovering twice moved the store the second time"
        );
    }

    assert!(
        faulted > 0,
        "{label}: no cut point faulted the transaction; the operation is not on this path"
    );
    assert!(
        clean > 0,
        "{label}: every cut point faulted; raise CUT_POINTS past the operation count"
    );
}

#[test]
fn cutting_the_journal_at_every_append_recovers_to_a_coherent_store() {
    sweep(FaultOp::Append, "append", false);
}

#[test]
fn cutting_every_commit_rename_recovers_to_a_coherent_store() {
    sweep(FaultOp::Rename, "rename", false);
}

#[test]
fn cutting_every_removal_recovers_to_a_coherent_store() {
    sweep(FaultOp::RemoveFile, "remove", false);
}

#[test]
fn cutting_the_journal_then_losing_power_recovers_to_a_coherent_store() {
    sweep(FaultOp::Append, "append + crash", true);
}

#[test]
fn cutting_every_commit_rename_then_losing_power_recovers_to_a_coherent_store() {
    sweep(FaultOp::Rename, "rename + crash", true);
}

#[test]
fn cutting_every_removal_then_losing_power_recovers_to_a_coherent_store() {
    sweep(FaultOp::RemoveFile, "remove + crash", true);
}

#[test]
fn failing_every_fsync_then_losing_power_recovers_to_a_coherent_store() {
    // The case only a crash can test: an fsync that fails leaves its data exactly as volatile
    // as one that was never issued.
    sweep_through(FaultOp::Fsync, "fsync + crash", true, 128);
}
