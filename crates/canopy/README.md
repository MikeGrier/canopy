# canopy

A durable, replicated store for tree-structured documents.

canopy keeps a namespace of `/`-separated paths, mapped onto a directory hierarchy,
where each node holds named documents. Document bodies are opaque bytes: canopy
stores, replicates, and verifies them but never parses them. It is a library with no
transport, no protocol layer, no maintenance scheduler, and no clock of its own, and it
starts no threads.

## Features

- **Atomic writes.** `Store::update_document` replaces one document all-or-nothing.
  `Store::transaction` builds an opaque operation list without exclusion, recording every read
  as a digest or absence check, and `Store::transact` submits it once across many documents.
  `TransactionBuilder::append` concatenates raw bytes under exclusion, treating absence as an
  empty body; it accepts a bounded suffix and resulting body. `Store::apply_transaction` retains
  the lower-level `SwapSet` path. Logged transactions are
  journaled through an append-only commit log and replayed on open after a crash.
- **Compare-and-set.** `Swap::conditional` commits only if the current body has an
  expected `ContentDigest`; `Swap::conditional_absent` commits only if the document
  does not yet exist.
- **N-copy durability.** Content lives in content-addressed blobs and the namespace
  in separately replicated pointer trees. Reads take a quorum of pointer replicas and
  verify each blob against the digest in its own name, so a bad copy is identified
  rather than outvoted.
- **Self-repair.** Damage seen by reads is queued for repair, and
  `Store::scrub_content` walks every document and rebuilds bad replicas from a
  verified copy.
- **Writers take turns.** Several writer handles may stay attached. Each outermost write
  holds coordination exclusion until it ends; the OS releases its lock if the holder dies,
  and subsequent writers require explicit recovery before making effects. Logged submissions
  through any clone are serialized at the commit. `Store::transform` reads and builds outside
  exclusion, then retries a changed read through a bounded number of attempts. Reads never take
  the lock, and `Store::snapshot` reads a state no write disturbed without excluding writers.
- **Schema admission.** Read-only handles may coexist with writers. Every handle holds
  shared reader attachment; a writer also holds shared writer attachment. After attaching,
  each schema layer admits readers from its oldest reader schema through its current schema,
  and writers only at current. A mismatch reports `StoreError::SchemaMismatch`. Clones share
  the attachment and its best-effort registry entry until the last clone is dropped.
- **Attachment drains.** Reads, writes, repairs and management work hold operation leases.
  `Store::drain_upgrade` refuses new work on its clones while its guard waits for existing work
  to end beneath the shared tiers. Cancelling or dropping the guard restores operation admission;
  starting or waiting on a drain from inside a store operation is refused.
- **Pluggable filesystem.** All I/O goes through the [`acadia`](https://crates.io/crates/acadia)
  `Vfs` trait, so the same store runs on the real filesystem (`OsFs`) or fully in
  memory with scripted fault injection (`MemFs`, `FaultyFs`) for tests.

## Client Contract for Sessions

`Store::session` creates an asynchronous in-process engine. `Session::queue_pair` returns mutable
submission and completion ends. Both are `Send`, neither is `Sync`, and every accepted request is
identified only within that pair's lifetime. This guide is the dependent's contract; queue storage,
doorbells, worker state and the scheduler cursor are mechanism details.

| Observable outcome | Meaning and caller action | Client-model promise |
|---|---|---|
| `Ok(RequestId)` | The command reserves a completion slot and was accepted. Match that request only with completions from the same pair. | `SubmissionOutcomesCoherent`, `CompletionCapacityReserved` |
| `SubmissionRefusal::CompletionQueueFull` | No command was accepted or given a request id. Drain or wait for a completion before retrying. | `SubmissionOutcomesCoherent`, `CompletionCapacityReserved` |
| `SubmissionRefusal::PairClosed` | The pair was closed, so no command was accepted. Retry only through a new pair and treat it as a new operation. | `SubmissionOutcomesCoherent` |
| `Completion` on a live pair | Exactly one result for its pair-local request. Completions for that pair are ordered by request. | `AtMostOneDelivery`, `PerPairCompletionOrder` |
| `CompletionQueue::wait` returns `None` | The bounded wait expired with no ready completion. The accepted request may still be pending or complete; this is not cancellation. | `AtMostOneDelivery` |
| No completion because the pair was dropped | The contract outcome is `CompletionUnavailable`. The request may have been withdrawn, applied, or left requiring recovery. Never infer no effect. | `LostCompletionIsUnknown` |
| `Read::Contended` or `DocumentOvertaken` | A read did not settle on one observed state. Retry according to the caller's budget; it says nothing about a write request's effect. | Store read classification, projected outside request effect |
| `SnapshotContended` | No undisturbed snapshot completed before its deadline. Retry if the operation remains useful; no write effect is claimed. | Client composition's no-effect result for disturbed reads |
| `StoreError::RecoveryRequired` | A store operation is refused because unresolved durable work must be recovered. It does not classify an earlier uncertain request as definitely applied or unapplied. | `RecoveryReportsUncertainEffect` |

A completed transaction may report `Committed`, `Rejected`, or a store error. A rejection and a
failure established before `prepare` are definite no-effect. A failure after the commit point can
be an error with uncertain durable effect and can require recovery. Recovery may later finish the
same operation, but an application must not rewrite the request's earlier uncertainty by reading
the store afterward.

A read establishes only the state it observed. For an arbitrary operation, another writer may
have changed that state before or after the read, so read-back is not a universal proof of whether
the uncertain request took effect. A caller that needs this distinction carries an application
idempotency token or equivalent durable evidence in the same transaction and checks that evidence.
Resubmission is not deduplicated by `RequestId`.

Completions are FIFO within one pair (`PerPairCompletionOrder`). Across ready pairs, the engine
gives round-robin service and parks a command waiting for writer exclusion without occupying an
engine worker (`ReadyPairService`). This prevents one ready pair from being skipped indefinitely,
but it is not a wall-clock latency promise, a global FIFO, or a stronger fairness rule for the
separate coordination lock. `ClientLiveSpec` assumes the selected command can finish and the
coordination environment permits progress; the contract does not manufacture either assumption.
`Session::transact` and the read/list/snapshot helpers are submit-one-and-wait conveniences over
this same contract; their timeout is a wait bound, not cancellation of accepted work.

## Schema Upgrades

After `UpgradeDrain::wait`, `begin_conversion(UpgradeRequest)` releases the source attachment's
tiers. `SchemaUpgrade::poll` acquires the gate and required tiers without waiting for locks;
once ready, `complete` advances only the selected layer and returns its independent stamp result.
A safe upgrade admits existing reader schemas and its target; safety is judged against the
oldest admissible reader, not the current schema. An unsafe upgrade drains all readers and sets
the completed reader floor to its target.

The source handle remains detached until `Store::reattach` performs ordinary admission with the
chosen agent and access. Cancelling a pending conversion releases its holds without changing a
schema. Cancelling after the admission announcement is published leaves it fenced for explicit
recovery, so target readers already admitted cannot be invalidated by a rollback.

For an agent migration, `begin_migration(request, units)` declares the body before publication.
Each `SchemaUpgrade::step` supplies an idempotent unit through `MigrationUnit`, whose progress
identifies the request and durable unit ordinal. The cursor advances only after the unit's journal
work settles. `complete` refuses unfinished bodies. A failure, cancellation or panic leaves intent
and a fence; recovery reaches `RecoveryPhase::UpgradeBody` after ordinary journal work and uses
`Recovery::step_upgrade` for the remaining units. A plain recovery call reports
`UpgradeBodyRequired` rather than guessing or discarding units.

Canopy-owned migration units require a concrete schema-change specification; undefined requests
are rejected. That agreed dependency is tracked in [CHECKLIST.md](CHECKLIST.md).

## Management

`Store::recovery_required` and `Store::observe` return `Result` so a draining attachment can refuse
the query or installation. `UpgradeDrain::registration_error` reports a failed best-effort registry
update without undoing the drain, and explicit cancellation reports failure to update it again.

`Store::properties` reads fresh metadata, and `Store::attached_handles` reports live registrations
plus unknown entries and possible incompleteness. A writable handle can durably update its
description with `set_agent_description`; read-only and recovery-fenced handles cannot.
`DiagnosticView::open` offers properties, attached handles and `recovery_need` without an agent or
attachment tier, so it works while the admission gate is held. It offers no document reads or writes.
`StoreProperties::schema_upgrade` reports an active per-layer admission announcement.

## Local filesystems only

**The store root must be on a local filesystem of the host that opens it. Network
filesystems are not supported** -- SMB/CIFS shares (including mapped network drives), NFS
mounts, and other remote or distributed filesystems. canopy's guarantees rest on the local
operating system's lock, rename and fsync behaviour, which network filesystems do not reliably
provide. canopy does not detect or refuse a networked root: one may appear to work while
losing writes or corrupting the store.

## Example

Every `StoreOptions` constructor requires an explicit `Agent`: its version-independent identity,
software version, description and document-schema version. Opens compare the identity exactly and
do not rewrite the recorded version or description. Genesis version 9 protects migration intent
and each layer's current schema and oldest reader schema. Earlier supported roots require
`Store::migrate` with the intended agent while the root is offline; a retry must supply identical
agent inputs. Migration changes the marker, not the document bodies or their layout.

Mutation APIs return `WriteOutcome<T>` inside their usual `Result`: `value` contains the
successful operation's result and `agent_stamp` reports the separate update of the recorded
agent version. A stamp failure is not a failed write and must not trigger replay of the operation.
Rejected transactions and no-ops do not stamp. Repairs and maintenance stamp successful work,
with scrub retaining per-document exclusion; a batch reports its first stamp failure separately.

```rust,no_run
use std::sync::Arc;

use acadia::{OsFs, Vfs};
use canopy::{
    DocumentBody, DocumentName, GenesisConfig, NodePath, Nonce, Read, StoreId, Store,
    StoreOptions, Swap,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let vfs: Arc<dyn Vfs> = Arc::new(OsFs::new());
    let root = std::path::PathBuf::from("/var/lib/my-store"); // an existing, empty directory

    // Every write draws a nonce from the source the store is opened with. The
    // `random-nonce` feature provides `canopy::random_nonce_source()`; any source whose
    // nonces are unique per operation will do.
    let mut next = 0u64;
    let agent = canopy::Agent::new("example-app", "1", "Example application", 1);
    let options = StoreOptions::with_nonce_source(agent, move || {
        next += 1;
        Nonce::new(&format!("op-{next}")).expect("valid nonce")
    });

    // Create the store, or open it if a call with this identity already made it: `init` is
    // idempotent, so any seeding that follows must be too. To reopen it, `Store::open(vfs, root,
    // options)` also returns what it found beside the handle -- call `resolve()` or `recover()`
    // if it needs either.
    let created_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let config = GenesisConfig::new(StoreId::new("example")?, created_at_unix_ms);
    let store = Store::init(Arc::clone(&vfs), root.clone(), config, options)?;

    // Write a document atomically.
    let node = NodePath::parse("projects/alpha")?;
    let name = DocumentName::new("settings.json")?;
    let body = DocumentBody::new(br#"{"theme":"dark"}"#.to_vec());
    let receipt = store.update_document(Swap::new(&node, name.clone(), body))?;
    assert!(receipt.value.is_committed());
    receipt.agent_stamp.expect("document committed, but its agent stamp failed");

    // Read it back, verified against its content digest.
    match store.read_document(&node, &name)? {
        Read::Found(body) => println!("{} bytes", body.as_bytes().len()),
        Read::Absent => println!("not there"),
        Read::Contended(_) => println!("a writer was busy; read again"),
    }

    // Scrub regularly: it is the only pass that finds and repairs unread bad replicas.
    let receipt = store.scrub_content()?;
    receipt.agent_stamp.expect("scrub completed, but an agent stamp failed");
    let (report, _nonces) = receipt.value;
    println!("scrub clean: {}", report.is_clean());
    Ok(())
}
```

## Names

Path components, document names, and nonces are validated and normalized: ASCII
letters (folded to uppercase), digits, `-` and `_`, with an interior `.` also allowed
in document names. There is no relative addressing: `.` and `..` are rejected, so a
path always names exactly one node.

## What the embedder must do

- **Schedule scrubs.** Reads only probe what they touch, and the repair backlog is
  bounded on purpose. A bad replica under a document nobody reads is rebuilt by
  `scrub_content` or by nothing, and every read keeps succeeding until the last good
  copy is gone. canopy has no scheduler, so choosing and keeping the cadence is the
  embedder's job.
- **Supply a nonce source.** Every write draws a `Nonce` from the source passed in
  `StoreOptions`; it names the commit-log records and staging files that crash recovery
  uses. Nonces need only be statistically unique, and a collision is detected and fails
  the operation rather than corrupting anything. Enable the `random-nonce` feature for a
  ready-made 128-bit random source.
- **Drain the repair backlog.** `Store::repair_reported_damage` rebuilds what reads have
  found damaged. canopy runs no thread to do it, so call it when the store is idle.
- **Retry contended reads.** `Read::Contended` means a writer was mid-update; it is
  not an error. `read_settled` and `read_within` provide the retry loop when you want
  one.

## Minimum supported Rust version

Rust 1.98.

## License

MIT
