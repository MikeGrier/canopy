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

> **SUPERSEDED in part, 2026-11-10, by "Several processes may open one store".** The root is to
> admit several processes, with writers taking turns under a per-write cross-process lock, so
> "no other process can open that root" and "acquisition never waits" describe the lock only
> until that work lands. The OS-lock properties below -- released on death, the lock file is
> not a signal, scoped to the filesystem handed in -- carry over.

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

> **SUPERSEDED, 2026-11-10, by "The public write surface belongs on `Store`" below.**
> `WriteMonitor` is removed; its verbs, its fence behaviour and its snapshot are `Store`'s,
> and `Store::snapshot` is optimistic rather than serialised against writers. Read that
> decision before acting on anything here.

> **SUPERSEDED in part, 2026-11-09, by "Third review fixes, 2026-11-09" and "Fourth review
> fixes, 2026-11-09" below.** A failure after `prepare` fences every handle to the store, not
> only the failing monitor: while the fence is up, monitor operations report
> `StoreError::RecoveryRequired` rather than `MonitorError::Stopped`, and no monitor runs a
> snapshot or plan.

> **SUPERSEDED in part, 2026-11-09, by "Review fixes, 2026-11-09" below.** A snapshot closure
> now runs while the monitor holds the store's writer gate, so in-process direct-store writes
> *are* serialized against it; only writes from another process are not.

The monitor (`src/monitor.rs`, retired) must not truncate the journal after an ambiguous prepared
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

Monitor regressions (`src/tests/monitor.rs`, retired) cover failed prepare/apply/end sequences,
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

## Formal state model in TLA+, 2026-11-10

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

## Naming the stranded pointer vote (`STRANDED-VOTE`), 2026-11-10

- **`STRANDED-VOTE` is the defect in which the pointer replicas for a document hold no
  majority -- neither a version nor absence -- while no writer is running.** A read then
  reports `Contended` indefinitely, `repair_pointer` refuses, and collection skips the
  document, so nothing converges. Consecutive non-transactional writes that each fail part-way
  through publishing their pointer reach it without any crash (`[C, C, A]`, then `[B, C, A]`).
- **This name supersedes the review-round label `E1`** (eighth review round,
  2026-11-10), which earlier records -- the formal-model decision above, the
  [formal-model design session](design-sessions/DESIGN-SESSION-2026-11-10-formal-model.md), and
  the first spec slice's entry in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) -- use and
  keep as written.
- Its fix is chosen from the model, as queued in [CHECKLIST.md](CHECKLIST.md), and accepted when
  [PointerPublication-Settled.cfg](spec/PointerPublication-Settled.cfg) passes.

## Naming the dangling durable pointer (`DANGLING-DURABLE`), 2026-11-10

- **`DANGLING-DURABLE` is the defect in which blob collection deletes the blob that the
  durable pointer majority still names**, so a crash restores a pointer to content that no
  longer exists. It arises because collection reads its keep-set from the live namespace, which
  can be ahead of the durable one when a pointer rename's parent sync failed. Fixed by having
  collection sync the pointer entry on every replica before deleting anything;
  `DurablePointerHasBlob` in
  [PointerPublication.tla](spec/PointerPublication.tla) is the invariant, and
  [PointerPublication-NoCollectSync.cfg](spec/PointerPublication-NoCollectSync.cfg) the control
  that reproduces it without the fix.
- **This name supersedes the review-round label `D1`** (seventh review round, 2026-11-10),
  which earlier records keep as written.

## Guards against `STRANDED-VOTE`, compared in the model, 2026-11-10

Four guards were modelled in [PointerPublication.tla](spec/PointerPublication.tla), selected by
its `Fix` constant, each checked against every invariant (one configuration per guard,
`PointerPublication-Fix<Guard>.cfg`). Pointer replica counts are odd
(`with_pointer_replica_count` refuses an even one), so any two values always have a majority;
every guard works by keeping a document to at most two values, live and durable.

- **Resolve before each write -- holds.** Before ingesting, the write reads the vote, sets every
  replica to the live majority (a rename only where it differs) and syncs every replica's
  parent. Publishing then starts from durable unanimity, so any interruption leaves at most the
  old value and the new one. Cost: one parent fsync per pointer replica on every write, whether
  or not anything disagreed; no new on-disk state. A failed write converges to whichever value
  the majority held, which may be the version the caller was writing.
- **A durable publication record -- holds.** After ingest and before the first pointer rename,
  the write durably records the version it is publishing; it clears the record once every
  replica is renamed and synced. A write that finds a record for its document first rolls every
  replica forward to it; after a death or a restart, recovery settles every record, and opening
  never does. Cost: two fsyncs per write (record and clear), plus a new
  store-root file with the torn-append and stale-record handling `.dir-sync-debt` needed. A
  failed write converges to the version the caller was writing. **Its scope is every operation
  on the document, not only unlogged publication:** a pending record must be settled before any
  precondition check, publication -- logged or not -- collection, repair or scrub of that
  document. Otherwise a logged write of C can land and collect B while a record for B is
  pending, and replaying the record later installs B over C, naming content that no longer
  exists. The model as checked writes and clears the record atomically and never mixes logged
  with unlogged writes, so this guard's "holds" is provisional until it does both.
- **Resolve only when the live replicas disagree -- fails.** A write whose renames all landed but
  whose last parent sync failed leaves the live replicas unanimous and the durable ones not; the
  next write skips the resolve, and a crash then exposes three values. Unanimity of the live
  namespace says nothing about the durable one, so the resolve must sync unconditionally.
- **Fence the store after a partial publication -- fails.** The fence lives in the process, so
  closing and reopening clears it, and the next partial publication strands the vote. A fence
  would only work with durable knowledge of which document to repair, which is the publication
  record.

Which of the two holding guards to implement is the engineer's decision; it is the first step of
the implementation item in [CHECKLIST.md](CHECKLIST.md).

## The public write surface belongs on `Store`, 2026-11-10

- **Consumers write through `Store`.** The document-write verbs a consumer needs --
  read-modify-write, compare-and-set, planned transactions, consistent snapshots, the commit
  feed -- are methods on `Store`, alongside the transactional and lower-level writes, rather than
  on a separate `WriteMonitor` handle.
- **How they are implemented is not part of the surface.** They may delegate to the monitor
  actor, commit through the commit log, or replace the actor entirely; the choice follows from
  what the actor is still needed for. The store's writer gate already serialises every write in
  the process (shared by every `Store` clone and every monitor), which covers the
  serialisation the actor was introduced for (`D41`); what remains to be justified is its
  thread and its idle-time housekeeping.
- **The migration is conservative.** `WriteMonitor` is kept only where an inventory of its uses
  finds a cause, and is narrowed or removed only after every caller has moved. The surface
  change lands before the first crates.io release, so no published API is removed.
- **It precedes the `STRANDED-VOTE` fix,** because the set of public entry points that publish
  pointers without the commit log -- the writes that fix must guard -- depends on it.

Work: [CHECKLIST.md](CHECKLIST.md), milestone M2.

### What `WriteMonitor` provides, and what each part needs

Read out of `src/monitor.rs` (retired). "Gate" means the store's writer gate, which every
`Store` clone and every monitor already share, and which the actor itself takes once per
command.

| Capability | Today | Needs |
|---|---|---|
| Serialising writes | one actor draining a queue | gate -- already FIFO and re-entrant |
| `transform` (no lost update) | read, closure, `write_document_replicated`, all on the actor | gate held across the read and the write; the closure then need not be `Send + 'static` |
| `write` | a `transform` that ignores the body | gate |
| `conditional_write` | digest compared outside the commit log, then `write_document_replicated` | gate, or a transaction with an `ExpectedDigest` precondition |
| `apply_transaction` | `Store::apply_transaction` plus nonce minting and the commit feed | nothing beyond what `Store` has |
| `plan_and_apply` | plan run on the actor, then `apply_transaction` | gate held across the plan and the commit |
| `snapshot` | caller's read closure run on the actor between commands | gate |
| Nonce minting | the actor owns the caller's `NonceSource` | a home for the source: a nonce parameter per call, or a source installed on `Store` |
| Commit feed (`observe`, `CommitObserver`) | called by the actor for the writes it serves; direct `Store` writes bypass it | a hook in `Store`'s write paths, which would also make the feed complete |
| Fencing and `Stopped` | actor exits when the store is fenced | nothing -- `Store` already fences, and `Stopped` has no other cause once there is no actor |
| Commit-log reset when idle | run when the queue drains, ahead of the last caller's reply | an idle point: the gate can observe its last holder leaving; the cost is already charged to the last caller today |
| Repair-backlog drain when idle | run after the last caller's reply, so no write pays for it (`D339`) | **a thread**, or an explicit call -- the only capability that needs one |
| Runtime neutrality (`D33`) | blocking submit on a plain OS thread | nothing -- `Store` methods are blocking already |

**Callers in the workspace.** The monitor's own unit tests
(`src/tests/monitor.rs`, retired) exercise every verb, the commit feed, snapshots and
the fence. Integration tests use it as a write path: `canopy_m3` (spawn and write),
`nontransactional_write` (`write` and `conditional_write` -- the unlogged path), and
`scrub_and_repair` (`repair_reported_damage`). The crate docs ([lib.rs](src/lib.rs)), the
[README](README.md) and the writer gate's module doc ([writer.rs](src/store/writer.rs)) describe
it.

**Finding.** The writer gate covers everything the actor was introduced for. Only draining the
repair backlog off the caller's latency path needs a thread; nonce minting needs a home on
`Store`; the commit feed is better placed on `Store`, where it would see every write instead of
only the monitor's.

### Choices made for the surface

- **Draining the repair backlog is an explicit call, with no thread.** It joins scrubbing among
  the maintenance the embedder schedules (`D340`).
- **Every document verb that is neither management nor repair commits through the commit log**,
  conditional and read-modify-write verbs included.
- **`write_document_replicated` is the deliberate exception:** it stays unlogged (`D332`). It
  still publishes pointers, so the `STRANDED-VOTE` guard covers it.
- **The store's building blocks are crate-private:** `write_pointer`, `ingest_blob` and
  `retain_blobs`. Each was public only because the store was built layer by layer, and each is
  unsafe in a caller's hands: `write_pointer` can name a blob that is absent, never collects the
  version it replaces, has no precondition, and would be a third unlogged publisher;
  `retain_blobs` can delete a blob a live pointer still names; `ingest_blob` alone only strands
  content. What a caller could want from them -- pointing a document at an existing blob, as a
  revert -- is unsafe anyway, since collection may already have removed that blob.
- **Nonces come from a source supplied at open, and need only be statistically unique.** A
  nonce names a write's staging and a transaction's commit-log records, so what correctness
  needs is that no two operations whose traces are on disk at the same time share one -- across
  every process. A sequence would have to be durable and trusted across restarts, which the
  coordination state is not; 128 random bits make a collision far less likely than an
  undetected hardware error, with no shared state at all. canopy still mints none of its own
  (`D61`): the embedder supplies the source, canopy calls it once per operation (recovery and
  repair included), and each outcome reports the nonce it used so the caller can correlate.
  canopy ships a random source behind a cargo feature for embedders who do not want to write
  one.
- **A collision fails an operation; it never corrupts.** An operation claims its nonce before
  it creates anything named by it: every nonce-named artifact -- blob staging, pointer staging,
  transaction staging, quarantine destinations -- is created exclusively, or beneath a claim
  that was, and none replaces an existing entry. A `begin` whose transaction id the commit log
  already holds is refused. Recovery reusing a transaction's staging is not a collision: it owns
  the nonce through that transaction's `begin` record. Correctness therefore never rests on the
  probability argument alone.
- **Tests choose their randomness deliberately.** Unit tests draw nonces from a seeded
  pseudo-random generator with a fixed seed, giving each actor its own stream derived from that
  seed and running schedules that inject faults in a controlled order, so every run is
  identical; the generator is a specified algorithm pinned by version, so a seed reproduces
  across dependency upgrades. Longer-running integration tests seed it from a hardware entropy
  source and print the seed; where several actors race, they also record the fault trace, since
  a seed alone cannot reproduce scheduling.
- **The commit log is reset only when it holds no unresolved work, and only under exclusion**:
  by `quiesce`, and automatically by a holder before it releases exclusion once its own
  operations are complete. Exclusion is held throughout, so a reset never overlaps another
  process's transaction.
- **The surface, by how each verb commits:**

  | Class | Verbs | Exclusion | Nonce |
  |---|---|---|---|
  | Logged | `update_document(swap)`, `apply_transaction(swaps)`, `transform(node, name, f)`, `plan_and_apply(f)` | writer | from the source |
  | Unlogged write | `write_document_replicated` | writer | from the source |
  | Read | `read_document`, `list_children`, `list_documents`, `read_once` / `read_settled` / `read_within`, `read_pointer`, `read_blob`, `verify_blob`, `snapshot(f)` | none | none |
  | Management | `init`, `open`, `properties`, `attached_handles`, `set_agent_description`, `upgrade_schema`, recovery discovery and its incremental run, `quiesce`, `observe` | only where it mutates | where it writes |
  | Repair | `scrub_content`, `repair_reported_damage`, `repair_blob`, `repair_pointer` | writer | from the source |

  `Swap` already expresses an unconditional write, a compare-and-set on a digest, a create-only
  write and a removal, so the monitor's `write` and `conditional_write` need no verbs of their
  own; convenience wrappers over `update_document` are added only if building a `Swap` proves
  clumsy for callers. `transform` reads under exclusion and commits as a swap conditional on the
  digest it read. The targeted repairs stay public for administrative tools, because they only
  restore toward what the majority holds.
- **`snapshot` is optimistic.** It runs the caller's read closure, samples the modification count
  before and after, and runs it again while disturbed, up to a bound. It holds no exclusion, so a
  long snapshot never blocks a writer in another process; the closure must be safe to run more
  than once. It is sound only because the count brackets whole logical operations, so a
  snapshot taken between two swaps of one transaction sees an odd count and runs again.
- **Callbacks see a read-only view.** The closures `transform` and `plan_and_apply` run under
  exclusion receive a view that can read but has no writing methods, so a write from inside one
  is a compile error rather than a deadlock against the exclusion its own thread holds, and its
  reads take no exclusion. A clone of the store captured by the closure can still write:
  exclusion is re-entrant on the thread that holds it, so that write lands at once, between the
  closure's reads and the commit. `transform` commits conditionally on the digest it read, so
  such a write rejects the commit rather than being lost; a plan's swaps are protected only by
  the preconditions the plan gives them. The documentation warns against it.
- **The commit feed reports every write** -- logged commits and `write_document_replicated`
  alike -- but not repairs, which restore content rather than change it. It reports this
  process's writes only.
- **A claim is a directory, `.claims/<nonce>` under the root**, created with an exclusive
  `create_dir` and made durable before anything named by the nonce exists. Taking it also
  refuses a nonce whose blob or namespace staging an earlier operation left behind, since
  nothing would tell the two operations' staging apart. It is removed when the operation ends,
  except after a failure that fences the store, when recovery still needs that nonce's traces.
  Open removes every claim once recovery has finished: a transaction's nonce stays owned by its
  records in the commit log, and an interrupted write's orphaned staging still refuses its
  nonce, so releasing the claims loses no protection. Kept quarantine evidence outlives its
  claim, so a quarantine name that already exists is refused where the evidence is kept.
- **The automatic reset is made by the outermost write on the thread holding exclusion**, after
  its operation completes and before exclusion is released; a nested write leaves it to the
  outermost. A reset that fails or is refused is left for the next open to replay.
- **`WriteMonitor` is removed, not kept crate-private.** Every capability it had is `Store`'s,
  and the one that needed its thread -- draining the repair backlog off the callers' latency
  path -- is the explicit `repair_reported_damage`. A crate-private actor would be a second
  write path with no caller.
- **Every public write draws its nonce from the source; none takes one.** `init` and `open`
  take `StoreOptions`, built either with a source or explicitly without one, so no store lacks
  a source by omission. The explicit-nonce forms of the verbs remain as crate-private
  internals, for the unit tests that must name a nonce to place a fault or inspect staging;
  integration tests name nonces through the source instead.

## Several processes may open one store, 2026-11-10

- **The target is several processes on one host opening the same root, readers and writers
  alike.** Writers take turns: each write holds writer exclusion for its duration, across
  every thread of every process, instead of one process holding the root for its whole lifetime
  (`D116`). The layout of documents, blobs and pointers and the read and write algorithms are
  unchanged; the root's metadata and the protocol that coordinates access are new, and so is who
  may run recovery.
- **The scope is one host and a local filesystem.** Both platforms provide what is needed: an
  OS lock released when its holder dies -- `LockFileEx` on Windows; `flock` or
  open-file-description locks on Linux, never process-associated `fcntl` locks, which any close
  of the file releases -- and a way for processes to share a few counters.
- **Network filesystems are unsupported, and are disclaimed rather than detected.** Every
  guarantee rests on the local OS's lock, rename and fsync behaviour, which SMB, NFS and other
  remote or distributed filesystems do not reliably provide. canopy does not try to recognise
  or refuse a networked root, because detection is an unbounded chase of mount types, mapped
  drives and redirectors; the README, the crate docs and `Store`'s docs state the restriction
  instead.

### The coordination contract

Threads and processes coordinate only through a specified contract, the **coordination state**,
which owns every synchronization the store needs as well as the shared counters. canopy depends
on the contract, never on how an implementation meets it, and uses no other primitive to
coordinate access to a root -- no lock taken directly through the `Vfs`, no in-process gate of
its own. Exclusion and counters live behind one interface because their ordering guarantees are
stated against each other, so one implementation must provide both. A straightforward
implementation (OS locks and a root file) and a high-performance one (a shared memory mapping
with its own locks) are interchangeable.

The interface is a layer above `Vfs` and complements it rather than wrapping it: it decides who
may act on a root and when, and canopy still performs every data operation -- blobs, pointers,
the commit log, staging, fsync and rename -- through `Vfs` directly. An implementation is built
from `Vfs` primitives (so the file-based one runs over `OsFs`, `MemFs` and `FaultyFs` alike) or
from OS facilities the `Vfs` does not model, such as shared memory. `Vfs` keeps its own
primitives for its other consumers, and gains only what an implementation needs from it.

#### Identity and creation

- **Several processes is a new canopy format, behind a barrier.** The genesis format version
  advances, so a binary that predates this design refuses the root rather than opening it under
  the whole-lifetime root lock (`D116`) and ignoring coordination state it does not know. Until
  a root is migrated, a current binary refuses it with a distinct error naming the migration.
  The barrier lands with the first new on-disk state an older binary would misuse -- the
  `STRANDED-VOTE` guard's publication record, if that guard is chosen -- not later.
- **Migrating a root requires it offline.** An older binary that has read the old marker but
  not yet taken the root lock is invisible to the migration, and today's `open` does not read the
  marker again once it holds the lock, so no lock can fence it out. The migration therefore
  requires, as an operator precondition it states rather than checks, that no older binary is
  running against the root or starting to open it. Current binaries read the marker again after
  taking any lock, so the same window cannot open for them.
- **Migration is crash-consistent and resumable.** It takes the old root lock, reads the marker
  again under it, durably records a migration record naming itself and its inputs, writes the
  implementation record and the root's other new metadata durably, and only then publishes the
  new marker atomically and syncs it. Coordination state is volatile: it is created untrusted,
  not migrated. A migration that finds a migration record resumes it idempotently if its inputs
  match, and is refused if they differ.
- **One implementation per root.** Two conforming implementations need not exclude each other,
  so a root records, when it is created, which implementation coordinates it, and an open
  through a different one is refused. A store keeps the implementation it was created with.
  Because the contract orders the coordination state with the store's filesystem operations, an
  implementation is correct only for the `Vfs` and root the store uses: the store builds its
  coordination from its own `Vfs` and root, through a factory supplied at open, and never
  accepts one built elsewhere.
- **Creation is reserved before it begins, by whoever publishes the reservation first.** Two
  implementations' creation locks need not exclude each other, so they cannot arbitrate between
  themselves. A creator first acquires its own implementation's creation exclusion and holds it
  until the marker is in place; it then publishes the reservation -- a file naming the
  implementation and recording every creation input (the genesis configuration, the agent, the
  schemas) and the creation's nonce -- with an exclusive publish-if-absent that leaves either the
  complete file or none, and makes the file and its directory entry durable before creation
  makes any other artifact -- so a crash leaves either no reservation and nothing else, or a
  complete reservation. Whoever publishes it owns the creation; any other creator finds it and
  stops. Because the reservation is never visible incomplete, and its creator already holds its
  implementation's exclusion when it appears, no observer can mistake a live creation for an
  interrupted one.
- **An interrupted creation is resumed, not repeated.** An `init` that finds a reservation
  without a marker acquires the creation exclusion of the implementation the reservation names
  -- which succeeds only once the original creator is gone -- re-reads the reservation to confirm
  it is unchanged, and resumes only if its own creation inputs are identical to those recorded;
  any other `init`, and every open, is refused. Resuming completes, idempotently, exactly the
  artifacts creation makes, in creation's order; anything else found beneath the root refuses
  the resume, as an occupied root refuses a creation today.
- **Creation exclusion.** Within the implementation that owns the reservation, creating a store
  excludes every other creator and opener of that root until its marker is in place.
- **The root names the software that manages it.** The **agent** is the software package that
  manages the store. It is identified by an **agent identifier** in two parts -- a
  version-independent part and a version -- and described by an **agent description**, a
  human-readable text (for example where to find the software) so an administrator who comes
  across the root can locate the software that understands it. Both are given by the embedder
  when it creates or opens a store.
  - **The version-independent part must match.** An open by an agent whose version-independent
    part differs from the recorded one is refused: over its life, a store is opened only by
    code of the same agent.
  - **The agent's version is the agent's alone.** It carries no meaning for canopy, which never
    orders or compares it beyond equality. The root records the version of an agent whose write
    succeeded: after each successful write, under the same hold of exclusion, a handle whose
    version differs from the recorded one replaces it. The stamp is separate from the write and
    never changes the write's outcome: a failed write never stamps, and a stamp that fails after
    a successful write is reported alongside that success. The recorded version is therefore
    always one whose write succeeded, though it may lag the latest such version when a stamp
    failed. The recorded value is cached with the generation, so the comparison costs nothing
    when it matches. An agent that wants to refuse to run against a store a different version
    wrote reads the recorded version and decides for itself.
  - Agent records are informational, so they need no transactional protection: each change is a
    durable replacement made under exclusion, and a crash leaves either the old value or the
    new one. canopy stores and reports them but never interprets the description.
  - **Only the description can be changed through a management call.** Other store-level
    settings are expected over time and join the same management surface.
- **Store-level properties are read through one call**, which reports the agent identifier and
  description, the coordination implementation, the canopy-format and agent-schema versions,
  the oldest reader schema still admissible, the identity and creation time, the genesis format version, and the replica counts and shard
  width.

#### Schemas and attachment

- **Compatibility is carried by schema versions, not software versions.** Two schemas govern a
  store: canopy's own on-disk format and protocol, which canopy defines, and the agent's document
  schema, which the agent defines and canopy enforces without interpreting. Each is a simple,
  linearly increasing number recorded in the root, together with, for each layer, the oldest
  reader schema still admissible. Whether a handle may attach, and stay attached, is decided per
  layer by the admission rules below; agents of different software versions may therefore work
  on one store side by side, so long as they speak admissible schemas.
- **Attachment.** A handle opened only to read holds the reader tier shared for its life; a
  handle that may write holds the reader and the writer tiers shared. Holding attachment blocks
  no read and no write, only an acquirer that wants a tier exclusively. The holder's death
  releases it.
  - **Admission.** A handle passes an admission gate before it attaches. The gate has two
    modes: ordinary admission takes it shared, only until the handle holds its reader tier, and
    then releases it before waiting for anything else; an upgrade takes it exclusively. Holders
    of the shared mode never block one another, so no handle is ever blocked behind another
    ordinary handle's wait. A waiting handle can give up. No order among waiters is promised,
    so an upgrade on a busy store may wait for a lull in admissions.
  - **The admission check happens after attaching**, under the shared tiers, so an upgrade
    cannot complete between a handle's check and its attachment. For each schema layer, with
    `oldest` the oldest admissible reader schema and `current` the current schema, a handle may
    attach, and stays attached, if it speaks:

    | Store state | Reader | Writer |
    |---|---|---|
    | ordinary | a schema from `oldest` to `current` | `current` |
    | reader-safe upgrade in progress | a schema from `oldest` to `current`, or the target | the target, and only to resume the upgrade |
    | reader-unsafe upgrade in progress | -- (none attaches; the upgrade drained them) | the target, and only to resume the upgrade |

    Each upgrade's completion sets `oldest` for its layer: unchanged by a reader-safe upgrade,
    which every admissible reader can still read, and the target by a reader-unsafe one.

  - **Order.** Admission gate, then the reader tier, then the writer tier, then writer exclusion.
    Nothing is acquired against that order, and **a handle never waits for the admission gate
    while it holds a tier**: anything waiting for the gate holds nothing another waiter needs.
- **A schema upgrade is a one-way transition made by one writer that owns the store.**
  - **It owns its attachment outright first.** A handle's attachment is shared by all of its
    clones, and every operation on any clone holds a lease on it for the operation's duration.
    The upgrade puts the attachment into a draining state: new operations on every clone are
    refused, and the upgrade waits for the outstanding leases to end while still holding its
    tiers, so no operation in flight ever runs beneath a schema change. The call that starts
    the upgrade holds no lease of its own that it would wait for; an upgrade started from inside
    an operation -- from a callback, or through a clone used within one -- is refused, since it
    would wait for itself. Cancelling while draining ends the drain and leaves the handle
    attached and usable; only once the drain has finished is the attachment the upgrader's
    alone to give up.
  - **It converts its own attachment.** The upgrading handle first releases its shared tiers,
    then takes the admission gate, then acquires the writer tier exclusively -- and the reader
    tier too when the change is not safe for readers. OS locks do not convert atomically, so the
    handle is detached for that interval. It never waits for the gate while holding a tier, and
    never holds writer exclusion while waiting for attachment, so it can deadlock neither against
    another upgrader nor against an attached writer waiting for exclusion. The gate admits one
    upgrader at a time. Once it holds its tiers, it checks the schemas again -- another upgrade
    may have finished while it waited -- and gives up if they moved.
  - **During a reader-safe upgrade the gate reopens to readers.** Once the upgrader holds the
    writer tier exclusively it releases the gate, so readers attach through ordinary shared
    admission while new writers wait on the writer tier itself, holding nothing a reader needs.
  - **A conversion that fails or is cancelled after the drain returns promptly, with the
    handle detached.** Reattaching is a separate, ordinary admission that may wait or be refused
    -- the schemas may have moved meanwhile -- and every operation on a detached handle is
    refused until it reattaches or is dropped. Cancellation therefore never carries an unbounded
    obligation.
  - **Reader compatibility is cumulative and recorded.** A reader-safe upgrade is one whose
    result every admissible reader schema, from `oldest` up, can read; the intent records the
    admission rules in force during it, and its completion sets `oldest` as the admission rules
    say. A later upgrade counts as reader-safe only if it is safe for every schema from `oldest`
    up; if it is not, it is reader-unsafe and takes the reader tier exclusively, which drains
    every reader before it crosses that boundary. A reader of the target schemas reads data the
    migration has not reached yet through the target's definition of the transitional state,
    which the agent's upgrade must provide.
  - **It starts only from a recovered store**, and is itself crash-consistent: before its first
    effect it durably records an upgrade intent naming the schema layer and its source and
    target versions; it migrates in idempotent, resumable steps that report progress as
    recovery's do; it advances the recorded schema version durably only after every step has
    completed, and then clears the intent.
  - **An interrupted upgrade is resumed, never rolled back, and resuming it is the store's
    recovery.** The upgrade began from a recovered store and excluded every other writer, so the
    only unfinished work an interrupted upgrade can leave is its own. While an intent exists,
    recovery is required and consists of resuming the upgrade, performed by a handle that
    speaks the target schemas and therefore understands the transitional state; no handle that
    speaks only the source schemas may write.
  - **There is no downgrade.** canopy's own format upgrades follow the same protocol.
- **A registry lists the attached handles, for diagnosis.** Each attached handle registers an
  entry -- its agent identifier, the schemas it speaks, the tiers it holds and whether it is
  draining, enough to find its process, and a token unique to the attachment -- live for as long
  as the attachment lasts, so an administrator can see who is attached and who blocks an
  upgrade. A handle registers when it attaches and unregisters when it detaches -- including the
  detachment an upgrade's conversion makes -- and updates its entry when its tiers or draining
  state change. It is paid for only at those transitions, never by a read or a write, and it is
  not durable: it is discarded with the rest of the coordination state after a host restart.
  - **A dead handle's entry is never reported as live**, because that is the claim an
    administrator acts on. Every change to an entry, and every inspection of one, happens under
    that entry's claim lock, taken without waiting. A handle takes an entry by acquiring its claim
    lock -- moving on to the next entry if it is held -- and, holding it, trying the entry's
    liveness lock exclusively without waiting: success proves the entry unused. Still holding
    the claim lock, it writes its data and token, keeps the liveness lock exclusively, and only
    then releases the claim lock. A listing acquires an entry's claim lock without waiting --
    reporting the entry as unknown if it cannot -- and, holding it, reads the token and tries
    the liveness lock in shared mode, released at once; it reports the entry live only if that
    attempt was refused, and liveness is as of that probe. Because a reuser holds the claim lock
    from before it takes the liveness lock until after its token is written, no listing can see
    a new owner's lock beside an old owner's token. A handle that finds every entry held attaches
    unregistered, and listings then say they may be incomplete.
  - **Diagnosis needs no attachment.** Listing the registry, reading store-level properties and
    recovery discovery are also offered through a diagnostic view that takes no tier and grants
    no access to documents, so an administrator can see who blocks an upgrade even while the
    upgrade holds the admission gate.
  - Otherwise it is best-effort: a handle that cannot register still opens, and a listing that
    may be missing entries says so. It is also the membership any future negotiation between
    writers would start from.

#### Exclusion and recovery

- **Writer exclusion.** At most one holder across every thread of every process opening the
  root. Acquisition waits until access is granted. The contract does not require fairness; it
  does not forbid it either. An implementation may serve waiters fairly -- the in-process gate
  is first-come-first-served -- but nothing may depend on the order in which an implementation
  serves them. Exclusion is not re-entrant: canopy acquires it once, in the
  outermost write, and a nested call made under that hold runs under it. Callbacks that run
  under exclusion receive a read-only view, so they cannot re-enter a write.
- **Ownership.** Acquiring exclusion records the holder; a clean release clears the record. An
  acquirer that finds a record left behind is told that the previous holder died while holding
  exclusion. This is the implementation's own bookkeeping -- no OS lock reports an abandoned
  owner (`flock` and `LockFileEx` both simply release) -- and it is independent of the
  modification count, so a holder that dies before its first effect or after its last is still
  reported. Being told does not recover anything. Before it replaces the abandoned record, the
  acquirer sets the fence, so the evidence outlives its own clean release; it then reports that
  recovery is required. Only a successful recovery clears the fence.
- **Attaching changes no store data.** Opening takes attachment and registers in the registry --
  coordination bookkeeping -- but never touches store data: it runs no recovery, discharges no
  directory-sync debt, and stamps no agent version, because another process may be in the middle of a
  transaction, and its `begin` is not an abandoned one. Those run only under writer exclusion.
- **Recovery holds a capability ordinary writes do not.** The fence refuses ordinary writes; the
  holder that runs recovery replays through the internal write paths without that check, and
  clears the fence only after recovery succeeds. A fence that blocked its own recovery would
  wedge the store permanently once it lived outside the process.

#### Counters

- **Readers take nothing.** A read never acquires exclusion and never waits for a writer (`D8`):
  it only samples the modification count, without taking any lock. **A sample is one read of a
  value published atomically**: the file-based implementation writes the new value to a fresh
  file and renames it over the published one, so a reader sees either the previous value or the
  new one, complete, and a writer suspended or killed before its rename leaves the previous value
  published. A sample is never assembled from parts a writer publishes separately -- such as
  alternating slots -- because a reader can then hold one stale part while rejecting a torn
  newer one, and pass a snapshot it should have refused.
- **Publishing the count can fail, and each failure has one outcome.** If the opening
  publication fails, the operation makes no effect and fails. If the closing publication fails
  after a completed operation, the count stays odd and the fence is set, so recovery -- which
  republishes it -- is required; the operation's own result is returned together with that
  report.
- **Modification count.** Raised only by the exclusion holder; sampled by anyone. A raise is
  ordered with the holder's filesystem operations, and a sample with the sampler's: a sample
  taken after observing any filesystem effect of a modification reflects that modification's
  opening raise. A sample is never stale -- an implementation that caches must invalidate
  synchronously, which is why an unacknowledged lease break does not qualify.
  - **A modification is a whole logical operation** -- every swap of a transaction, a write from
    ingest through collection, a repair -- not each filesystem step: the count goes odd before
    its first effect and even after its last. Operations nested under one hold change nothing;
    the depth is tracked, so an inner operation never restores even parity early. This is what
    lets a reader, and an optimistic snapshot, tell a transaction in progress from a settled one.
  - **An operation that ends unresolved leaves the count odd.** A transaction that fails after
    its first effect and cannot be rolled back sets the fence; the count then stays odd until
    recovery completes, whether or not its holder lives, so no snapshot accepts its partial
    result.
  - **A dead holder's odd count stays odd until recovery has finished**, so a reader that meets
    the partial state it left classifies it as contention rather than damage; recovery does not
    lower it early, and nothing resets it while any process is attached.
- **Fence and generation.** The fence is set by a holder, seen by every process, and cleared
  only by recovery. The generation advances on every acquisition of exclusion, by any handle in
  any process, and changes only under exclusion. Acquiring exclusion returns the generation it
  found before advancing it, and each handle remembers the value it advanced to.
- **Caches are tied to the generation.** Each handle's knowledge of durable state -- what is
  known durable about the commit log, the directory-sync debt record's contents and torn state,
  and the recorded agent version -- is valid only while no other handle has held exclusion. A
  handle whose acquisition finds the generation it last advanced to knows nobody intervened and
  keeps them; any other value means another holder came between, and it reloads them, so
  it never appends onto another holder's torn record or truncates a record it has not read. Two
  handles opened independently in one process are as separate as two processes.
- **Survival.** All of it survives the death of any process. It is never trusted across a host
  restart, or whenever an implementation cannot show it was continuously maintained: recovery is
  then required. A shared mapping can vanish when its last user exits without any restart, and
  a count raised without an fsync can roll back over a writer that was mid-modification, so each
  implementation states how it recognises a restart (a boot identity the OS provides) and how it
  creates or re-creates its state, treating anything it cannot vouch for as requiring recovery.
  **State that cannot be vouched for reads as odd** to every sampler from the moment it is
  created or found, so no snapshot accepts data a crash may have left half applied; only a
  completed recovery publishes a trusted even value.
  What recovery resolves -- an in-flight transaction, a partial publication -- it reads from
  durable state (the commit log, and whatever the `STRANDED-VOTE` guard records), never from the
  coordination state.

The TLA+ model's processes interact only through this contract, so the model is checked once,
and every implementation is held to the contract by one conformance suite that all of them must
pass. Cost is not part of the contract.

### Consequences

- **It is modelled before anything is built on it.** A single-process assumption is cheap to
  remove while little depends on it, so the TLA+ model gains processes -- including the
  transaction, fence and log-reset behaviour they interact through -- before the
  `STRANDED-VOTE` guard is implemented, and the guard chosen must hold with several processes.
  A process's death and a host's crash are distinct events in the model: a dead process loses
  only its in-process state, while the live filesystem it left survives. **The model checks
  progress as well as safety**, under assumptions it states: every handle eventually detaches,
  every I/O eventually succeeds, and each synchronization resource -- writer exclusion, each
  mode of the admission gate, each attachment tier -- is weakly fair as a whole, so that when it
  stays available *some* waiter for it is served. Under them, an upgrade that holds its tiers,
  is not cancelled and whose process survives completes, a recovery that is driven completes,
  and the store as a whole makes progress. Progress is stated about individual requests: a
  request may be served, or withdrawn by giving up, by its process's death, or by becoming
  ineligible, and only a request that persists and stays eligible counts as starved. **The contract promises no fairness**: no individual waiter
  -- writer, upgrader or attaching handle -- is promised to be served, because requiring it
  would constrain every implementation for a property canopy's correctness does not need. The
  model shows the schedules in which a waiter is passed over indefinitely, and that limit is
  stated rather than hidden. Whether any fairness returns to the contract is decided from those
  schedules and from a quantitative model of contention[^odi-2], not assumed in advance.
  Deadlock checking stays on, with negative controls -- such as an upgrader waiting for the gate
  while holding a tier -- that must deadlock.
- **Fairness is a tool of last resort, used in limited amounts.** It is expected that reasonable
  behaviour under contended writes will need some fairness, but fairness carries its own
  hazards -- lock convoys when the lock is handed to a waiter that is not running, the cost of
  handing exclusion across processes, priority inversion, and a contract every implementation
  must honour. So each point of conflict is first examined for a way to avoid the conflict
  (shorter holds, finer-grained exclusion, keeping readers out of the wait altogether); only
  where that fails is fairness considered, and then the weakest form that addresses the measured
  problem -- such as a bound on how many times one waiter may be passed over -- rather than
  first-come-first-served handoff.
- **What the evidence must show.** The model checker can show that a waiter can be starved and
  by which schedule, but not how likely that is. Rates come from a quantitative model -- queueing
  analysis for the baseline, and a seeded discrete-event simulation for the store's real
  structure (waiters unordered among threads and processes alike, as the contract allows, with
  first-come-first-served within a process compared as an implementation option; upgrades
  waiting behind a stream of admissions) -- which predicts the tail of the wait, and the bypass
  counts, as functions of the workload, under unordered service, first-come-first-served service
  and bounded bypass. For a stable, work-conserving, non-preemptive single server with
  independent arrivals and service times independent of the order of service and of finite
  variance (M/G/1), serving waiters in random order leaves the mean wait unchanged and raises
  its variance and its upper percentiles; that result is the baseline only, since the gate's
  shared mode, upgrades and handoff costs fall outside its assumptions. The prediction to look
  for is the contention at which the unordered tail breaches a stated budget on the wait --
  such as the probability of waiting longer than a deadline, or a high percentile -- that FIFO
  would meet. Benchmarks of the implementation then test the prediction against the same
  budget.
- **Tests simulate a process's death by abandonment, never by dropping its handles.** A clean
  drop releases ownership through the normal path and returns the modification count to even,
  erasing exactly what such a test must observe. Abandonment releases a dead process's OS-level
  holds without any clean-release bookkeeping, and is cross-checked against real process kills.
- **Recovery is explicit: discovered first, then run incrementally.** Recovering a badly
  damaged store can take a long time, so it is never charged silently to an open or to
  whichever write happens to arrive. Opening never recovers, and a store that needs recovery
  refuses writes with `StoreError::RecoveryRequired` until it has been recovered. Recovery is
  required after a holder died while holding exclusion, after a host restart, and while the
  fence is set. Unfinished durable work -- in the commit log, the directory-sync debt, the
  `STRANDED-VOTE` guard's record -- counts only once it is **abandoned**: a live holder in the
  middle of a transaction has unfinished work too, and that is not a reason to recover. Work is
  abandoned when the ownership record shows its holder died, or after a host restart.
  - **While recovery is required:**

    | Verbs | Allowed |
    |---|---|
    | reads, `properties`, `attached_handles`, recovery discovery | yes |
    | the recovery run -- which, while an upgrade intent exists, resumes the upgrade | yes, under its own capability |
    | `set_agent_description` | yes -- informational, under exclusion |
    | logged and unlogged writes, `transform`, `plan_and_apply` | no |
    | `scrub_content`, `repair_reported_damage`, `repair_blob`, `repair_pointer` | no -- they write, and would act on state recovery has not settled |
    | `quiesce` and the automatic commit-log reset | no -- a reset never discards unresolved work |
    | `upgrade_schema` | no -- an upgrade starts only from a recovered store |
  - **Discovery** is a public call any attached handle can make: it reports whether recovery is
    required and why, mutates nothing, and costs little. It reads the ownership record, the
    fence and the restart state together, so it reports work a live holder owns as not
    requiring recovery; a later discovery reports otherwise if that holder dies.
  - **The run is incremental.** A recovery acquires exclusion and holds it for its life; each
    step does a bounded unit of work and reports progress -- the phase, and units done of units
    known, where the known total can grow as recovery learns more and the report says when it
    has -- so the caller can gauge it, pace it, and stop between steps.
  - **It is resumable.** Every step is idempotent and leaves durable state from which recovery
    continues, so a recovery that is stopped, or whose process dies, is resumed by the next one
    rather than restarted. The fence stays set and the modification count odd until the final
    step completes, so writers stay refused and readers see contention throughout.
  - **A convenience runs every step to completion**, for an embedder that has no need to pace
    it.
- **Placement of the state that is per-process today:**

  | State | Where it lives |
  |---|---|
  | Write serialisation | the coordination state's writer exclusion, replacing both the in-process writer gate and the root lock; no order among waiters is promised |
  | Modification count (`D335`) | the coordination state, so a reader in any process classifies contention against every writer |
  | Recovery fence | the coordination state; it survives process death, and a host restart forces recovery regardless |
  | What is known durable about the commit log | per handle, reloaded when the generation has moved since the handle last held exclusion |
  | Directory-sync debt | the existing root file, read and written only under exclusion, its in-memory state reloaded on the same rule |
  | Repair backlog, read rotation, commit feed | per process; the feed reports this process's writes |

- **Cost.** An uncontended OS lock is one system call, small beside the fsyncs every write
  already performs. A file-based count costs a reader a few system calls per sample, modest
  beside the replica reads a document read already makes; a mapping costs what the in-process
  atomics do.
- **Constraint on work until it lands:** nothing new may depend on there being no other writing
  process. The `Store` write surface treats its nonce source and commit feed as per-process,
  and a guard against `STRANDED-VOTE` qualifies only if its correctness rests on durable state.
- **Staged:** readers in other processes first, then writers taking turns. Work:
  [CHECKLIST.md](CHECKLIST.md). Discussion:
  [DESIGN-SESSION-2026-11-10-multi-process.md](design-sessions/DESIGN-SESSION-2026-11-10-multi-process.md).

[^odi-2]: `ODI-2` in [CHECKLIST.md](CHECKLIST.md)

## The content-store scale tests are unit tests with a 2 s budget, 2026-11-10

- **The scale tests stay unit tests, at full scale, in
  [scale.rs](src/tests/blob/scale.rs).** They drive `ingest_blob` and `retain_blobs`, which are
  crate-private, so only unit tests can reach them without widening the API.
- **Their module's budget is 2 s, not the repository's 1 s.** The cost is linear in the
  document count and spread across the whole ingest path with no hotspot left to remove, so
  meeting 1 s would mean shrinking the data the tests exist to exercise. The engineer accepted
  the overrun for this module alone; the budget is measured the way the 1 s rule is, one test
  at a time in a debug build.
- **The cost rests on two properties that must be kept:** a `MemFs` directory sync costs what
  changed in the directory rather than its size, and each test opens a `MemFs::fork` of a store
  populated once per replica count instead of populating its own. Losing either puts the
  module well past 2 s.
