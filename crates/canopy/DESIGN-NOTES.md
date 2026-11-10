# canopy design notes

Decision IDs (`Dnnn`) cited here belong to the decision register of the repository this
crate was imported from; that register was not imported. Names such as `canopy-core`,
`gaffer` and `bag-end` are consumers in that repository.

Decisions about the canopy tree-document store. See the crate-root and per-module
doc comments for the API contract; this file records the *why* behind choices a
future reader may want to understand or diagnose.

> **The namespace is hierarchical, and removal exists, as of `D222` (`BG-M1`/`BG-M2`).**
> `D40`/`D54` specified a path namespace over a directory hierarchy; what had been implemented
> was a single path *component* with a shard prefix derived from that component's own
> characters, and nothing was ever recorded deciding that.
> the originating repository's `DESIGN-NOTES.md` `D222` restored the hierarchy, retired sharding as a
> canopy concept, and in the same breaking change added the document and node removal this store
> had never had -- unblocking `D216`'s "a resolved entry leaves the list". See
> [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) and
> [COMPLETED-PLANS.md](COMPLETED-PLANS.md).

## Transaction preconditions: three variants, one atomic phase-1 (`D53`/`D54`/`D69`) -- [transaction.rs](src/transaction.rs), [store.rs](src/store.rs)

A [`Swap`](src/transaction.rs) is one document write within a `D53` transaction. Every
swap carries exactly one [`Precondition`](src/transaction.rs) that
[`apply_transaction`](src/store.rs) evaluates in the transaction's **phase-1** -- before
any log record or on-disk mutation -- so a mismatch rejects the whole [`SwapSet`] with
**no fsync** (`D69`). The three variants are:

- **`Unconditional`** -- a blind write: place the body regardless of the current state.
- **`ExpectedDigest(d)`** -- the `D54` compare-and-set: the document must currently exist
  with content digest `d` (`D59`). This is the lost-update guard a read-compute-write
  caller frames against the digest it read.
- **`ExpectedAbsent`** -- the **conditional-create**: the document must currently *not*
  exist. The write lands only if the target is absent, so two racers cannot both create
  the same document.

### Why a distinct `ExpectedAbsent` rather than a sentinel digest

A conditional-create cannot be expressed as `ExpectedDigest` of any value: there is no
digest that means "absent" (`D59` digests are content hashes of *present* bodies, and a
collision-resistant hash has no reserved "nothing here" point). Absence is a first-class
state of a `(node, document)` target, so it is a first-class precondition. Folding it into
the digest space would either burn a magic digest constant (a manifest numeric constant,
forbidden) or make "create if absent" inexpressible. The enum keeps all three mutually
exclusive states -- unconditional, present-with-digest, absent -- explicit and total.

### Rejection names the precondition kind, not a bare digest

When phase-1 rejects a swap, [`TransactionOutcome::Rejected`](src/store.rs) reports the
offending `(node, document)`, the actual `current: Option<ContentDigest>` (the `D40`
quorum-verified digest now on disk, `None` when absent), and an
[`RejectedPrecondition`](src/store.rs) that echoes *which* precondition failed --
`ExpectedDigest(d)` or `ExpectedAbsent`. A consumer (e.g. canopy-core's commit facade) classifies
the rejection by that tuple: a caller who framed a conditional-create learns the document
already exists (`current` is `Some`), while a compare-and-set caller learns the digest
moved. `Unconditional` swaps never appear in a rejection because they impose no
precondition.

### Reads bypass the writer, so absence is only atomic in phase-1 (`D41`)

Under the single-writer `WriteMonitor` (`D41`) only *writes* are serialized; *reads* are
lock-free and bypass the monitor. A consumer therefore cannot safely "read-then-write" to
enforce absence -- another writer could create the document between the read and the apply.
`ExpectedAbsent` exists precisely because the absence check must happen *inside* phase-1,
under the writer's serialization, atomically with the write it guards. This is why a
conditional-create is a store primitive and not something a caller can synthesize from a
read plus an unconditional write.

### ...and the serialization that makes them sound stops at the process boundary (`D116`)

Everything above holds *given a single writer*, and until `D116` nothing made that true
across processes. `D41`'s monitor is an actor inside one process; two processes opening the
same root each got their own, and neither knew about the other.

The consequence is easy to state and easy to miss: **a precondition does not create the
serialization it depends on.** Phase-1 evaluates `ExpectedDigest` or `ExpectedAbsent` and
then stages and renames, atomically with respect to other writes *this* monitor is
serializing. A writer in another process is not one of those. Two processes can each pass
the same precondition and each believe they won.

`D116` closes this by making a store root exclusive to one writing process, acquired through
the `Vfs` at `init`/`open` and released on drop -- so the phrase "another writer" in the
section above now has a bounded meaning. Note the scope carefully when reading tests: a
`MemFs`-backed store is exclusive among openers of *that filesystem*, which is one process
by construction, so a passing `MemFs` test says nothing about the cross-process property.

## Why `store.rs` remains a 34 KiB facade after the module split -- [store.rs](src/store.rs)

> **SUPERSEDED in part, 2026-11-09.** The size figure and the module list below describe the
> tree when this was written (`store/quorum.rs` no longer exists; `store/blob.rs`,
> `store/namespace.rs`, `store/repair.rs` and `store/writer.rs` do). The reasoning about why
> shared private items keep the facade large still holds.

The `Store` implementation was split into per-layer modules (`store/genesis.rs`,
`store/document.rs`, `store/quorum.rs`, `store/scrub.rs`, `store/apply.rs`,
`store/errors.rs`), yet the facade is still the largest file of that family. That is a
structural floor, not unfinished work.

Rust's visibility rules decide what a split can move: a child module can see its
ancestors' private items, but a parent cannot see its children's, and siblings cannot see
each other's. So any private item reachable from more than one part has to live in the
parent. Two clusters are bound to `store.rs` by that rule:

- **The genesis types.** `Store` holds a private `Genesis` field; `Genesis` reads
  `GenesisConfig`'s private fields; those read `ShireId`'s. Moving any one of the three
  down into `store/genesis.rs` leaves the parent unable to name it, so all three stay with
  the struct definition -- only the `init` / `open` *paths* moved out.
- **The staging and commit machinery.** `StagedWrite`, `MissingStagedTemp`, `Resolution`,
  `commit_swap`, `committed_matches_intent`, `sweep_transaction_staging`, `staged_paths`,
  `node_dir`, `resolve_copies`, `enumerate_documents`, `recover`, and `roll_forward` are
  each called from the facade *and* from two or more parts. `recover` is the sharpest case:
  it is called by `open`, which now lives in `store/genesis.rs`.

Widening these to `pub(super)` would make a deeper split compile, and is deliberately
rejected: a split is a pure relocation, visibility changes are not part of one, and a
widened item stays in a module that no longer owns it while looking like it does.

**This decision schedules no work.** It records why the facade is the size it is, so the
next reader does not re-derive the constraint by attempting the same split again.

## A store root has one writer, and it is a guarantee you may rely on (`D116`) -- [store/genesis.rs](src/store/genesis.rs)

> **SUPERSEDED in part, 2026-11-09, by "Review fixes, 2026-11-09" below.** `init` now takes
> the lock *before* its empty-root scan, not after; and within one process every clone of a
> store, and every monitor spawned from one, shares a writer gate, so "one writer" also
> holds in-process. Read that section before relying on the ordering described here.

**For consumers.** While you hold a [`Store`], no other process can open that root. `Store::init`
and `Store::open` take an exclusive lock on the root through the `Vfs`; a root someone else holds
is refused with `StoreError::RootInUse`. You do not need to build your own interlock on top of
this, and you should not: a second, weaker mechanism beside this one is how the guarantee gets
quietly lost.

This is written down because the failure mode is silence. A consumer that does not know the
guarantee exists hand-rolls something narrower -- a pid marker, a "is anyone else running?" probe
-- and then two mechanisms disagree.

**What it is and is not.**

- **Whole-root, writers only.** `D8`'s atomic rename already isolates a reader from a landing
  write, so readers are not excluded and need no lock.
- **Released by the last handle, not the first.** `Store` is `Clone` and `WriteMonitor` owns a
  clone, so the clones are one writer rather than several. Cloning does not re-acquire.
- **Released when the owning process dies, by any means.** It is an OS lock, so a killed or
  crashed holder does not leave the root wedged. This is the property the mechanism was chosen
  for, and it is what a pid marker file could not have given.
- **Not a lease and not a queue.** Acquisition never blocks and never waits its turn: a contended
  root fails immediately. Waiting would turn "two daemons pointed at one Shire" into a hang, which
  is harder to diagnose than a refusal.
- **Scoped to the filesystem you handed in.** Over `OsFs` the scope is the machine. Over `MemFs`
  it is the process -- which is that filesystem's whole universe, so it is faithful rather than a
  stub, but it means a green `MemFs` test says nothing about cross-process behaviour. The
  cross-process witnesses live in [tests/root_lock.rs](../canopy-lock-holder/tests/root_lock.rs) and spawn a real second
  process.

**The lock file is not a signal.** `.lock` at the root is left behind on release and is never
reaped. Its presence means nothing at all; only a live holder excludes anyone. Anything that
treats the file as evidence of a running process, or deletes it to "clear a stale lock", is wrong
in both directions.

**Why `open` validates before it locks.** A directory that turns out not to be a store never has a
lock file dropped into it. Acquisition is still the serialization point -- validation is read-only,
so two concurrent opens can both pass it and exactly one wins the lock. `init` is the other way
around: it locks after its shape checks but before its first write, because two concurrent `init`s
can both pass the empty-root scan, and there the lock is the only thing that stops the loser.

**What this does *not* cover.** Only canopy store roots. A plain directory tree beside a store --
gaffer's `spill/` and `exec/` artifact roots, for instance -- is not a store and gets none of this;
a consumer that wants exclusion over such a tree can use `Vfs::lock_exclusive` directly, but that
is its own mechanism with its own scope, not this guarantee.


## `ContentDigest` gained `Serialize`/`Deserialize` and `from_hex` for reuse outside the store (`D298`)

Added 2026-09-16 while implementing `ECAT-3` (`CHECKLIST.md#plan-ecat`): canopy-core's
engine-catalog voucher document (`D298`) needed a digest field, and `ContentDigest` already
*is* "SHA-256 over some bytes" with an existing `to_hex`/`Display` -- reusing it kept one
digest type flowing from the voucher through the eventual content-addressed store region
(`ECAT-5`) rather than introducing a second, parallel digest representation in canopy-core.
`from_hex` is the inverse of the existing `to_hex`; `Serialize`/`Deserialize` round-trip
through that same hex string, so a digest reads and writes identically whether via
`Display`/`from_hex` or via `serde_json`. Purely additive: no existing caller's behavior
changed, and `ContentDigest`'s own identity (SHA-256 over raw bytes, `D37`) is untouched.

## Quorum Failure Diagnostics (QDIAG-1)

> **SUPERSEDED in part, 2026-09-25, by `CVS-19.3`.** The quorum verify-read over document
> bodies no longer exists, and with it the `quorum_read_no_quorum` and `quorum_read_started`
> events and the whole first paragraph below. A document read now resolves a pointer and
> fetches content by digest (the originating repository's `DESIGN-NOTES.md` `D333`), and reports
> being outrun as an outcome rather than logging about it (`D336`). The **publication**
> events (`partition_apply_started` / `partition_apply_finished`) survive unchanged, on the
> same `canopy::quorum` target; that target name now outlives the module it was named for.

> **SUPERSEDED in name only, 2026-09-22, by the originating repository's `DESIGN-NOTES.md` `D319`.**
> The error is now `Unsettled`, and the read no longer claims damage. The warn line, its
> fields, and the reasoning below are unchanged -- read `NoQuorum` as `Unsettled` throughout.

The sequential verified read is not a simultaneous snapshot. A `NoQuorum` error's
counts alone cannot distinguish stable disagreement from samples crossing multiple
publications. Quorum reads (`src/store/quorum.rs`, retired) therefore emit a WARN event on
`canopy::quorum`, `quorum_read_no_quorum`, with root, node, document, process-local read ID,
counts, index-ordered `(length, sha256)` samples (`None` for absence), and per-partition
read start/end offsets in monotonic nanoseconds. These are the original sampled bytes,
not a second read after the failure; document contents are never logged. The warning does
not claim corruption or repair the document. With this target disabled, timing collection
and failure hashing are skipped. Read IDs are correlation labels, not synchronization.

At DEBUG, `quorum_read_started` correlates the warning with
`partition_apply_started` / `partition_apply_finished` from
[publication diagnostics](src/store.rs) (`Store::trace_swap`; these lived in
`src/store/diagnostics.rs` until `CVS-19.3` retired that module). Publication events include root,
transaction nonce, partition, node/document, live versus recovery mode, intended
digest/length or removal, success/error, and monotonic operation duration. They surround
the existing transactional apply operation; completion includes its durability work, not
just rename. They do not change its error or ordering semantics, acquire a store lock,
inspect the commit log, or trigger additional filesystem reads. Nontransactional direct
writes/repairs are not covered by the transaction events; absence of these events is not
proof that no external mutation occurred.

For an executable with the usual komorebi subscriber, append `canopy::quorum=debug` to
`RUST_LOG` (for example `info,canopy::quorum=debug`). Retain stderr or the configured log
sink. The CAS-race integration test (`canopy-core/tests/transactional_facade/cas_races.rs`, originating repository)
installs a process-global, thread-labelled test-writer subscriber for this target so the
worker and monitor events appear automatically in nextest's failure output. No environment
setup is required for that test. Verbose tracing can perturb scheduling, so a passing run
with tracing does not settle an intermittent failure.

QDIAG-1 adds observability only; the earlier failure was subsequently resolved by TCAS-1
in core's monitor-serialized handle reads, recorded in
`RESOLVED-TEST-FAILURES.md`. Rationale is in `DESIGN-RATIONALE.md`
(`qdiag-1-quorum-failure-evidence`), and the work is recorded in `CHECKLIST.md`
(`plan-body-voucher`) -- all in the originating repository, not imported here.

## Transaction Failures and Snapshots (ECAT-12)

> **SUPERSEDED in part, 2026-11-09, by "Third review fixes, 2026-11-09" and "Fourth review
> fixes, 2026-11-09" below.** A failure after `prepare` fences every handle to the store, not
> only the failing monitor: while the fence is up, monitor operations report
> `StoreError::RecoveryRequired` rather than `MonitorError::Stopped`, and no monitor runs a
> snapshot or plan.

> **SUPERSEDED in part, 2026-11-09, by "Review fixes, 2026-11-09" below.** A snapshot closure
> now runs while the monitor holds the store's writer gate, so in-process direct-store writes
> *are* serialized against it; only writes from another process are not.

The [monitor](src/monitor.rs) must not truncate the journal after an ambiguous prepared
transaction failure. Such a failure stops the actor; queued and subsequent operations,
including barriers and snapshots, return `MonitorError::Stopped`. The caller must drop
the old store handles and reopen, allowing the existing journal recovery to finish before
constructing another monitor. An unreadable journal or failed rollback also stops it.
For a transaction proven to have no prepare record, staging is swept and the journal
reset before the error reply; ordinary retry on that same monitor remains supported.
The original store error is preserved for the failing operation. A transaction CAS
rejection is not an error and does not stop the actor.

`WriteMonitor::snapshot` runs a bounded caller read closure between monitor commands.
All reads in that closure see one state relative to that monitor's writes; the closure
must never call back into the monitor. It does not serialize external direct-store
writes and does not change the lock-free direct-read baseline. Engine/catalog mutation
snapshots use this primitive so a partially applied replicated removal is not mistaken
for persistent corruption. Final writes still use CAS and retry on actual conflicts.

[Monitor regressions](src/tests/monitor.rs) cover failed prepare/apply/end sequences,
successful reopen, stopped follow-up work, pre-prepare rollback/retry, and 100 concurrent
multi-document snapshot/write pairs. The prepared-failure regression was proved sensitive
by temporarily disabling the stop path and observing its expected failure.
Rationale: `DESIGN-RATIONALE.md` (`ecat-12-coordinated-adoption`).
Work: `CHECKLIST.md` (`plan-ecat`) `ECAT-12`. Both are in the originating repository, not
imported here.

## Review fixes, 2026-11-09 (`RF-11`..`RF-18`)

> **SUPERSEDED in part, 2026-11-09, by "Second review fixes, 2026-11-09" below.** The writer
> gate is now first-come-first-served, a scrub holds it per document rather than for the
> whole pass, and ingest stages the caller's stream outside it. `init` now scans the root
> once *before* locking as well as once under the lock, so `init` over an occupied root
> reports `NotEmpty` / `AlreadyInitialized` without touching it; `RootInUse` is reported
> only for a root that passes the unlocked scan.

- **One writer per process, not only per root.** The OS lock (`D116`) excludes other
  processes; within a process every `Store` clone and every `WriteMonitor` spawned from one
  share a re-entrant writer gate ([store/writer.rs](src/store/writer.rs)). Every public
  write verb holds it, and the monitor holds it for each burst of commands, so a
  check-then-write (a conditional swap, a transform) cannot interleave with another handle's
  write. Re-entrant per thread because the write verbs call one another.
- **A transaction that cannot apply never reaches `prepare`.** Namespace collisions -- a
  document named where a child node is, or a node nested under a document, on disk or
  between two writes of one set -- are refused before `begin`. Once `prepare` is durable
  recovery must roll it forward, and a roll-forward that always fails would wedge every
  later open.
- **The commit log is durable from its first record, and a torn tail is cut before the
  next append.** Creating the log fsyncs the store root; an append that finds the file
  ending mid-line truncates it back to the last newline first, so the new record cannot
  weld onto the fragment and turn a tolerated torn tail into a malformed complete line.
- **New directories are synced into their parents.** Shard and node directories created by
  a write have each new name fsynced into its parent, not only the deepest directory's
  entries, so a crash cannot drop a freshly created chain together with the files made
  durable inside it.
- **`init` takes the root lock before it checks the root is empty.** A check made without
  the lock could pass and then lose the root to another `init` that completed and released
  it. `init` over a held root therefore reports `RootInUse`.
- **A scrub reports staleness instead of passing it off as health.** Stale pointer replicas
  are still never repaired (`D320`), but they are listed in `ContentScrubReport::stale`,
  are not counted `whole`, and make the scrub not clean.
- **Blob replication copies in bounded chunks.** The staged first replica is copied to the
  others by positioned reads of `INGEST_CHUNK` bytes, so ingest memory is bounded by the
  chunk size, not by the blob.

## Second review fixes, 2026-11-09 (`RV-6`..`RV-15`)

> **SUPERSEDED in part, 2026-11-09, by "Third review fixes, 2026-11-09" below.** The
> monitor holds the writer gate per command rather than per burst, and `create_dir_durably`
> creates and syncs one directory at a time and remembers a name it could neither sync nor
> remove, rather than taking the whole chain back down.

- **The writer gate is fair, narrow, and held by a thread.** Entry is first-come-first-
  served by ticket, so a writer that re-enters in a loop (the monitor between bursts, a
  scrub between documents) cannot starve another handle. The gate covers the critical
  section only: a scrub takes it per document, so a pass over a large store does not hold
  every other writer off for its whole duration, and is therefore not a snapshot; ingest
  takes it after staging the caller's stream, because the staging temp is private to the
  nonce and a slow source must not stall every writer. `WriterGuard` is `!Send`, because the
  gate is owned by a thread and must be released by it.
- **A rollback collects what the aborted transaction ingested.** Before the log is reset,
  the blobs of every target named in `begin` are collected against the namespace, so a
  transaction that failed after ingesting cannot strand content once the record naming it
  is gone.
- **Refusals happen before side effects.** `write_document_replicated` checks namespace
  collisions before ingesting the blob, so a refused write leaves no unnamed blob.
  `repair_blob` refuses a replica index the store does not have
  (`StoreError::ReplicaIndexOutOfRange`) before touching anything. `init` refuses an
  occupied root before creating `.lock` in it, then scans again under the lock for the
  race the lock exists to close.
- **A durable-directory chain that could not be synced is taken back down.** A retry decides
  what to sync by what is missing, so `create_dir_durably` removes the (still empty) chain it
  created when a parent fsync fails; otherwise the retry would see the directories present
  and never sync their names.
- **Replication verifies what it copies.** Each replica copied from the staged first replica
  is hashed as it streams and refused (`StoreError::StagedReplicaMismatch`) if it does not
  match the digest computed while staging, so bytes that changed underneath an ingest are
  never committed.
- **A backlog drain never loses findings.** The drain takes the whole backlog up front; if a
  repair fails part-way, the failing suspect and every one not yet reached are noted again.
- **A burst that ends on a snapshot or a repair still owes quiescence.** If the burst applied
  a write, the monitor resets the commit log (and, after a snapshot, drains the repair
  backlog) before blocking for the next command.

## Third review fixes, 2026-11-09 (`RV3-1`..`RV3-9`)

> **SUPERSEDED in part, 2026-11-09, by "Fourth review fixes, 2026-11-09" below.**
> `MonitorError::Stopped` makes no claim that the log was truncated; while the store is
> fenced every monitor operation reports `StoreError::RecoveryRequired` instead.

- **A transaction that may have passed its commit point fences the whole store.** Every
  handle to a store shares one "unresolved" flag with the writer gate. `apply_transaction`
  rolls back a failure it can prove happened before `prepare` (staging swept, ingested blobs
  collected, journal reset); any other failure sets the flag. While it is set, every write
  verb and every journal reset through any handle -- including a sibling monitor's barrier or
  shutdown -- is refused with `StoreError::RecoveryRequired`, and monitors stop. Only a reopen,
  whose recovery finishes the transaction, clears it. The journal is shared, so a fence that
  covered only the failing handle would let another erase what recovery needs.
- **The journal's own name is durable before any record is trusted.** Until the store root
  has been synced after the log was created, every append syncs it again; a failure leaves
  that unknown so the next append retries. The knowledge is shared by every handle and lasts
  for one open.
- **Collision checks fail closed.** Only absence counts as "no collision"; any other error
  probing the namespace is returned before `begin`, because a collision a transient error
  hid would reach `prepare` and leave recovery a roll-forward that can never succeed.
  Ancestors are probed before the target, and an absent ancestor ends the walk.
- **Pointer repair re-reads under the gate.** `repair_pointer` takes the caller's report
  only as the finding that prompted it: the namespace is read again with the gate held, and
  if the majority has moved the report is obsolete and nothing is written.
- **Removals of a node and its descendants compose.** A child removed in the same
  transaction does not count against its parent's emptiness; each removal is still checked
  for its own emptiness, and removals are recorded and applied deepest first, so recovery
  replays them in an order the non-recursive remove accepts.
- **Directories are created and synced one at a time.** `create_dir_durably` creates each
  missing directory outermost first and syncs its parent before creating the next. One whose
  parent cannot be synced is removed; one that cannot be removed either is remembered and its
  parent synced by the next call, because a retry would otherwise find it present and never
  sync its name. Quarantine directories are created this way too, so sequestered evidence is
  never on a name a crash can lose.
- **The monitor holds the writer gate per command.** A transform's read and write still
  cannot interleave with another handle's write, but a queue that never empties no longer
  keeps every other handle out.
- **Published-crate tests are not built from the package.** The test suites depend on the
  unpublished `test-timers` crate as a path-only dev-dependency, which `cargo publish` strips.
  This decision schedules no work: the suites are run from the repository, and making them
  build from the package would mean publishing test support that exists to be private.
- **Crash sweeps discard volatile state.** The phase-boundary sweeps run both as a restart
  over the same live state and after `simulate_crash`, and a sweep failing each `fsync` in turn
  runs only the latter, since only a crash makes a failed sync observable.
- **A barrier never reports a truncation that did not happen.** `WriteMonitor::quiesce` returns
  the reset's own error, and while the store is fenced it returns
  `StoreError::RecoveryRequired` whether or not the actor is still running. `Stopped` is
  reserved for an actor that exited for another reason, whose drain-and-stop did truncate. The
  automatic resets after a drain and at shutdown still discard their result, because nobody is
  told they happened; an actor that observes the fence after any command exits.

## Fourth review fixes, 2026-11-09 (`RV4-1`..`RV4-7`)

> **SUPERSEDED in part, 2026-11-09, by "Fifth review fixes" (`RV5-2`).** The directory-debt
> record is not best effort: a directory is recorded durably before it is created, and is not
> created if the record cannot be written. Read the fifth-round decision before acting on the
> third bullet.

- **Removals are checked for collisions before `begin` too.** Removing a name that is a child
  node, or one beneath a document, can never apply; refusing it only after `prepare` would
  fence the store and make every reopen retry it.
- **A removal syncs its parent even when the entry is already gone.** A replayed removal cannot
  tell an entry that was never there from one whose removal was never made durable, and blob
  collection after `end` turns the latter into a pointer that could come back naming a deleted
  blob. Node removals do the same.
- **Unsynced-directory debt is persisted.** A directory whose name could be neither synced nor
  removed is recorded in `.dir-sync-debt` at the root as well as in memory, and `open` loads and
  discharges it before recovery. Writing the record is best effort; a store that cannot even
  write it still covers the process that saw the failure.
- **A fenced monitor runs no callback.** The actor checks the store-wide fence under the gate
  before every command, so a snapshot or plan cannot observe a store another handle left
  half-applied. Every monitor call that finds the actor gone reports
  `StoreError::RecoveryRequired` while the fence is up, and `Stopped` otherwise; `Stopped`
  makes no claim about the log.
- **A pointer may only name its own document's blob.** `Pointer::check_owner` requires the
  recorded owner and the target blob's owner both to equal the document's path hash; `parse`
  applies it to every read (`PointerError::ForeignTarget`) and `write_pointer` to every write
  (`StoreError::InvalidPointer`). Collection follows a blob's owner, so a foreign target would
  dangle once its real owner changed.
- **An empty commit log is flushed until known flushed.** A truncation whose flush failed looks
  like a clean empty log, so a reset of an empty log flushes it once per open and skips the
  flush only after one has succeeded; any append clears that knowledge.

## Fifth review fixes, 2026-11-09 (`RV5-1`..`RV5-8`)

> **SUPERSEDED in part, 2026-11-09, by "Sixth review fixes" (`RV6-2`, `RV6-3`, `RV6-5`).** A
> stale directory record costs more than needless syncs: it can name a path beneath what is
> now a document, and a torn append can corrupt the record. The `MemFs` reclamation bullet's
> trigger is also replaced. Read the sixth-round section before acting on either bullet.

- **A set's own writes are checked against its removals.** A removal whose name another write
  in the set nests beneath, or whose path passes through a document the set writes, can never
  apply; it is refused before `begin` with `StoreError::NameIsChildNode`.
- **A directory is recorded before it is created.** `create_dir_durably` appends the chain it
  is about to create to `.dir-sync-debt` and flushes it first, so a directory whose name sync
  fails is known to a reopen even if this process cannot remove it. If the record cannot be
  written, nothing is created. On success the record is truncated without a flush; a stale
  record only costs the next open some needless parent syncs.
- **On Windows only, a nonce that is exactly a reserved device name is substituted where it
  forms a whole path component.** Its first character becomes U+00A9, the copyright sign (`CON` becomes U+00A9 then `ON`), keeping
  its length in characters; the character is outside the nonce set, so the result never equals
  another nonce. Only the pointer staging directory `.staging/<nonce>` is affected: everywhere
  else a nonce follows a `.`, where Windows does not apply the rule. Nothing reads a nonce back
  from a path, and the logical nonce (`Nonce::as_str`, the commit log) is unchanged. Other
  platforms use the nonce as is.
- **A node removal checks the physical directory too.** A node that lists empty but holds an
  entry the namespace does not recognise (a dotfile, an unparseable name, another file type)
  is rejected before `begin` with `RejectedPrecondition::NodeHoldsUnrecognizedEntries`, because
  the non-recursive remove would fail on it after `prepare`.
- **The modification count is raised on both sides of a modification.** It is odd while a
  modification is in progress and even otherwise, including after one that failed, so a reader
  sampling it across a writer paused between replicas sees contention rather than "nothing
  happened". `Store::undisturbed(before, after)` is the test: equal and even.
- **`MemFs` reclaims unreachable file data**, after a directory sync that drops or replaces
  entries and after a crash: live bytes not named by the live tree, and durable bytes named by
  neither tree nor any carried snapshot.

## Sixth review fixes, 2026-11-09 (`RV6-1`..`RV6-8`)

- **A write is checked against the set's node removals.** If a written document's entry path
  is a removed node's path or one of its ancestors, the write turns that path into a file
  before the removal runs, and the removal would then fail after `prepare` on every recovery.
  `check_transaction_namespace` refuses the set before `begin` with
  `StoreError::NameIsChildNode`. A removed node *above* a write is still refused when the set
  is built (`SwapSet::with_node_removals`).
- **A path beneath a file is absent.** `store::is_absent` treats `NotFound` and
  `NotADirectory` alike, because POSIX reports a path under a file as "not a directory" where
  Windows reports "not found", and either way nothing is there. Pointer reads, child listings,
  pointer removal, a node removal's physical check, and directory-intent discharge use it.
  This is what lets a reopen discharge an intent for a directory a later write replaced with a
  document, and lets recovery collect an old transaction whose path is now beneath one.
- **The directory-intent record is parsed and appended defensively.** Only newline-terminated
  records count, and only plain relative paths below the root are followed. An append that
  fails marks the record possibly torn, and the next append first cuts it back to its last
  complete line, so a retry can never extend a torn prefix into a wrong path that a reopen
  would then discharge in place of the directory actually created.
- **A blob reference is constructed only at a width its parser accepts.** `BlobRef::new`
  returns `BlobNameError::ShardWidth` for a width of zero or wider than a digest, so a
  reference always renders a target `BlobRef::parse` reads back. `write_pointer` additionally
  refuses a target at another width than the store's (`PointerError::ShardWidth`), since it
  names a shard the store never writes into.
- **Repairs are modifications.** `repair_blob` and `repair_pointer` hold a modification guard
  across their sequestering and replacement, so a reader whose samples bracket a repair sees
  the count move. A pointer repair whose report is obsolete writes nothing and moves nothing.
- **`MemFs` reclaims incrementally** (see acadia's [DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md),
  sixth review fixes).

## Seventh review fixes, 2026-11-10 (`RV7-1`, `RV7-2`, `RV7-5`, `RV7-6`)

- **Collection makes its keep-set durable before deleting.** `collect_document_blobs` reads
  the keep-set from the live namespace, which can be ahead of the durable one when a pointer
  write renamed its entry but could not sync the parent. It syncs the nearest existing
  directory above the entry on every replica (`Store::sync_pointer_entry`) before
  `retain_blobs` deletes anything, so no crash can restore a pointer to a blob it removed.
  Failing that sync fails the collection, deleting nothing.
- **A node removal at or beneath an existing document is refused before `begin`**, with
  `StoreError::PathIsDocument` naming the shallowest document on the path
  (`check_node_path_is_not_document`). Replaying a node removal treats a path beneath a file
  as already removed, but still fails if the node itself is a file.
- **A repair names blobs at the store's shard width only.** `repair_blob` refuses another
  width with `StoreError::ReferenceShardWidth` before moving anything, so it never reports a
  repair at a path the supplied reference does not read.
- **`canopy-lock-holder` reports through one `Reporter`**: the handshake to stdout, flushed;
  failures to stderr.

## Formal state model in TLA+ (`FM-1`), 2026-11-10

- **The protocol is specified in TLA+ and the code is checked against the specification.**
  Crash, partial-failure and replica-disagreement interleavings are enumerated by TLC rather
  than imagined by reviewers. Specs live in `spec/` in this crate, one module per protocol
  slice, each action citing the function it abstracts.
- **Conformance is by trace replay.** TLC behaviours are replayed step by step against the
  real `Store` on `MemFs` + `FaultyFs`, and a projection of the store's live and durable state
  must equal the model state after every step. A spec property is only trusted once a
  revert-probe of the fix it covers makes the check fail.
- **TLA+ rather than a Rust-native model checker**, because the team reviews TLA+; the cost
  is a JDK and `tla2tools.jar` in the toolchain and a trace bridge for conformance.
- **The first slice is non-transactional pointer publication, collection, and crash**,
  because the open `E1` defect (no pointer majority after consecutive failed writes) lives
  there, and its fix is decided from the model. Work: [CHECKLIST.md](CHECKLIST.md).
