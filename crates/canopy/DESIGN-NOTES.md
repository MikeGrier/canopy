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

> **SUPERSEDED in part, 2026-11-11, by "The publication record's live and durable state,
> modelled in `PointerPublication.tla`" below.** The record guard's "holds" is no longer
> provisional: it was checked with the record's live and durable state separate, beside logged
> writes, and with checks, collection, repair, scrub and a lost pointer replica, and it holds.
> It holds only with a settle that fsyncs the record in every operation, not where one is found,
> which adds one fsync per operation to the cost stated here. The resolve guard holds under the
> same refinements, and the two differ in one respect that is not a safety failure: the resolve
> guard restores a lost pointer replica at the next write and the record guard leaves it to a
> repair. Read that section before acting on this one.

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

[^odi-2]: `ODI-2` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#odi-2) [completed 2026-11-12]

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

## Writer exclusion and its ownership record, modelled in `Coordination.tla`, 2026-11-10

- **The coordination contract is modelled in its own module, [Coordination.tla](spec/Coordination.tla),
  and [PointerPublication.tla](spec/PointerPublication.tla) stays the model of one document's
  publication.** The two interact where a holder's death or a host restart leaves unfinished
  publication behind, so they are composed for that check and kept apart otherwise: a
  coordination concern gets its own configurations without multiplying the publication model's
  state space. Each part of the contract is added to `Coordination.tla` with its own invariants and
  a control that must fail.
- **A clean release clears the ownership record while it still holds the lock, and only then
  drops the lock.** The contract's rule -- an acquirer that finds a record under the lock is told
  its holder died holding exclusion -- is true only in that order. Dropping the lock first lets
  another acquirer find the record of a holder that is alive and releasing, report a death that
  did not happen, and set the fence on a healthy store (T2 below). Implementations and their
  conformance suite carry the order as a requirement[^mp-5][^mp-8].
- **An acquirer decides that a holder died from the record alone.** An OS lock reports no
  owner, and nothing in the contract lets an acquirer ask whether a process is alive, so the
  model gives it no such question; the checker compares its decision with what really happened.
- **Acquiring is three steps -- take the lock, set the fence if a record was left behind, then
  replace the record -- and a process may die between any two.** The fence comes before the
  replacement so that no state exists in which a holder's death is neither recorded nor fenced:
  the evidence is the record naming a dead holder until the fence carries it, and the fence
  outlives the acquirer's own clean release (T1 below). A death between the steps leaves the
  previous record for the next acquirer.
- **A host restart needs no abandoned holder to require recovery.** After one, the coordination
  state is untrusted: the record and the fence may be gone, so an acquirer is told recovery is
  required even when it finds neither. This is distinct from a process's death, which loses only
  that process and leaves the live filesystem and the record as they were.
- **An acquirer told recovery is required changes no store data and writes nothing.** It may
  only release exclusion; running recovery is a separate capability.
- **A wait is a request that ends in exactly one of four outcomes: served, withdrawn, dead with
  its process, or refused as ineligible.** The state holds no queue and no ticket. Whenever the
  lock is free, acquisition is enabled for every eligible waiter and the checker chooses among
  them, so each invariant is checked against every service order; the contract's silence about
  fairness is the model's.
- **Each rule has a control that must fail and each interesting state a witness that must be
  reached.** The controls reverse a rule: replacing an abandoned record without setting the
  fence, dropping the lock before clearing the record, and ignoring a host restart. The witnesses
  are runs that must violate an invariant stating that a holder died before its first effect, in
  the middle of a write, or after its last effect and the next acquirer is told; that a restart
  alone requires recovery; and that refused, withdrawn and dead requests all occur. A property
  that holds only because its states are unreachable would pass without them.

### T1 -- a holder dies mid-write; the evidence outlives two clean releases

Status: `[verified]` against the model, which reaches this state. A, B and C have attached.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `REQ1` | A | requests writer exclusion | -- |
| 2 `LCK1` | A | is granted the lock | lock: A |
| 3 `REC1` | A | finds no record and no fence, and records itself as holder | record: A |
| 4 `EFF1` | A | applies the first effect of its write | store data: half applied |
| 5 `KILL` | EXTERNAL | A's process dies | lock: none; record still names A; store data unchanged |
| 6 `REQ2` | B | requests writer exclusion | -- |
| 7 `LCK2` | B | is granted the lock | lock: B |
| 8 `FNC` | B | finds a record left behind and sets the fence | fence: set |
| 9 `REC2` | B | replaces the record and is told recovery is required; it writes nothing | record: B |
| 10 `CLR` | B | clears the record while holding the lock | record: none |
| 11 `ULK` | B | drops the lock | lock: none |
| 12 `REQ3` | C | requests writer exclusion | -- |
| 13 `LCK3` | C | is granted the lock | lock: C |
| 14 `REC3` | C | finds no record, finds the fence set, and records itself; it is told recovery is required and writes nothing | record: C |

### T2 -- releasing the lock before clearing the record reports a death that did not happen

Status: `[verified]` against the model, in the control that reverses the order. A and B have
attached.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `REQ1` | A | requests writer exclusion | -- |
| 2 `LCK1` | A | is granted the lock | lock: A |
| 3 `REC1` | A | finds no record and no fence, and records itself as holder | record: A |
| 4 `ULK` | A | drops the lock first, still holding the record | lock: none; record: A |
| 5 `REQ2` | B | requests writer exclusion | -- |
| 6 `LCK2` | B | is granted the lock | lock: B |
| 7 `FNC` | B | finds a record under the lock, concludes its holder died, and sets the fence; A is alive | fence: set |
| 8 `REC2` | B | replaces the record and is told recovery is required | record: B |

[^mp-5]: `MP-5` in [CHECKLIST.md](CHECKLIST.md)
[^mp-8]: `MP-8` in [CHECKLIST.md](CHECKLIST.md)

## The modification count, the generation and reads, modelled in `Coordination.tla`, 2026-11-10

- **A read is modelled as the store classifies it today**: a sample of the count, an
  observation of the data, a second sample, and `Store::undisturbed` -- the two samples equal and
  even -- as the verdict that the read saw a settled store. The data is two cells a transaction's
  two swaps change in turn, observed in two separate steps, so a read can see one swap and not
  the other. The property checked is that a read classified as settled saw both cells equal and
  as they still are. That one property carries both of the contract's uses of the count: a
  snapshot that accepts only an undisturbed run, and a verdict that a disagreement is real rather
  than raced.
- **A publication is two steps -- prepare, then publish -- and only the second is visible.** A
  holder that dies or fails between them leaves the previous value published, which is what the
  contract's atomic publication (a fresh file renamed over the published one) provides. The
  opening raise is published before the first effect and the closing raise after the last, so a
  death at any point leaves the count odd if any effect landed.
- **Every way an operation ends is a separate step with its own rule.** It completes; it ends
  without an effect; it fails after its first effect and is rolled back, then closes normally; it
  fails unresolved, setting the fence and leaving the count odd; its opening publication fails,
  with no effect and the count unchanged; or its closing publication fails, setting the fence and
  leaving the count odd. A nested operation under the same hold publishes nothing.
- **A host crash may roll the count back over a writer that was mid-operation**, as the
  contract says a count raised without an fsync can. That is why state that cannot be vouched for
  must read as odd rather than as stored: with the ownership record and the fence also lost in the
  crash, an even rolled-back count is the only thing a reader would have, and it would accept
  half-applied data (T3 below).
- **A cached sample is invalidated synchronously with the publication that outdates it.** The
  model lets a reader serve its next sample from the value it last read; a cache invalidated any
  later lets a read compare two samples that bracket nothing.
- **The generation is checked by the use it guards.** A holder caches state another holder can
  change, relies on it without reading again, and keeps it only when the generation it finds is
  the one it last advanced to. A holder that kept it on any other value would act on state it
  never read.
- **The contract's ordering rules are an assumption of the model, not a result.** Its steps are
  sequentially consistent, so a raise is ordered with the holder's effects and a sample with the
  sampler's observations by construction. Those orderings are requirements on each
  implementation, which the model cannot check[^mp-5].
- **Controls and witnesses.** Five controls reverse a rule and must fail: a sample assembled from
  two separately read parts, a cached sample invalidated late, state that cannot be vouched for
  read as stored, a cache kept across a generation the handle did not advance to, and a nested
  operation that raises the count when it ends. Witnesses show each way an operation ends, each
  death point during a publication, a settled read accepted, a torn read and a read over a failed
  transaction rejected, a read meeting untrusted state, a read spanning a whole completed
  operation, and a cache both kept and reloaded. The configurations are named
  `Coordination-Control-*.cfg` and `Coordination-Witness-*.cfg`, and each states in its header
  what TLC must report.

### T3 -- a crash rolls the count back over a half-applied write

Status: `[verified]` against the model, in the control that reads untrusted state as stored. A
and B have attached; the read is B's.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `REQ1` | A | requests writer exclusion | -- |
| 2 `LCK1` | A | is granted the lock | lock: A |
| 3 `REC1` | A | finds no record and no fence, records itself and advances the generation | record: A |
| 4 `OPN` | A | prepares the opening raise | -- |
| 5 `PUB` | A | publishes it | count: 1 |
| 6 `EFF1` | A | applies the first swap | data: half applied |
| 7 `CRSH` | EXTERNAL | the host loses power; A dies | count rolled back to 0; record lost; fence never set; coordination state untrusted |
| 8 `SMP1` | B | samples the count and gets 0 | -- |
| 9 `OBSA` | B | observes the first swap's cell, applied | -- |
| 10 `OBSB` | B | observes the second swap's cell, not applied | -- |
| 11 `SMP2` | B | samples the count again, gets 0, and classifies the read as settled | -- |

Under the contract, both samples in steps 8 and 11 read as 1, because the state cannot be vouched
for, and the read is contended instead.

## The admission gate and the attachment tiers, modelled in `Coordination.tla`, 2026-11-10

- **Where a process is says what it holds.** The model gives each process one attachment state
  and derives from it which of the gate and the tiers it holds, shared or exclusive, so there is
  no lock state to fall out of step with the processes. A process's death moves it to detached,
  which releases all of it at once, as the OS does. Death can therefore happen holding or
  waiting for any of them.
- **An ordinary handle takes the gate shared, the reader tier, drops the gate, takes the writer
  tier if it may write, and only then is checked**, under the shared tiers, so an upgrade cannot
  complete between its check and its attachment. A handle that is not admitted releases
  everything and is gone; a handle that is waiting may give up. A control that checks before it
  waits for the writer tier admits a handle against a store an upgrade then moved.
- **The per-layer admission rules are a table the model states once** (reader and writer, by
  ordinary, reader-safe upgrade in progress and reader-unsafe upgrade in progress), over the
  oldest admissible reader schema, the current schema and the upgrade's target. What is checked
  is that the protocol keeps the rules true as the store moves -- every admitted handle was
  admissible when admitted, and none stays attached once it is not -- and that the cases the
  table distinguishes all occur: a reader speaking the target admitted during a reader-safe
  upgrade, a reader refused after an interrupted unsafe one, a writer speaking the target
  admitted only to resume an interrupted upgrade.
- **An upgrader takes the gate exclusively, then the reader tier if the upgrade is unsafe for
  readers, then the writer tier -- the contract's order, gate then reader tier then writer tier
  then writer exclusion.** The upgrade's description in the contract names the writer tier first
  and the reader tier "too"; that is a list of what is taken and not an order to take it in. An
  upgrade that follows the sentence takes the writer tier first and deadlocks against a handle
  that holds the reader tier shared and waits for the writer tier (T4 below). The order is a
  requirement on every implementation and on the interface that exposes the tiers[^mp-5].
- **An upgrader releases its shared tiers before it waits for the gate.** An upgrader that kept
  them deadlocks against a second upgrader (T5 below); one that began while holding writer
  exclusion deadlocks against an attached writer that waits for it (T6 below).
  The contract already says neither; the controls confirm that each rule is the thing that
  prevents the deadlock and not an incidental safeguard.
- **Holding the gate while waiting for the writer tier is a violation whether or not it
  deadlocks**, since the gate is held only until the reader tier is held, so that no handle waits
  behind another's wait for the gate.
- **The order is checked as a rank.** Gate, reader tier, writer tier and writer exclusion are
  ranked in that order and no process waits for a resource ranked at or below one it holds. That
  one property fails in all three deadlock controls, and is how the contract's "nothing is
  acquired against that order" is stated checkably.
- **The model checks for a deadlock directly**, with no process allowed to give up or die, so
  the only state with no successor in a deadlock-free model is every process ended. The
  contract lets a waiting handle give up, which turns a deadlock into a wait that ends only
  when someone gives up; the model keeps that behaviour behind a constant so the deadlock is
  visible where the contract would hide it. A deadlock-free configuration per mix of upgraders,
  writers and readers states the contract's claim for that mix.
- **A schema upgrade is modelled here only as far as the locks need.** It completes in one
  step, which moves the layer to its target and the oldest reader schema with it if the upgrade
  is unsafe for readers; an upgrader that dies in its body leaves the upgrade recorded as in
  progress. The upgrade's body, its durable intent, resumable steps and cumulative reader
  compatibility are modelled by the upgrade's own item[^mp-2], and the handle's clones and the drain
  by theirs[^mp-1-4]. Nothing here is a claim about those.
- **Some checks follow from the locks and are not protocol.** An ordinary handle's check for an
  exclusive reader tier, and an upgrader's check for shared writer tier holders before it takes
  the writer tier exclusively, can be removed from the model without any property failing,
  because the gate and the order already exclude the situations they guard. They are the lock's
  own mode compatibility, so an implementation gets them from the OS lock and not from extra
  code.
- **Attachment multiplies the three-process state space, so three processes are checked in two
  configurations.** Splitting attaching into the contract's steps lets several processes be
  part-way through it at once. The three-process configuration with every dimension on
  was therefore split into one that keeps three-way contention and a death and one that keeps a
  host crash and a read; the two-process configuration keeps every dimension at once.
- **A configuration cannot state a sequence, so the processes' roles and schemas are named
  scenarios** in the specification's table, which the configuration selects by name.
- **Controls and witnesses.** The controls reverse a rule: an upgrader that keeps its tiers while
  it waits for the gate (deadlock), one that begins holding writer exclusion (deadlock), an
  unsafe upgrade that takes the writer tier first (deadlock), a handle that holds the gate
  through the writer tier, an unsafe upgrade that leaves readers attached, and a check made
  before the writer tier is waited for. The witnesses reach each case of the admission table,
  each way an upgrade ends, and each wait the protocol creates. The configurations follow the
  existing naming and each states in its header what TLC must report; a header that says
  `TLC must report a deadlock` names a control for a wait that never ends.

### T4 -- an unsafe upgrade that takes the writer tier first deadlocks against a writer

Status: `[verified]` against the model, in the control that takes the writer tier first. U is an
upgrader whose upgrade is unsafe for readers and W a handle that may write.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `BGN1` | W | begins to attach and asks for the gate shared | -- |
| 2 `GTS1` | W | holds the gate shared | gate: shared by W |
| 3 `BGN2` | U | begins to attach | -- |
| 4 `GTS2` | U | holds the gate shared | gate: shared by W and U |
| 5 `RDS1` | U | takes the reader tier shared | reader tier: shared by U |
| 6 `DRP1` | U | drops the gate | gate: shared by W |
| 7 `WRS1` | U | takes the writer tier shared | writer tier: shared by U |
| 8 `ADM1` | U | passes the check and is attached | -- |
| 9 `SUP` | U | starts its upgrade: releases its tiers and asks for the gate exclusively, which W holds shared | reader tier: none; writer tier: none |
| 10 `RDS2` | W | takes the reader tier shared | reader tier: shared by W |
| 11 `DRP2` | W | drops the gate and asks for the writer tier shared | gate: none |
| 12 `GTX` | U | takes the gate exclusively | gate: exclusive by U |
| 13 `WRX` | U | takes the writer tier exclusively, before the reader tier | writer tier: exclusive by U |
| 14 `DLK` | W, U | W waits for the writer tier, held exclusively by U; U waits for the reader tier exclusively, held shared by W; neither can proceed | -- |

Under the contract's order step 13 takes the reader tier exclusively, which waits for W; W takes
the writer tier, is attached, and U waits for it to detach, which is a wait and not a deadlock.

### T5 -- an upgrader that keeps its tiers while it waits for the gate deadlocks against another upgrader

Status: `[verified]` against the model, in the control that keeps the tiers. U1 and U2 are
upgraders whose upgrades are safe for readers.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `BGN1` | U1 | begins to attach | -- |
| 2 `GTS1` | U1 | holds the gate shared | gate: shared by U1 |
| 3 `RDS1` | U1 | takes the reader tier shared | reader tier: shared by U1 |
| 4 `BGN2` | U2 | begins to attach | -- |
| 5 `GTS2` | U2 | holds the gate shared | gate: shared by U1 and U2 |
| 6 `RDS2` | U2 | takes the reader tier shared | reader tier: shared by U1 and U2 |
| 7 `DRP1` | U1 | drops the gate | gate: shared by U2 |
| 8 `WRS1` | U1 | takes the writer tier shared | writer tier: shared by U1 |
| 9 `ADM1` | U1 | passes the check and is attached | -- |
| 10 `SUP1` | U1 | starts its upgrade keeping its tiers, and asks for the gate exclusively, which U2 holds shared | -- |
| 11 `DRP2` | U2 | drops the gate | gate: none |
| 12 `WRS2` | U2 | takes the writer tier shared | writer tier: shared by U1 and U2 |
| 13 `GTX1` | U1 | takes the gate exclusively and, holding it, gives up its tiers | gate: exclusive by U1; reader tier: shared by U2; writer tier: shared by U2 |
| 14 `ADM2` | U2 | passes the check and is attached | -- |
| 15 `SUP2` | U2 | starts its upgrade keeping its tiers, and asks for the gate exclusively, which U1 holds | -- |
| 16 `DLK` | U1, U2 | U1 waits for the writer tier exclusively, held shared by U2; U2 waits for the gate, held by U1 | -- |

### T6 -- an upgrader that begins holding writer exclusion deadlocks against an attached writer

Status: `[verified]` against the model, in the control that starts the upgrade while holding
exclusion. U is an upgrader whose upgrade is safe for readers and W a handle that may write.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `BGN1` | U | begins to attach | -- |
| 2 `GTS1` | U | holds the gate shared | gate: shared by U |
| 3 `RDS1` | U | takes the reader tier shared | reader tier: shared by U |
| 4 `BGN2` | W | begins to attach | -- |
| 5 `DRP1` | U | drops the gate | gate: none |
| 6 `GTS2` | W | holds the gate shared | gate: shared by W |
| 7 `WRS1` | U | takes the writer tier shared | writer tier: shared by U |
| 8 `ADM1` | U | passes the check and is attached | -- |
| 9 `REQ1` | U | requests writer exclusion | -- |
| 10 `RDS2` | W | takes the reader tier shared | reader tier: shared by U and W |
| 11 `LCK1` | U | is granted the lock | lock: U |
| 12 `REC1` | U | finds no record and no fence, and records itself as holder | record: U |
| 13 `SUPX` | U | starts its upgrade while holding exclusion: releases its tiers and asks for the gate exclusively, which W holds shared | reader tier: shared by W; writer tier: none |
| 14 `DRP2` | W | drops the gate | gate: none |
| 15 `WRS2` | W | takes the writer tier shared | writer tier: shared by W |
| 16 `GTX` | U | takes the gate exclusively | gate: exclusive by U |
| 17 `ADM2` | W | passes the check and is attached | -- |
| 18 `REQ2` | W | requests writer exclusion, which U holds | -- |
| 19 `DLK` | U, W | U waits for the writer tier exclusively, held shared by W's attachment; W waits for exclusion, held by U | -- |

[^mp-2]: `MP-2` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-2) [completed 2026-11-11]
[^mp-1-4]: `MP-1.4` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-1-4) [completed 2026-11-11]

## Clones, leases and the upgrade drain, modelled in `Coordination.tla`, 2026-11-11

- **A handle's operations are its leases, and the drain depends on them only by their ending.**
  The model counts the operations in flight on each handle. A clone's operation holds a lease
  and does nothing else; the process's own read, and its own write from request to release, are
  operations on its handle too. The attachment is the handle's, shared by all its clones, so a
  clone's lease is a lease on the same tiers and a process that ends drops every one at once.
- **An upgrade begins by draining, and drains while still attached.** The upgrader refuses new
  operations on every clone, waits for the leases to end holding its shared tiers, and only then
  releases them and asks for the gate exclusively. Holding the tiers through the drain is what
  keeps every operation in flight beneath them, so no upgrade completes under one; releasing
  them before the gate is what keeps it from waiting for the gate while holding a tier, which a
  drain that kept them past its end would break (T5 above, which the model now reaches through
  the drain).
- **A call made from inside an operation is refused at the door.** The drain would wait for an
  operation that cannot end while its own call is blocked, so the call returns at once as
  refused and the handle stays attached; a refused call is an outcome and not a wait. What the
  implementation must tell apart is the *caller's own* operation context, not whether the handle
  has any lease: leases of other clones are waited for, and only a lease the caller holds makes
  the call refusable. An upgrade started from inside a read's callback deadlocks on that read
  (T7 below), and one begun while holding writer exclusion is the same fault with a different
  victim[^mp-7].
- **Cancelling while draining ends the drain and leaves the handle attached and usable**, with
  operations in flight carrying on, because the tiers never left. **A conversion cancelled or
  failing after the drain leaves the handle detached, promptly and in one step**: the tiers are
  already released and nothing remains to wait for, so there is no unbounded obligation to
  restore the attachment. Cancellation and failure -- including the recheck finding that the
  layer moved -- have the same successor, and differ only in the outcome they report.
- **A detached handle holds nothing and refuses every operation until it attaches again or is
  dropped.** Attaching again is an ordinary admission, checked against the store as it then is,
  so it may be refused; dropping needs every clone's operation to have ended, since the
  attachment lasts until the last clone is gone.
- **The rule that makes the drain safe is stated on the state, not on the steps.** An operation
  is in flight only while its handle holds the tiers it was admitted under, and no conversion
  begins with an operation in flight. Both are written over the raw counts, so a fault in the
  model's own notion of a lease cannot hide them.
- **A clone's operation waits for nothing in this model.** An operation that waited, for
  exclusion say, would add a wait the drain depends on. The drain cannot form a cycle through
  such a wait because no upgrader holds exclusion while it drains or converts, which is the
  rule the exclusion-holding control of the previous section reverses; the model does not
  check a clone's wait on its own.
- **The cancelled drain, the failed conversion and the call refused at the door are separate
  outcomes of a request** (`withdrawn`, `refused`, `refused`), and the first two differ in
  where they leave the handle. A conversion in the upgrade's body cannot be cancelled here: the
  body is the migration's, and cancelling or failing within it is modelled with it[^mp-2].
- **Controls and witnesses.** The controls reverse a rule: an upgrade started from inside an
  operation (deadlock), a conversion that begins without waiting for the leases, a clone that
  starts an operation on a draining handle, a cancelled drain that detaches the handle, and a
  conversion cancelled after the drain that leaves the handle usable. The witnesses show an
  upgrade waiting for a clone's operation, operations ending while the handle drains, a drain
  that ends and goes on, one cancelled with operations still running, a conversion cancelled and
  one that fails at the recheck, a detached handle attaching again and being dropped, a call
  refused from inside a read and from inside a write, an upgrader dying while it drains, and a
  death ending the leases. Operations of clones are bounded by a constant, and a configuration
  that needs none leaves it at zero.
- **Earlier accounts omit the drain.** T4 to T6 predate it: the model now passes through a drain
  step, `DRN`, between an upgrade starting and its asking for the gate, which ends at once when
  no operation is in flight, and those accounts stand with that step added.

### T7 -- an upgrade started from inside a read waits for the read it is blocking

Status: `[verified]` against the model, in the control that lets an upgrade start from inside an
operation. U is an upgrader whose upgrade is safe for readers. The scenario's other process
attaches and exits and takes no part, so its steps are omitted.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `BGN1` | U | begins to attach | -- |
| 2 `GTS1` | U | holds the gate shared | gate: shared by U |
| 3 `RDS1` | U | takes the reader tier shared | reader tier: shared by U |
| 4 `DRP1` | U | drops the gate | gate: none |
| 5 `WRS1` | U | takes the writer tier shared | writer tier: shared by U |
| 6 `ADM1` | U | passes the check and is attached | -- |
| 7 `RDB` | U | begins a read and takes its first sample | -- |
| 8 `SUPI` | U | starts an upgrade from inside the read, as a callback would: refuses new operations and begins to drain, holding its tiers | -- |
| 9 `DLK` | U | waits for the read's lease to end; the read cannot go on while U's call is blocked, so the drain never ends | -- |

Under the contract step 8 is refused: U stays attached, the read goes on and ends, and U may
then start the upgrade from outside any operation.

[^mp-7]: `MP-7` in [CHECKLIST.md](CHECKLIST.md)

## The registry of attached handles, modelled in `Coordination.tla`, 2026-11-11

- **An entry exists exactly while its handle is attached, and that is stated on the locks.** An
  entry's liveness lock is held only by a handle that holds an attachment, so an entry exists
  neither before its handle is admitted nor once the attachment is given up -- by a clean drop,
  or by the conversion that detaches an upgrader -- and a handle that is attached has an entry
  it holds or the registry says some handle is missing. A handle leaves the registry before it
  lets go of its tiers and registers after its check and before its attach returns, so the
  window in which an attached handle has no entry is inside the call and never visible to the
  caller. An upgrade that kept its entry through the conversion would list a handle that holds
  nothing.
- **A listing's verdict depends on one rule: the token and the liveness probe are read under the
  claim lock.** A listing reports an entry live only if the liveness lock was held when it
  probed, by the handle the token names. A listing that does not claim the entry can read the
  token of a handle that died beside the liveness lock of the handle that has since taken the
  entry, and report the dead handle live (T9 below).
- **The same rule on the registering side is that the claim lock is held from before the
  liveness lock until after the token is written.** With the claim lock free, an entry's token
  names the handle that holds its liveness lock: no one can see a new owner's lock beside an old
  owner's token. A registrant that lets go of the claim lock early, or takes an entry without
  trying its liveness lock, breaks that, and the state-form of the rule says so without a
  listing to witness it.
- **Every acquisition of a registry lock is a try.** A registrant that finds a claim lock held or
  a liveness lock held moves on to the next entry, and a listing that finds a claim lock held
  reports the entry unknown; an owner leaving the registry that finds its claim lock held
  releases only its liveness lock. No process waits for a registry lock, and a configuration with
  nothing giving up or dying finds no deadlock.
- **Clearing an entry is not what makes it dead; releasing the liveness lock is.** An owner that
  cannot claim its entry when it leaves skips the clearing, and the entry then carries the token
  of a handle that is gone beside a free liveness lock, which a listing reads as dead. The
  model checks this: removing the clearing step from unregistration is invisible to every
  property.
- **A death releases the locks it held and changes no entry.** An entry left by a handle that
  died stays as it was, reads as dead because nothing holds its liveness lock, and is reused by
  the next registrant, including after a death between taking an entry's claim lock and its
  liveness lock. A host crash discards every entry and the marker with the rest of the
  coordination state.
- **Finding: the contract says listings "then say they may be incomplete" after a handle
  attaches unregistered, and the entries alone cannot carry that.** A listing that infers
  incompleteness from what it sees -- no free entry, or an entry it could not claim -- misses a
  handle that attached without an entry while every entry was held and was listed after one
  freed. The registry therefore records an overflow, set by the handle that attaches without an
  entry and discarded only by a host restart, and every listing reads it. Never clearing it is
  conservative and correct; making it precise would need a count updated under a lock, which is
  not modelled and is not needed for the guarantee. The requirement is queued in MP-5[^mp-5].
- **Finding: "every entry held" includes an entry whose claim lock is merely held for a moment.**
  A listing, or another registrant, holding a claim lock sends a registrant on to the next
  entry, so a handle can attach unregistered while an entry is free (T8 below). That costs
  diagnosis precision and nothing else, since the overflow is announced; retrying an entry is an
  implementation choice the model does not constrain.
- **A listing runs on a thread of its own, on a process that need not be attached, and a process
  does not end while its listing runs.** Listing needs no attachment, which is what lets an
  administrator see who blocks an upgrade; the model lets a process that has not attached, or
  is detached, list. A process that ends with a listing under way would leave the claim lock of
  an entry held by a process that is gone.
- **An entry's data is a token and a draining flag.** The flag is updated under the claim lock
  when its handle starts or stops draining, and nothing claims the entry reflects the handle's
  other state: the registry is for diagnosis, and its accuracy beyond the token and the liveness
  probe is not a contract.
- **Entries are optional in the model.** A configuration with no entries leaves the registry out
  and every earlier configuration unchanged. Attachment now ends with a registration step and
  detaching begins with an unregistration step, each a part of the attach or the drop and not an
  operation, so no operation can start between a handle's admission and its entry.
- **Controls and witnesses.** The controls reverse a rule: a listing that does not claim the
  entry it inspects, which must report a dead handle live; a registrant that lets go of the
  claim lock early; an upgrader that keeps its entry through the conversion; a handle that
  writes an entry without claiming it; a handle that takes an entry without trying its liveness
  lock; and a listing that infers incompleteness from fullness. The witnesses show an entry
  written, reused after its handle died and cleared, a registrant moving on from a held claim
  lock and from a live entry, an owner updating its entry, a handle leaving at a drop and at a
  conversion, a clearing skipped because the claim lock was held, a listing seeing an entry live,
  dead and free, a dead handle's entry reported dead, an entry the listing cannot claim, a
  listing that says it may be incomplete and one that says it is complete, a handle attaching
  without an entry, and a death between an entry's claim lock and its liveness lock and one
  while holding an entry. Three-process configurations leave out listings, which the two-process
  ones cover, to keep the state space small.

### T8 -- a listing that infers incompleteness from fullness misses a handle that attached without an entry

Status: `[verified]` against the model, in the control that infers from fullness. H is a handle
and L a listing thread of the same process; the registry has one entry.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `BGN1` | H | begins to attach | -- |
| 2 `GTS1` | H | holds the gate shared | gate: shared by H |
| 3 `RDS1` | H | takes the reader tier shared | reader tier: shared by H |
| 4 `DRP1` | H | drops the gate | gate: none |
| 5 `WRS1` | H | takes the writer tier shared | writer tier: shared by H |
| 6 `ADM1` | H | passes the check and goes on to register | -- |
| 7 `RGS1` | H | starts to look for an entry | -- |
| 8 `LSB` | L | starts a listing | -- |
| 9 `LCL` | L | claims the entry | claim lock: L |
| 10 `RGC1` | H | finds the claim lock held and moves on; no entry is left | -- |
| 11 `OVF` | H | attaches without an entry; the registry records that a handle is missing | overflow marker: set |
| 12 `LRD` | L | reads the entry's token and finds none | -- |
| 13 `LPB` | L | finds the entry free and lets go of the claim lock | claim lock: none |
| 14 `LDN` | L | says the listing is complete, since it found a free entry and met none it could not claim | -- |

Under the contract step 14 reads the overflow marker and says the listing may be incomplete.

### T9 -- a listing that does not claim an entry reports a dead handle live

Status: `[verified]` against the model, in the control that lists without claiming. A and B are
handles and L a listing thread of B; the registry has one entry.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `BGN1` | A | begins to attach | -- |
| 2 `GTS1` | A | holds the gate shared | gate: shared by A |
| 3 `RDS1` | A | takes the reader tier shared | reader tier: shared by A |
| 4 `DRP1` | A | drops the gate | gate: none |
| 5 `WRS1` | A | takes the writer tier shared | writer tier: shared by A |
| 6 `ADM1` | A | passes the check and goes on to register | -- |
| 7 `RGS1` | A | starts to look for an entry | -- |
| 8 `RGC1` | A | claims the entry | claim lock: A |
| 9 `RGL1` | A | takes the entry's liveness lock | liveness lock: A |
| 10 `RGW1` | A | writes its token | entry: token A |
| 11 `KILL` | EXTERNAL | A's process dies | claim lock: none; liveness lock: none; entry still token A |
| 12 `BGN2` | B | begins to attach | -- |
| 13 `GTS2` | B | holds the gate shared | gate: shared by B |
| 14 `RDS2` | B | takes the reader tier shared | reader tier: shared by B |
| 15 `DRP2` | B | drops the gate | gate: none |
| 16 `WRS2` | B | takes the writer tier shared | writer tier: shared by B |
| 17 `ADM2` | B | passes the check and goes on to register | -- |
| 18 `RGS2` | B | starts to look for an entry | -- |
| 19 `RGC2` | B | claims the entry | claim lock: B |
| 20 `RGL2` | B | takes the liveness lock, which no one holds | liveness lock: B; entry still token A |
| 21 `LSB` | L | starts a listing | -- |
| 22 `LCL` | L | begins to inspect the entry without claiming it | -- |
| 23 `LRD` | L | reads the entry's token: A | -- |
| 24 `LPB` | L | tries the liveness lock in shared mode, is refused because B holds it, and reports A live | -- |

Under the contract step 22 claims the entry, which B holds: L reports the entry unknown and says
the listing may be incomplete, and no handle is reported live that is not.

## Creating a store root, modelled in `Creation.tla`, 2026-11-11

- **Creation is a model of its own, and the marker is what it hands to the rest of the
  contract.** [Creation.tla](spec/Creation.tla) ends where [Coordination.tla](spec/Coordination.tla)
  begins: a root with its marker in place. Creation needs a file's existing and its being
  durable as separate facts, which Coordination.tla leaves to
  [PointerPublication.tla](spec/PointerPublication.tla), and nothing in attachment, exclusion or
  the registry can happen before the marker exists, so one model of both would interleave
  behaviours that cannot occur together. The interface is the marker: an open succeeds only on
  one, and the model checks that the marker an open relies on is durable. No work is queued to
  compose the two models, since neither depends on a property the other establishes beyond that
  marker.
- **A creator reads the root, takes its exclusion, and reads the root again.** It reads the root
  and decides: a marker means the root is a store; anything under the root that creation does
  not make means it is not a root to create in; a reservation means a creation was begun, which
  it resumes if the reservation names its implementation and records its inputs, and refuses
  otherwise; no reservation means it creates. It then takes its own implementation's creation
  exclusion, waiting for it, and reads the root again under it. A creator publishes the
  reservation with an exclusive publish-if-absent; a resumer confirms that no marker has
  appeared, that the reservation is the one it read and that the root is still unoccupied. It
  makes the reservation durable, then makes the artifacts in order, each durable before the
  next, the last being the marker, and lets go of the exclusion when the marker is durable.
- **Only the reservation can arbitrate between implementations.** A creation exclusion excludes
  only the processes of its own implementation, so two creators of different implementations
  hold different exclusions at once. The reservation is what they share: whoever publishes it
  owns the creation, and the other finds it and stops. A publication that replaces a reservation
  that is there starts two creations (T11 below), and the property that at most one process is
  creating holds only because publishing is exclusive.
- **The reservation is complete or absent, across a host crash as well as at every step.** A
  reservation written in two steps lets another process read one that is not complete (T10
  below), and a crash can keep the first step and lose the second. The model takes publish-if-absent
  to leave the complete file or none even when power is lost before the file is durable; that is
  a requirement on the primitive, queued in AC-1[^ac-1].
- **Durability is ordered: the reservation, then each artifact, then the marker.** The reservation
  is durable before any other artifact exists, and each artifact is durable before the next is
  made, so the marker exists only when every artifact before it is there and durable. Power loss
  keeps what is durable and keeps or loses each other file on its own, so without that order a
  crash leaves artifacts whose reservation is gone, or a marker whose artifacts are gone. The
  reservation's file and its directory entry are one unit in the model, made durable together.
  A creator that died before making the reservation durable leaves it for its resumer to make
  durable before going on.
- **A resumer reads the root again under the exclusion, because the creation it read may be
  live.** A reservation without a marker looks the same whether its creator died or is still
  working, and no observer can tell which. The creator holds its exclusion until the marker is
  durable, so a resumer that waits for the exclusion either meets a creator that died, or one
  that finished and left a marker. Reading the marker again after the wait is what tells them
  apart, and a resumer that skips it creates over a store whose creation has finished (T12
  below). The reread also confirms the reservation is the one it decided on.
- **A resumer compares its inputs with the reservation's, and the artifacts record the inputs
  they were made from.** A creation resumed with different inputs would mix two sets in one
  store. The model gives every creator one number for its inputs and checks that the process
  creating, and every artifact, carries the inputs the reservation records.
- **A root holding anything creation does not make is not created in or resumed.** The root is
  scanned before the exclusion, so a root that is plainly not ours is refused without taking it,
  and again under it, because something can appear in between. The check applies to a resumer as
  to a creator, and a refusal changes nothing in the root.
- **Finding `UNSYNCED-MARKER`: an open must make the marker durable before it relies on it.** A
  creator publishes the marker and then makes it durable, and holds its exclusion until it has.
  If it dies between the two, its exclusion is released with the marker visible and not durable,
  and an opener that trusts a visible marker opens the store; a host crash can then take the
  marker away from a store whose documents are already durable (T13 below). An open therefore
  makes the marker durable before it relies on it. Doing so costs one idempotent sync, and the
  marker's creator has made every artifact before it durable, so nothing else needs it. The
  requirement is queued in MP-5[^mp-5].
- **Finding: an opener's wait for the creation exclusion is not what makes an open safe.** The
  marker is published atomically and last, so an opener that reads the root sees a complete
  store or none. Removing the opener's exclusion from the model is invisible to every property.
  The model keeps it because the contract states it; whether an implementation needs it for
  anything the model does not cover is not decided here.
- **What the model leaves out.** An artifact is abstract: it exists, is durable, and records
  inputs. The failure of an I/O step is not a separate event, since a creator that stops at a
  step because it failed is the same as one that dies there, and the model checks a death, and a
  host crash, at every step. A waiting creator never gives up. The reservation's file and its
  directory entry are not made durable separately, so an implementation that syncs only the file
  is not distinguished here, and is left to MP-5's crash test at each step of creation[^mp-5].
- **Controls and witnesses.** The controls reverse a rule: a reservation published in two
  writes, which another creator must read between them; artifacts begun before the reservation is
  durable; an artifact not made durable before the next, so the marker outlives what it depends
  on; a publication that replaces a reservation, which runs two creations and replaces a
  published reservation; a reservation published without holding the exclusion; a resumer that
  takes no exclusion, or does not read the root again, or ignores the inputs, which makes the
  marker from different ones; an open through an implementation the reservation does not name;
  an open that trusts a marker its creator died before making durable; and a creator that does
  not refuse a root holding something else. The witnesses show a creation completing and one
  resumed after a death and after a host crash, a crash losing the reservation, a crash leaving
  only the reservation and one leaving every artifact but the marker, a creator waiting for the
  exclusion, stopping at another implementation's reservation, at different inputs, after losing
  the race, at a finished store and at an occupied root, found before and after the exclusion,
  a resumer finding the store finished while it waited, and an open opening, being refused for
  another implementation, finding no store or a creation in progress, and making a marker
  durable that its creator did not. Probes in scratch copies of the spec -- a resumer that does
  not make the reservation or an artifact durable, a crash that loses what was durable, and each
  place the root is checked for being occupied -- were caught by the intended property each time,
  and removing the opener's exclusion, above, was not caught.

### T10 -- a reservation published in two writes is read between them

Status: `[verified]` against the model, in the control that publishes in two writes. A and B are
creators of one implementation with the same inputs.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `LOOK1` | A | reads the root and finds nothing, and decides to create | -- |
| 2 `LOCK1` | A | takes its implementation's creation exclusion | creation exclusion: A |
| 3 `PUBP` | A | writes the first part of the reservation: the file exists and is empty | reservation: partial |
| 4 `LOOK2` | B | reads the root and finds a reservation that is not complete, which no step of the contract produces, and stops | -- |

Under the contract step 3 publishes the complete reservation or nothing, so B finds none or a
whole one.

### T11 -- a publication that replaces a reservation runs two creations at once

Status: `[verified]` against the model, in the control whose publication replaces. A uses
implementation 1 and B implementation 2; they carry the same inputs.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `LOOK1` | A | reads the root and finds nothing, and decides to create | -- |
| 2 `LOOK2` | B | reads the root and finds nothing, and decides to create | -- |
| 3 `LOCK1` | B | takes implementation 2's creation exclusion | creation exclusion 2: B |
| 4 `PUB1` | B | publishes its reservation, naming implementation 2 | reservation: B's |
| 5 `LOCK2` | A | takes implementation 1's creation exclusion, which B's exclusion does not hold back | creation exclusion 1: A |
| 6 `PUB2` | A | publishes its reservation over B's, naming implementation 1 | reservation: A's |

A and B are both creating, each believing it owns the creation. Under the contract step 6 finds
B's reservation and A stops.

### T12 -- a resumer that does not read the root again creates over a finished store

Status: `[verified]` against the model, in the control that does not read again. A and B are
creators of one implementation with the same inputs; A creates while B reads.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `LOOK1` | A | reads the root and finds nothing, and decides to create | -- |
| 2 `LOCK1` | A | takes the creation exclusion | creation exclusion: A |
| 3 `PUB` | A | publishes the reservation | reservation: complete, not durable |
| 4 `SYNR` | A | makes the reservation durable | reservation: durable |
| 5 `LOOK2` | B | reads the root and finds A's reservation for its implementation and inputs, and no marker; it cannot tell A's live creation from an interrupted one, and decides to resume | -- |
| 6 `WRA1` | A | makes the first artifact | artifact 1: exists |
| 7 `SYA1` | A | makes it durable | artifact 1: durable |
| 8 `WRM` | A | makes the marker | marker: exists |
| 9 `SYM` | A | makes the marker durable and lets go of the exclusion | marker: durable; creation exclusion: none |
| 10 `LOCK2` | B | takes the creation exclusion, which is now free | creation exclusion: B |
| 11 `CNFM` | B | goes on to create without reading the root again | -- |

Under the contract step 11 reads the root again, finds the marker and stops.

### T13 -- a creator dies before its marker is durable, an opener relies on the marker, and power is lost

Status: steps 1 to 9 are `[verified]` against the model, in the control whose open trusts a
visible marker; step 10 is a step the model allows and the control does not reach, since the
property is already violated at step 9. A creates, with an opener O of its implementation.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `LOOK` | A | reads the root and finds nothing, and decides to create | -- |
| 2 `LOCK` | A | takes the creation exclusion | creation exclusion: A |
| 3 `PUB` | A | publishes the reservation | reservation: complete, not durable |
| 4 `SYNR` | A | makes the reservation durable | reservation: durable |
| 5 `WRA1` | A | makes the first artifact | artifact 1: exists |
| 6 `SYA1` | A | makes it durable | artifact 1: durable |
| 7 `WRM` | A | makes the marker | marker: exists, not durable |
| 8 `KILL` | EXTERNAL | A's process dies | creation exclusion: none |
| 9 `OPEN` | O | reads the root, finds the marker and a reservation naming its implementation, and opens the store on a marker that is not durable | -- |
| 10 `CRSH` | EXTERNAL | power is lost, and the marker, which was never durable, is lost | marker: absent |

`[projected]` A store opened at step 9 may have made documents durable before step 10, and the root
then holds them with a durable reservation and artifacts and no marker, which no open accepts and
no creation resumes over, since the documents are something creation does not make. Under the
contract step 9 makes the marker durable before it opens, so step 10 loses nothing an open relied
on.

[^ac-1]: `AC-1` in [CHECKLIST.md](../acadia/CHECKLIST.md#ac-1)

## A transaction and the commit log, modelled in `Coordination.tla`, 2026-11-11

- **A write is a transaction behind a switch, and the abstract write stays.** With `Txn` false a
  write is the one abstract operation the earlier models use: two cells changed in turn inside
  the count's bracket. With it true a write is `begin`, ingest, `prepare`, namespace apply and
  `end` over a commit log, staging and the namespace, with the count bracketing the whole of it.
  The switch leaves every earlier configuration, and its state space, unchanged, so the gate,
  tiers, leases and registry are still checked against the cheaper write. Whether the abstract
  form is retired once the refined one is proven is the engineer's decision, kept open[^odi-3].
- **The prepare record is the commit point: before it nothing is visible, and after it nothing
  is undone.** Ingest only stages bodies, which no pointer names, so a failure before `prepare`
  is rolled back by sweeping the staging and resetting the log, with the namespace as it was and
  the operation ending without an effect. The swap set is in the prepare record, so from the
  moment that record may be in the log the transaction can only be rolled forward, whether or not
  its holder ever learns it was written. The namespace moves only after the record, and the end
  record follows every swap; each has a control that reverses it.
- **A failure at or after the commit point is unresolved, including the failure of the prepare
  append itself.** An append that fails may or may not have written its record, and the holder
  cannot tell which, so it sets the fence, leaves the count odd and the log as it is, and is told
  recovery is required; the model takes both outcomes. Failing to make a swap, to append the end
  record, or to sweep after it, are the same: the store is whole or it is not, and only the log
  says which. A control that handles such a failure as one before the commit point leaves a
  half-applied namespace, an even count and nothing to say so.
- **The log is modelled by its tail.** Transactions are serialised, so only the latest can be
  incomplete, and the interaction between processes depends on whether that tail is empty,
  begun, prepared or ended and on nothing earlier. Staging is the version whose bodies are staged,
  and the prepare record carries the version its swaps move to. The two cells are a transaction's
  two swaps, and a read sees one and not the other exactly as before.
- **Staging is always announced by the log.** A begin record is written before anything is
  staged, so recovery can find and sweep whatever a dead holder staged. A reset that discards a
  begun transaction while its staging remains leaves it unannounced, and the model checks that no
  state has staged bodies without a record to say so.
- **The commit-log reset is made by a holder with nothing in flight, and only of a settled
  log.** A reset made while a transaction is in flight, or by an acquirer that was told recovery
  is required, discards a prepared transaction that only the log lets recovery finish; each is a
  control. A holder that rolls a transaction back resets the log as part of the rollback.
- **Every record changes what a handle may have cached of the log.** The version of the shared
  durable state moves with each record appended and each reset, and the holder updates its own
  cache, so another handle reloads when the generation shows someone came between; that rule is
  unchanged.
- **A holder that dies at any phase leaves evidence.** The ownership record names it, so the
  next acquirer is told recovery is required whether the holder died after `begin`, with its
  bodies staged, with `prepare` in the log, between its swaps, after the last, or after `end`
  and before its sweep. A host crash leaves a prepared transaction in the log as well, and the
  untrusted state is the evidence. Writing past any of it is the one thing a transaction may
  never do, which the model checks at the start of each.
- **What the model leaves out.** The content of the bodies and the collection of what a
  transaction supersedes belong to
  [PointerPublication.tla](spec/PointerPublication.tla); a torn record at the log's end belongs
  to the log's own reader; and the nonce claims a transaction's artifacts carry are the
  implementation's. None changes what processes see of one another.
- **Controls and witnesses.** The controls reverse a rule and must fail: a reset made while a
  transaction is in flight and one made by an acquirer told recovery is required, which also
  changes store data without being a healthy holder; a swap before `prepare`; a `prepare` before
  ingest; an `end` before the swaps; and a failure after `prepare` handled as a rollback. The
  witnesses show a transaction committing, a failure before ingest and after it, a prepare append
  that fails with and without a record, a failure after the prepare record, after the last swap
  and after `end`, the log reset, and a holder dying after each phase with the next acquirer
  told, and a host crash leaving a prepared transaction. Probes in scratch copies of the spec
  that break one rule each -- an empty swap set, an end that keeps it, a rollback that keeps the
  staging or the log, a sweep that is skipped, a begin before the opening raise, a request that
  resets the log, a rollback that writes the namespace, and a failure that does not set the
  fence -- were each caught by the intended property.

[^odi-3]: `ODI-3` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#odi-3) [completed 2026-11-11]

## Recovery and its discovery, modelled in `Coordination.tla`, 2026-11-11

- **Recovery is a capability of a holder that was told, and it decides what to do from the
  commit log alone.** A holder told that recovery is required, or whose own transaction failed
  unresolved, may run recovery; until it does it can only release. The fence refuses ordinary
  writes and not recovery, so the fence that says recovery is needed never blocks the work that
  clears it. Recovery looks at the log's last record: a prepared transaction is rolled forward
  through its swaps and its end record, one that was begun and not prepared is rolled back, and
  anything else is already resolved. It does not read the fence, the ownership record or the
  trust flag to choose its steps, since they say that recovery is required and nothing about
  what it should do. A recovery that rolls a prepared transaction back discards a commit the
  log says was reached, which a control shows.
- **The steps run in one order, and each position is what a recoverer's stop or death depends
  on.** The swaps, the end record and the sweep of the staging come first; a transaction that was
  only begun has its staging swept and then its log reset. The count and the trusted state are
  published next, together, and the fence is cleared last.
  - The staging is swept before the log is reset, so that between the two there is always a
    record announcing it.
  - Nothing is published until the namespace is whole. An even count published earlier lets a
    read sample it on both sides of a recoverer that stopped between its swaps and classify a
    half-applied store as settled (T14 below).
  - The count and the trust are one publication, so no state reads as trusted with the odd count
    it had, and none as an even count that cannot be vouched for.
  - The fence is cleared last. It keeps writers out and later acquirers told while anything is
    unresolved; cleared when recovery starts, it leaves a recoverer that stops with unresolved
    work and nothing to mark it, and the next holder begins a transaction over that work (T15
    below).

  Between the count's publication and the fence's clearing the store is settled, even and
  trusted with the fence still set. A recoverer that stops or dies there leaves a store for which
  the next acquirer is told recovery is required; that acquirer finds nothing to finish,
  republishes the count unchanged and clears the fence.
- **Every step takes what it needs from durable state and is idempotent, so a resumed recovery
  is the same recovery.** A swap made again lands on the cell it already moved, the end record is
  written only while the log's last record is the prepare record, a sweep removes nothing that is
  not there, and the count is the even value at or above what a sampler of the store reads now,
  so republishing it changes nothing. A recoverer that stops between steps goes back to the told
  state, still holding exclusion with the fence set. One that dies leaves the record, the fence
  and the count where they were, and the next acquirer is told and starts from what the log says
  (T16 below).
- **Publishing the recovered count drops every cached sample, as every publication does.**
  Restoring trust changes what a sampler of an untrusted store reads -- an even count that cannot
  be vouched for reads one higher -- so a sample cached while the state was untrusted is stale
  the moment trust returns.
- **Discovery is one observation of four things, and it reports abandoned work and never work in
  progress.** Any attached handle, one that may only read included, may ask whether recovery is
  required. In one step it reads the fence, the restart state, the ownership record and whether
  the lock is held, and reports recovery required when the fence is set, when the state cannot be
  vouched for, or when a record names a holder and the lock is free, which can only mean the
  holder died. A record beside a held lock is a live holder's transaction and is reported as not
  requiring recovery; discovery that trusts the record alone reports it as abandoned (T17
  below). A recovery in progress is still reported, since the fence stays set until it finishes,
  and when no one holds exclusion every one of the three signals is.
- **Discovery changes nothing but its own report.** A discovery that clears a fence it finds set
  while no one holds exclusion makes the answer disappear while the work remains: the next
  acquirer is not told, and the unresolved transaction has nothing to mark it. The model checks
  that no step that produces a report changes anything else, and the control that repairs what it
  finds violates it.
- **What the model leaves out.** Discovery is one step. An implementation cannot read files and
  probe a lock in one step, so it must make its observation consistent with one -- by trying the
  writer lock without waiting and reading under it, for example -- and the model does not say
  whether a weaker order is also safe. The progress a recovery reports and its cost are not
  modelled, nor is its handling of the publication record and of directory-sync debt, which
  belong to [PointerPublication.tla](spec/PointerPublication.tla) and the composition of the
  two; and whether a driven recovery eventually completes is liveness, which the model of
  fairness checks[^mp-4].
- **What it asks of an implementation, queued in the recovery work[^mp-6].** Recovery reads the
  log and never the coordination state. Its steps run in the order above with the fence last,
  and a stop or a death at any step leaves the fence set. The publication of the recovered count
  restores trust and drops cached samples. Discovery's observation is consistent, as above, and a
  live holder's transaction is never reported as abandoned while the holder acquires or releases.
- **Controls and witnesses.** The controls reverse a rule and must fail: discovery that trusts
  the ownership record alone, that does not read the fence, that does not read the restart state,
  and that clears the fence it finds; recovery that clears the fence when it starts, which a
  recoverer that stops leaves unprotected and which lets the next holder begin a transaction over
  unresolved work; recovery that publishes the count before the namespace is whole, which lets a
  read accept a half-applied store and lets trust return before the swaps; and recovery that
  rolls a prepared transaction back. The witnesses show a recovery rolling a prepared
  transaction forward and one that was only begun back, completing and clearing the fence,
  restoring trust, following a host crash, a holder's death and an unresolved failure of the
  holder's own, following a crash that rolled the count back over a prepared transaction, being
  stopped, a recoverer dying after each of its steps with the next acquirer told, and a recovery
  resumed after an earlier one died, once between its swaps; and discovery finding a dead
  holder's work, the fence and untrusted state, a healthy store, a live holder's transaction, and
  being called by a handle that may only read. Probes in scratch copies of the spec -- a sweep
  skipped, an end record not written, a swap skipped or both, a count left odd, trust not
  restored, a fence never cleared or cleared when a recoverer stops, a log reset before the
  sweep, and discovery that reads nothing, always reports, or ignores a dead holder's record --
  were each caught by the intended property.

### T14 -- a recoverer that publishes the count before its swaps lets a read accept a half-applied store

Status: `[verified]` against the model, in the control that publishes the count first, with a
read. A has attached; the read is A's own, after it lets go, and any attached handle could take it.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `REQ1` | A | requests writer exclusion | -- |
| 2 `LCK1` | A | is granted the lock | lock: A |
| 3 `REC1` | A | finds no record and no fence, and records itself as holder | record: A |
| 4 `OPN` | A | prepares the opening raise | -- |
| 5 `PUB` | A | publishes it | count: 1 |
| 6 `BEGN` | A | appends the begin record | log: begun |
| 7 `STG` | A | stages its bodies | staging: version 1 |
| 8 `PRPF` | A | appends the prepare record; the append fails with the record in the log, which A cannot tell, so it sets the fence and is told recovery is required | log: prepared, naming version 1; fence: set |
| 9 `RSRT` | A | starts recovery and, in the control, goes to publishing the count | -- |
| 10 `RCNT` | A | publishes the count, even and trusted, before any swap | count: 2 |
| 11 `RSWA` | A | makes the first swap | cell a: version 1 |
| 12 `RSTP` | A | stops recovery between its swaps, holding exclusion | -- |
| 13 `CLR` | A | clears the record while holding the lock | record: none |
| 14 `ULK` | A | drops the lock | lock: none |
| 15 `SMP1` | A | samples the count and gets 2 | -- |
| 16 `OBSA` | A | observes cell a: version 1 | -- |
| 17 `OBSB` | A | observes cell b: version 0 | -- |
| 18 `SMP2` | A | samples the count again, gets 2, and classifies the read as settled | -- |

Under the contract step 10 comes after the swaps, the end record and the sweep, so the samples in
steps 15 and 18 read 1, an odd count, and the read is contended.

### T15 -- a recoverer that clears the fence when it starts lets the next transaction begin over unresolved work

Status: `[verified]` against the model, in the control that clears the fence at the start. A has
attached.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `REQ1` | A | requests writer exclusion | -- |
| 2 `LCK1` | A | is granted the lock | lock: A |
| 3 `REC1` | A | finds no record and no fence, and records itself as holder | record: A |
| 4 `OPN1` | A | prepares the opening raise | -- |
| 5 `PUB1` | A | publishes it | count: 1 |
| 6 `BEGN1` | A | appends the begin record | log: begun |
| 7 `STG` | A | stages its bodies | staging: version 1 |
| 8 `PRPF` | A | appends the prepare record; the append fails and leaves no record, which A cannot tell, so it sets the fence and is told recovery is required | fence: set |
| 9 `RSRT` | A | starts recovery and, in the control, clears the fence now | fence: clear |
| 10 `RSTP` | A | stops recovery before any step, holding exclusion | -- |
| 11 `CLR` | A | clears the record while holding the lock | record: none |
| 12 `ULK` | A | drops the lock | lock: none |
| 13 `REQ2` | A | requests writer exclusion again | -- |
| 14 `LCK2` | A | is granted the lock | lock: A |
| 15 `REC2` | A | finds no record, no fence and a trusted state, and records itself as holder; it is not told anything | record: A |
| 16 `OPN2` | A | prepares the opening raise | -- |
| 17 `PUB2` | A | publishes it, and the count reads even while an operation is open | count: 2 |
| 18 `BEGN2` | A | appends a begin record over the first transaction's, with its staging still there | log: begun |

Under the contract step 9 leaves the fence set, so step 15 finds it and A is told recovery is
required and writes nothing.

### T16 -- a recovery resumed after its recoverer died between its swaps

Status: steps 1 to 20 are `[verified]` against the model, in the witness for a recovery resumed
after such a death; step 21 is a step the model allows and the witness does not reach, since it
is violated at step 20. A and B have attached.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `REQ1` | A | requests writer exclusion | -- |
| 2 `LCK1` | A | is granted the lock | lock: A |
| 3 `REC1` | A | finds no record and no fence, and records itself as holder | record: A |
| 4 `OPN` | A | prepares the opening raise | -- |
| 5 `PUB` | A | publishes it | count: 1 |
| 6 `BEGN` | A | appends the begin record | log: begun |
| 7 `STG` | A | stages its bodies | staging: version 1 |
| 8 `PRPF` | A | appends the prepare record; the append fails with the record in the log, which A cannot tell, so it sets the fence and is told recovery is required | log: prepared, naming version 1; fence: set |
| 9 `RSRT1` | A | starts recovery, which reads the log: a prepared transaction, to roll forward | -- |
| 10 `RSWA1` | A | makes the first swap | cell a: version 1 |
| 11 `KILL` | EXTERNAL | A's process dies | lock: none; record still names A; fence set; store half applied |
| 12 `REQ2` | B | requests writer exclusion | -- |
| 13 `LCK2` | B | is granted the lock | lock: B |
| 14 `REC2` | B | finds a record left behind and the fence already set, replaces the record, and is told recovery is required | record: B |
| 15 `RSRT2` | B | starts recovery, which reads the log: the prepare record is still its last | -- |
| 16 `RSWA2` | B | makes the first swap again, which the cell already holds | -- |
| 17 `RSWB` | B | makes the second swap | cell b: version 1 |
| 18 `REND` | B | appends the end record | log: ended |
| 19 `RSWP` | B | sweeps the staging | staging: none |
| 20 `RCNT` | B | publishes the count, even, and the trusted state, together | count: 2 |
| 21 `RCLR` | B | clears the fence, its last step | fence: clear |

### T17 -- discovery that trusts the ownership record reports a live holder's transaction as abandoned

Status: `[verified]` against the model, in the control that reports on the record alone. A and B
have attached.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `REQ1` | A | requests writer exclusion | -- |
| 2 `LCK1` | A | is granted the lock | lock: A |
| 3 `REC1` | A | finds no record and no fence, and records itself as holder | record: A |
| 4 `DISC` | B | discovers: finds a record naming A, no fence and a trusted state, and reports recovery required because a record is there, without asking whether the lock is held | -- |

Under the contract step 4 finds the lock held by A and reports that recovery is not required: A's
record beside a held lock is a transaction in progress.

[^mp-4]: `MP-4` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4) [completed 2026-11-12]
[^mp-6]: `MP-6` in [CHECKLIST.md](CHECKLIST.md)

## The schema upgrade's body, modelled in `Coordination.tla`, 2026-11-11

- **The body is behind a switch, and the one-step upgrade stays.** With `UpSteps` zero an upgrade
  completes in one step when its upgrader holds its tiers, as the model of attachment has it,
  and every earlier configuration, with its state space, is unchanged. With it set, the upgrader
  goes on from its tiers to a body of `UpSteps` migration steps, so the gate, the tiers, the
  leases and the registry are still checked against the cheaper form. Whether the one-step form
  is retired once the body is proven is the engineer's decision, kept open[^odi-3].
- **The upgrader takes writer exclusion last, as the lock order says, and finds it free.** The
  order is the gate, the reader tier, the writer tier, then writer exclusion. An upgrader holds the
  writer tier exclusively, so no handle that may write is attached to hold or want the lock, and
  a dead holder's lock is released with it; the model checks that the lock is free rather than
  waiting for it. The upgrader then acquires as any holder does: it finds the record, the fence
  and the trust flag, and is told or not.
- **An upgrade starts only from a store that needs no recovery.** An upgrader that is told
  recovery is required releases, its upgrade refused and its handle detached, and runs nothing: it
  speaks the source schema, which is not the schema a recovery of an upgrade needs, and the
  permission table already refuses an upgrade while recovery is required. An upgrader that goes
  on regardless starts a migration over work nothing has resolved (T21 below).
- **The body is inside the count's bracket, and the order within it is the safety argument.**
  The opening raise is published first. The upgrader then records the upgrade intent -- the layer
  is recorded as being upgraded, with no step durable -- makes the migration steps one at a time,
  each durable when it ends, advances the layer's schema only after the last, clears the intent,
  and publishes the closing raise.
  - The intent comes before the first step. A step made first leaves a layer partly migrated with
    nothing to say it is, so an upgrader that dies after it leaves recovery nothing to resume (T19
    below).
  - The schema advances after every step. Advanced earlier, handles are admitted against a layer
    that is only partly migrated.
  - The intent clears after the schema has advanced, and takes the progress with it. Cleared
    earlier, or by a recovery that discards the progress, an upgrade that cannot be undone is
    undone, and the next writer finds the old schema over steps already made.
  - The count stays odd from the opening raise to the closing one, so every effect of the body is
    inside the bracket and a reader's samples bracket nothing while it runs.
- **The body cannot be rolled back, so a failure in it is unresolved.** A failure between the
  intent and its clearing, a cancellation there included, sets the fence, leaves the count odd and
  the intent standing, and the upgrader is told recovery is required. It can only release. Before
  the intent a failure leaves nothing behind, and the upgrader ends its operation as a writer that
  found nothing to change does; after the intent clears, one in the closing publication is a
  failure of that publication and nothing more.
- **Recovery of an interrupted upgrade is resuming it, by a handle that speaks the target
  schema.** An upgrader that dies leaves its record, its intent and the progress it made. The next
  acquirer is told, and a recovery that finds an intent standing resumes the upgrade before it does
  the rest of recovery: it makes the steps that remain, advances the schema if that has not been
  done, and clears the intent, and then settles the log and publishes the count as any recovery
  does. Each of those is decided from the intent, the progress and the schema as durable state
  shows them, so a resumer that stops or dies leaves the rest to the next, which repeats nothing it
  need not (T18 below).
- **A handle that speaks only the source schema never holds exclusion while an intent stands.**
  The rule is enforced where the contract puts it, at admission: while an upgrade is in progress,
  recorded or interrupted, a writer is admitted only if it speaks the target schema. The model
  checks the consequence -- whoever holds exclusion during an upgrade is its upgrader or speaks
  the target -- and that no resumption is made by any other handle, including the upgrader that
  failed, which speaks the source schema. A control that admits source writers during an
  interrupted upgrade violates both (T20 below). A store whose upgrade failed therefore stays
  recovery-required for every handle that speaks only the source schema, which cannot be
  admitted to recover it.
- **A safe upgrade lets readers attach during its body, and an unsafe one does not.** The gate
  reopens to readers once the body begins when the upgrade is safe for them, and an unsafe one
  keeps the reader tier exclusively; the oldest reader schema moves with the schema, and only for
  an unsafe upgrade.
- **Upgrade starts are bounded apart from write requests.** An upgrade that took exclusion from the
  budget of write requests would starve whenever writers used it, and the model would report a
  deadlock that no execution has. Each start counts against its own bound, and only when the body
  is modelled.
- **What the model leaves out.** What a migration step does is abstract: it is durable when it ends
  and idempotent, and the layer is partly migrated between the first and the last. The progress an
  upgrade reports and what its steps cost are not modelled. Which reader schemas an upgrade must
  leave readable, across layers and across successive upgrades, is the cumulative compatibility of
  readers, modelled apart[^mp-2-4]; and whether a driven resumption eventually completes is
  liveness, checked with the fairness the contract grants[^mp-4].
- **What it asks of an implementation, queued in the upgrade work[^mp-7].** An upgrade that finds
  recovery required is refused and takes nothing. The intent names the layer and its source and
  target schemas, and is durable before the first step. Steps are idempotent and each takes what it
  needs from the intent and the progress durable state shows. The schema is advanced durably only
  after the last step, and the intent cleared only after that. A failure or a cancellation in the
  body sets the fence and leaves the intent. A resumption is the store's recovery, made by a handle
  that speaks the target schema, and no handle that speaks only the source schema writes while the
  intent stands.
- **Controls and witnesses.** The controls reverse a rule and must fail: a migration step made
  before the intent; a schema advanced before the last step; a recovery that rolls an interrupted
  upgrade back; a writer that speaks only the source schema admitted while an upgrade is
  interrupted, which then holds exclusion and resumes it; an upgrader told that recovery is
  required that goes on; and a clone that starts an operation on a handle whose upgrade is in its
  body. The witnesses show the intent recorded, a step made, the schema advanced and an upgrade
  completing through its body, one that gives up before the intent and one that fails in its
  body, an upgrader refused after being told and its handle detached, an upgrader dying before the
  intent, before its first step, between steps, after its last step and after the schema
  advances, each followed by a next acquirer that is told, a recovery resuming before any step,
  between steps and after the schema advanced, making the remaining steps, advancing the
  schema, completing and being stopped, one dying in the upgrade with another told, a reader
  admitted during the body, an upgrade unsafe for readers moving the oldest schema, an upgrade
  completing with its closing publication failed, and discovery meeting a live upgrader's body and
  finding an interrupted upgrade. Probes in scratch copies of the spec -- an intent never
  recorded, an intent cleared before the schema advances, a resumer that starts the steps over,
  a failure that sets no fence, a completed upgrade reported refused, the oldest schema left
  alone, the intent recorded before exclusion and outside the count, and a failed upgrader that
  resumes its own upgrade -- were each caught by the intended property.

### T18 -- an upgrader dies between two migration steps, and a writer that speaks the target schema resumes the upgrade

Status: steps 1 to 18 are `[verified]` against the model, in the witness for a recovery that starts
resuming an upgrade between two steps; steps 19 to 25 are the model's next steps, which the witness
does not reach, and are `[projected]`. A is an upgrader that has drained and holds the gate and the
tiers it needs, the upgrade being safe for readers; B is a writer that speaks the target schema and
has not attached. The upgrade has two steps.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `LCK1` | A | takes writer exclusion, which is free | lock: A |
| 2 `REC1` | A | finds no record, no fence and a trusted state, and records itself as holder | record: A |
| 3 `OPN` | A | prepares the opening raise | -- |
| 4 `PUB` | A | publishes it | count: 1 |
| 5 `INTN` | A | records the upgrade intent | intent: recorded, progress 0 |
| 6 `STEP1` | A | makes the first migration step | progress: 1 |
| 7 `KILL` | EXTERNAL | A's process dies | lock: none; record still names A; intent and progress stand; writer tier: free |
| 8 `ATT` | B | begins to attach | -- |
| 9 `GATE` | B | passes the gate shared | -- |
| 10 `RTIR` | B | takes the reader tier | -- |
| 11 `DROP` | B | lets go of the gate | -- |
| 12 `WTIR` | B | takes the writer tier, which A held | -- |
| 13 `ADMT` | B | is checked against the rules in force, which admit a writer only if it speaks the target while an upgrade is in progress, and is admitted | -- |
| 14 `REQ2` | B | requests writer exclusion | -- |
| 15 `LCK2` | B | is granted the lock | lock: B |
| 16 `FNC` | B | finds a record left behind and sets the fence | fence: set |
| 17 `REC2` | B | replaces the record and is told recovery is required | record: B |
| 18 `RSRT` | B | starts recovery, and finds an intent standing, so it resumes the upgrade | -- |
| 19 `STEP2` | B | makes the step that remains, which the progress shows | progress: 2 |
| 20 `ADV` | B | advances the schema of the layer | schema: target |
| 21 `ICLR` | B | clears the intent and its progress | intent: none, progress 0 |
| 22 `REND` | B | finds the log resolved | -- |
| 23 `RSWP` | B | finds nothing staged | -- |
| 24 `RCNT` | B | publishes the count, even, and the trusted state | count: 2 |
| 25 `RCLR` | B | clears the fence, its last step | fence: clear |

### T19 -- a migration step made before the intent leaves nothing to resume from

Status: `[verified]` against the model, in the control that lets a step precede the intent. A is an
upgrader that has drained and holds the gate and the tiers it needs.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `LCK1` | A | takes writer exclusion, which is free | lock: A |
| 2 `REC1` | A | finds no record, no fence and a trusted state, and records itself as holder | record: A |
| 3 `OPN` | A | prepares the opening raise | -- |
| 4 `PUB` | A | publishes it | count: 1 |
| 5 `STEP1` | A | makes a migration step before it has recorded the intent | progress: 1; intent: none |

An upgrader that dies after step 5 leaves its record, so the next acquirer is told recovery is
required; but nothing durable says an upgrade was under way, so that recovery has nothing to
resume, finishes, and leaves a layer partly migrated at the schema it started from. Under the
contract step 5 follows the intent, and a death after it leaves the intent for recovery to resume.

### T20 -- a writer that speaks only the source schema, admitted during an interrupted upgrade, resumes it

Status: `[verified]` against the model, in the control that admits such a writer. A is an upgrader
that has drained and holds the gate and the tiers it needs; B is a writer that speaks only the
source schema and has not attached.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `LCK1` | A | takes writer exclusion, which is free | lock: A |
| 2 `REC1` | A | finds no record, no fence and a trusted state, and records itself as holder | record: A |
| 3 `OPN` | A | prepares the opening raise | -- |
| 4 `PUB` | A | publishes it | count: 1 |
| 5 `INTN` | A | records the upgrade intent | intent: recorded, progress 0 |
| 6 `FAIL` | A | a failure in the body: the layer is partly migrated and cannot be rolled back, so A sets the fence and is told recovery is required | fence: set |
| 7 `KILL` | EXTERNAL | A's process dies | lock: none; record still names A; intent stands; writer tier: free |
| 8 `ATT` | B | begins to attach | -- |
| 9 `GATE` | B | passes the gate shared | -- |
| 10 `RTIR` | B | takes the reader tier | -- |
| 11 `DROP` | B | lets go of the gate | -- |
| 12 `WTIR` | B | takes the writer tier, which A held | -- |
| 13 `ADMT` | B | is checked against the rules in force and, in the control, is admitted although it speaks only the source schema | -- |
| 14 `REQ2` | B | requests writer exclusion | -- |
| 15 `LCK2` | B | is granted the lock | lock: B |
| 16 `REC2` | B | finds a record left behind and the fence set, replaces the record, and is told recovery is required | record: B |
| 17 `RSRT` | B | starts recovery, finds the intent standing, and resumes an upgrade to a schema it does not speak | -- |

Under the contract step 13 refuses B: a writer is admitted while an upgrade is in progress only if
it speaks the target schema, so B never holds exclusion and never resumes.

### T21 -- an upgrader that is told recovery is required goes on

Status: `[verified]` against the model, in the control whose upgrader ignores being told. A is an
upgrader that has attached.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `REQ1` | A | requests writer exclusion, as any writer may | -- |
| 2 `LCK1` | A | is granted the lock | lock: A |
| 3 `REC1` | A | finds no record and no fence, and records itself as holder | record: A |
| 4 `OPN1` | A | prepares the opening raise | -- |
| 5 `PUB1` | A | publishes it | count: 1 |
| 6 `BEGN` | A | appends the begin record | log: begun |
| 7 `STG` | A | stages its bodies | staging: version 1 |
| 8 `PRPF` | A | the append of the prepare record fails and leaves no record, which A cannot tell, so it sets the fence and is told recovery is required | fence: set |
| 9 `CLR` | A | clears the record while holding the lock | record: none |
| 10 `ULK` | A | drops the lock | lock: none |
| 11 `UPST` | A | starts an upgrade, which first drains | -- |
| 12 `DRN` | A | finds no operation of its handle in flight, so its drain ends | -- |
| 13 `GTX` | A | takes the gate exclusively | -- |
| 14 `TRS` | A | takes the tier it needs | -- |
| 15 `LCK2` | A | takes writer exclusion for the body, which is free | lock: A |
| 16 `REC2` | A | finds the fence set and, in the control, goes on as a holder that was not told | record: A |

Under the contract step 16 tells A recovery is required, and A releases: it starts nothing, its
upgrade is refused and its handle is left detached, so no migration step is made over a store with
an unresolved transaction.

[^mp-2-4]: `MP-2.4` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-2-4) [completed 2026-11-11]

## Cumulative reader compatibility, modelled in `Coordination.tla`, 2026-11-11

- **An upgrade states which readers can read its result, and the store judges it.** Reading is
  cumulative in a layer's schemas: what a schema can read, every later schema can read too. So the
  readers that can read what an upgrade leaves are those from some schema up, and the model gives
  each upgrade that schema, its floor, which is at or below the schema the upgrade moves the layer
  to. An upgrade is safe for readers exactly when its floor is at or below the layer's `oldest`,
  the oldest reader schema the store still admits, because then every admissible reader schema,
  from `oldest` up, can read the result. The upgrade does not say whether it is safe, since "safe"
  means nothing without the `oldest` it is safe against, and only the store holds that; it states
  its floor and the store judges it.
- **An upgrade judged against the current schema is wrongly safe.** The schema an upgrade starts
  from can always read what it leaves, so judged against the current schema an upgrade whose floor
  is above `oldest` and at or below the current schema counts as safe for readers. It then drains
  no reader and leaves `oldest` where it was, and a reader of an older schema is left attached over
  data it cannot read, or is admitted over it afterwards (T22 below). Judged against `oldest` it is
  unsafe, which is the whole difference. A control that judges against the current schema violates
  the judgment itself and each of its consequences.
- **An upgrade that is unsafe for readers drains every reader, and sets `oldest` to its target.**
  It takes the reader tier exclusively, so it waits for every attached reader, of whatever schema,
  to leave before it begins; a reader whose schema is at or above the floor is drained too, since a
  lock cannot tell. When it completes it sets `oldest` for its layer to its target, as the
  admission rules state, and a reader below that is refused from then on. The floor judges the
  upgrade and says what the data requires; it is not what `oldest` becomes, so readers from the
  floor to the target are refused although they could read. That is the decision as it stands, and
  nothing is scheduled to change it.
- **The judgment is made under the gate and recorded.** It is made when the upgrader takes its
  first tier, to decide whether the reader tier is taken, and again when the upgrade is recorded;
  the schemas cannot change between the two, since the upgrader holds the gate and the start
  checks the layer's schema again. The intent records the judgment with the layer, the floor and
  the target, and a resumption by recovery completes the upgrade under the judgment it was begun
  with and judges nothing again. A judgment of safe where the record says unsafe leaves a reader
  attached that the rules no longer admit, which the admission properties detect.
- **Layers are independent.** The admission rules are per layer and an upgrade is of one layer:
  its intent names the layer, and its judgment, its effect on `oldest` and on the current schema,
  and the rules for an upgrade in progress are that layer's alone. One upgrade is in progress at a
  time whatever its layer, since an upgrade holds the gate exclusively, so upgrades of two layers
  take turns, as the witness `TwoLayersAreUpgradedInTurn` shows. While the first layer is upgraded
  from 1 to 2, with the second at 3 and `oldest` 1 in both, a handle is admitted if it speaks:

  | handle | in the layer being upgraded | in every other layer |
  |---|---|---|
  | reader | 1 or 2 (the target, if the upgrade is safe for readers); none if it is not | 1 to 3 |
  | writer | 2, the target, and only to resume the upgrade | 3, the current schema |

- **The data floor is the model's device, and the store need not record it.** To say that a reader
  cannot read the data, the model keeps for each layer the highest floor among the upgrades that
  have completed there, starting from the layer's first `oldest`. `oldest` is at or above it by
  construction: a safe upgrade's floor is at or below `oldest`, and an unsafe one sets `oldest` to
  its target, which is at or above its floor. Two properties use it: `oldest` is never below the
  data floor, and no attached handle speaks a schema below it.
- **What the model leaves out.** Which of an agent's schemas can read which data is one number for
  each upgrade, its floor; the model does not say how an agent works it out. A reader of the target
  schema reading data a migration has not reached, through the target's definition of the
  transitional state, is not modelled, and nor is an upgrade whose floor is above its target, which
  no schema could read. Whether an upgrade that waits for readers is eventually served is liveness,
  checked with the fairness the contract grants[^mp-4].
- **What it asks of an implementation, queued in the upgrade work[^mp-7].** An upgrade states the
  oldest reader schema that can read what it leaves, which is at or below its target. Canopy, not
  the upgrade, judges it against the layer's `oldest` and never against the current schema, records
  the judgment in the intent with the layer, the floor and the target, takes the reader tier
  exclusively when the upgrade is not safe so that every reader has left before it begins, and
  sets `oldest` to the target when it completes and leaves it alone when the upgrade is safe. The
  admission rules are applied to every layer of every attaching handle, and the rules of a layer
  that is not being upgraded are its ordinary ones.
- **Controls, witnesses and probes.** The controls reverse a rule and must fail: an upgrade judged
  against the current schema, which violates the judgment, the data floor and a reader's ability to
  read, in one layer, with a body, and in the second of two layers; an upgrade unsafe for readers
  that does not drain them, whose reader then cannot read the data; an upgrade that sets the oldest
  reader schema of every layer and not only its own, with and without a body; and an upgrade of
  one layer that changes what every layer admits, with and without a body. The witnesses show a
  reader of an older schema attached through a safe upgrade and a reader two schemas behind
  through two in a row, an upgrade that is unsafe against the oldest schema and safe against the
  current one, an upgrade of one layer beside handles of another, a writer admitted to resume an
  interrupted upgrade of one layer as an ordinary handle of the other, two layers upgraded in turn,
  and an unsafe upgrade of the second layer, each with and without a body where both can occur.
  Probes in scratch copies of the spec -- a judgment made with a strict comparison, an unsafe
  upgrade that leaves `oldest` alone, a completion that moves the first layer and not its own, with
  and without a body, an intent that records `oldest` as its floor, an admission rule that ignores
  `oldest`, and a rule for an upgrade in progress that is not specific to its layer -- were each
  caught by the intended property. Two of them first survived: both layers' upgrades moved to the
  same schema, so moving the wrong layer changed nothing, and the scenarios now start the layers at
  different schemas.

### T22 -- an upgrade judged against the current schema leaves a reader of an older schema attached over data it cannot read

Status: `[verified]` against the model, in the control that judges an upgrade against the current
schema, run in a scratch copy of the model whose only change is a guard that lets the reader attach
first, which removes behaviours and adds none. A speaks the layer's schema 1 and its upgrade is
readable by every schema; B speaks 2 and its upgrade leaves data that only schemas from 2 up can
read; C is a reader that speaks 1. The layer starts at schema 1, with 1 the oldest reader schema,
and nothing has attached.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `ATT1` | C | begins to attach | -- |
| 2 `GATE1` | C | passes the gate shared | -- |
| 3 `RTIR1` | C | takes the reader tier shared | -- |
| 4 `DROP1` | C | lets go of the gate | -- |
| 5 `ADMT1` | C | is checked against the rules in force, which admit a reader that speaks a schema from the oldest, 1, to the current, 1, and is admitted | -- |
| 6 `ATT2` | A | begins to attach | -- |
| 7 `GATE2` | A | passes the gate shared | -- |
| 8 `RTIR2` | A | takes the reader tier shared | -- |
| 9 `DROP2` | A | lets go of the gate | -- |
| 10 `WTIR1` | A | takes the writer tier shared | -- |
| 11 `ADMT2` | A | is checked against the rules in force, which admit a writer that speaks the current schema, and is admitted | -- |
| 12 `UPST1` | A | starts an upgrade, which first drains | -- |
| 13 `DRN1` | A | finds no operation of its handle in flight, so its drain ends and it lets go of its tiers | -- |
| 14 `GTX1` | A | takes the gate exclusively | -- |
| 15 `TRS1` | A | judges its upgrade against the oldest reader schema, 1: its floor is 1, so the upgrade is safe for readers and A takes only the writer tier exclusively, which C does not hold; C stays attached | -- |
| 16 `BGN1` | A | records the upgrade of the layer from 1 to 2 as in progress and safe for readers | intent: layer, 1 to 2, safe |
| 17 `END1` | A | completes it: the layer's schema becomes 2 and the oldest reader schema stays 1; A's handle ends | schema: 2; intent: none |
| 18 `ATT3` | B | begins to attach | -- |
| 19 `GATE3` | B | passes the gate shared | -- |
| 20 `RTIR3` | B | takes the reader tier shared | -- |
| 21 `DROP3` | B | lets go of the gate | -- |
| 22 `WTIR2` | B | takes the writer tier shared | -- |
| 23 `ADMT3` | B | is checked against the rules in force, which admit a writer that speaks the current schema, 2, and is admitted | -- |
| 24 `UPST2` | B | starts an upgrade, which first drains | -- |
| 25 `DRN2` | B | finds no operation of its handle in flight, so its drain ends and it lets go of its tiers | -- |
| 26 `GTX2` | B | takes the gate exclusively | -- |
| 27 `TRS2` | B | judges its upgrade, in the control against the current schema, 2: its floor is 2, so the upgrade is safe for readers and B takes only the writer tier exclusively, which C does not hold; C stays attached | -- |
| 28 `BGN2` | B | records the upgrade of the layer from 2 to 3 as in progress and safe for readers | intent: layer, 2 to 3, safe |
| 29 `END2` | B | completes it: the layer's schema becomes 3 and the oldest reader schema stays 1; B's handle ends, and the data can now be read only by schemas 2 and up, while C, attached, speaks 1 | schema: 3; intent: none |

Under the contract step 27 judges B's upgrade against the oldest reader schema, 1, and B's floor is
above it, so the upgrade is unsafe for readers: B takes the reader tier exclusively, which it
cannot while C holds it, and waits for C to leave. When it completes, the oldest reader schema
becomes 3, so a reader that speaks 1 is refused from then on. That outcome is `[projected]`, the
model's next steps from step 26, which the control does not reach.

### T23 -- a reader of the first source schema stays attached through two upgrades that are safe for readers

Status: `[verified]` against the model, in the witness for a reader two schemas behind, run in the
same scratch copy as T22 with the reader attaching first. A speaks the layer's schema 1 and B speaks
2, and the floor of each upgrade is 1; C is a reader that speaks 1. The layer starts at schema 1,
with 1 the oldest reader schema, and nothing has attached.

| step | actor | event | durable effect |
|---|---|---|---|
| 1 `ATT1` | C | begins to attach | -- |
| 2 `GATE1` | C | passes the gate shared | -- |
| 3 `RTIR1` | C | takes the reader tier shared | -- |
| 4 `DROP1` | C | lets go of the gate | -- |
| 5 `ADMT1` | C | is checked against the rules in force, which admit a reader that speaks a schema from the oldest, 1, to the current, 1, and is admitted | -- |
| 6 `ATT2` | A | begins to attach | -- |
| 7 `GATE2` | A | passes the gate shared | -- |
| 8 `RTIR2` | A | takes the reader tier shared | -- |
| 9 `DROP2` | A | lets go of the gate | -- |
| 10 `WTIR1` | A | takes the writer tier shared | -- |
| 11 `ADMT2` | A | is checked against the rules in force, which admit a writer that speaks the current schema, and is admitted | -- |
| 12 `UPST1` | A | starts an upgrade, which first drains | -- |
| 13 `DRN1` | A | finds no operation of its handle in flight, so its drain ends and it lets go of its tiers | -- |
| 14 `GTX1` | A | takes the gate exclusively | -- |
| 15 `TRS1` | A | judges its upgrade against the oldest reader schema, 1: its floor is 1, so the upgrade is safe for readers and A takes only the writer tier exclusively, which C does not hold; C stays attached | -- |
| 16 `BGN1` | A | records the upgrade of the layer from 1 to 2 as in progress and safe for readers | intent: layer, 1 to 2, safe |
| 17 `END1` | A | completes it: the layer's schema becomes 2 and the oldest reader schema stays 1; A's handle ends | schema: 2; intent: none |
| 18 `ATT3` | B | begins to attach | -- |
| 19 `GATE3` | B | passes the gate shared | -- |
| 20 `RTIR3` | B | takes the reader tier shared | -- |
| 21 `DROP3` | B | lets go of the gate | -- |
| 22 `WTIR2` | B | takes the writer tier shared | -- |
| 23 `ADMT3` | B | is checked against the rules in force, which admit a writer that speaks the current schema, 2, and is admitted | -- |
| 24 `UPST2` | B | starts an upgrade, which first drains | -- |
| 25 `DRN2` | B | finds no operation of its handle in flight, so its drain ends and it lets go of its tiers | -- |
| 26 `GTX2` | B | takes the gate exclusively | -- |
| 27 `TRS2` | B | judges its upgrade against the oldest reader schema, 1: its floor is 1, so the upgrade is safe for readers and B takes only the writer tier exclusively, which C does not hold; C stays attached | -- |
| 28 `BGN2` | B | records the upgrade of the layer from 2 to 3 as in progress and safe for readers | intent: layer, 2 to 3, safe |
| 29 `END2` | B | completes it: the layer's schema becomes 3 and the oldest reader schema stays 1; B's handle ends, and C, still attached, speaks 1, two schemas behind, and can read what both upgrades left | schema: 3; intent: none |

## The model clients rely on, and the model the code is held to, 2026-11-11

- **There are two models: the one the code is held to and the one clients rely on.** The code's
  model is the one in `spec/`. It carries the mechanism -- the count, the fence, the generation,
  the commit log, the lock order, the registry -- because conformance replays it against the
  code and compares state after every step. The client model[^cm-2] carries only what a `Store`
  user can observe: the logical contents, which handles are attached and in what role, each
  request's outcome, and whether the store needs recovery. It hides mechanism and not outcomes: a
  write whose holder was told recovery is required may or may not have taken effect, a read may be
  rejected as disturbed, and an upgrade may drain or refuse readers. Which of these canopy
  promises is decided separately[^odi-5].
- **The abstract write and the one-step upgrade are not the client model.** They are coarser
  steps over the same internal state -- the abstract write still has the count, the fence and the
  ownership record -- so they are a cheaper variant of the code's model and say nothing a user
  sees. A client model has fewer variables, not coarser steps.
- **The client model is above the code's model, and the relation between them is checked.** TLC
  checks that every behaviour of the code's model, projected onto the observable variables, is a
  behaviour of the client model[^cm-3]. The projection is where the commit point is written
  down: when a write counts as having happened, including when its holder was told it failed. The
  code is then held to the client model in two steps, the code to its model and its model to the
  client's, and the client model is the oracle for tests at the API that pass through any change of
  mechanism the refinement still admits[^fm-3-4].
- **A dependent binds to the promise and to nothing else.** The client model is what a user reasons
  with: what is promised, what is not, and what to do on each outcome[^cm-4]. A dependent that
  relies on what happens to hold today has bound to something canopy never promised, so what is
  not promised is stated as plainly as what is.
- **What is promised is decided before it is modelled.** A promise is a commitment a dependent can
  feel, so the engineer decides it[^odi-5], starting from the properties classified by
  audience[^cm-1], and the client model promises no more, which leaves the implementation the
  freedom the promise leaves. The decision on fairness[^odi-2] settles what is promised about
  waiting. The refinement waits until the work on the durable publication record[^mp-3] and on
  composing the models[^mp-4] has settled the state its projection is written against.
- **Both forms of a write and of an upgrade stay in the code's model, for now.** The abstract write
  and the one-step upgrade keep the configurations that do not exercise a transaction's phases or
  an upgrade's body small, and the detailed forms follow the code more closely. Keeping both costs
  two forms to hold in step. The decision rests on the mechanisms that keep the cost of checking
  proportionate -- the targeted check of each work item, the tiers, and runs scoped to what
  changed[^spec-2] -- and stands only while they are acted on. Merging the forms, by defining the
  abstract form as a coarsening of the detailed one and having TLC check it, and retiring the
  abstract forms both remain open. The decision schedules no work of its own[^odi-3] and sets no
  trigger for revisiting it: poor work-item throughput is what prompts inspecting the impediments
  again.

The discussion is recorded in
[DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md](design-sessions/DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md).

[^cm-1]: `CM-1` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#cm-1) [completed 2026-11-11]
[^odi-5]: `ODI-5` in [CHECKLIST.md](CHECKLIST.md)
[^cm-2]: `CM-2` in [CHECKLIST.md](CHECKLIST.md)
[^cm-3]: `CM-3` in [CHECKLIST.md](CHECKLIST.md)
[^cm-4]: `CM-4` in [CHECKLIST.md](CHECKLIST.md)
[^fm-3-4]: `FM-3.4` in [CHECKLIST.md](CHECKLIST.md)
[^mp-3]: `MP-3` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-3) [completed 2026-11-11]
[^spec-2]: `SPEC-2` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#spec-2) [completed 2026-11-12]

## Probe mutants are data in the repository, matched so that they survive edits to the spec, 2026-11-11

- **A probe's mutants are kept as data beside the spec they probe.** `spec/probes/<Name>/` holds
  one `.psd1` file per subject, each a list of mutants: a name for the rule it breaks, the
  configuration it runs in, the one property written for the rule, and a text edit.
  [run-probes.ps1](../../tools/run-probes.ps1) applies the edit to a scratch copy of the spec, runs
  TLC in that configuration restricted to the property, and the mutant is caught when TLC reports
  the property violated. The corpus is the record that each property can fail, and it is a record
  only if the next contributor finds it in the checkout and can rerun it; a mutant kept as a
  script of its own, or in scratch, is neither[^spec-1].
- **A mutant stays current by what its text is matched against.** Mutants are used at milestone
  boundaries, so the spec changes under them for the whole milestone and they must still apply
  when the boundary arrives. A mutant's `Old` text must occur exactly once in the spec, or in the
  body of the definition the mutant names, and the matching ignores what an `UNCHANGED` tuple
  lists: a frame names every variable an action leaves alone, so a variable added to the spec
  changes nearly every frame and would make nearly every mutant stale. Where `Old` and `New`
  differ in a tuple, the variables `New` drops are removed from the spec's own tuple and the ones
  it adds are put in it, and a place whose tuple lacks a variable `New` drops is not an
  occurrence, so an edit of a frame alone is found by the variable it changes and by its
  definition.
- **A mutant that no longer matches is stale, and stale is reported and fails the run; it is never
  skipped.** A skipped mutant is a property nothing is checking, with no sign that nothing is. The
  runner finds every stale mutant before it runs TLC and says where its first line is now, so that
  re-expressing it, or retiring it on purpose, is a matter of reading the edit against the
  current text. That repair is the first step of a boundary's probe work: the staleness a
  milestone causes is paid for once, at the point where its diff says what the rules now are, and
  nothing in the work-item loop reads or edits the corpus.
- **An edit that changes one component of a derived tuple names the others, and breaks loudly.**
  A mutant that changes `count` while `coord` also holds the owner and the fence must leave those
  unchanged by name, so when the tuple gains a component the mutated action no longer defines it.
  TLC then reports the variable undefined and the run is an error, never a quiet pass; it is
  repaired as a stale mutant is. A mutant that spans more than one place in a definition lists its
  edits and each is matched on its own, so that a rule broken in two places is not forced into one
  long quotation that a change anywhere between them makes stale.
- **A mutant that was moved or repaired is trusted only after it has run.** A text match says where
  an edit lands and not that it lands at the rule the mutant is named for: a frame edit can match
  the one action that now has its shape, and an edit can match a prefix of a construct that has
  grown. The run is the check -- the mutant comes out caught by the property written for its rule,
  or recorded as equivalent -- so a boundary's probe work ends with a run of what it repaired.
- **A rule nothing checks stays on record.** A mutant that says it is expected to pass is the
  record of a rule with no property; the run reports it as equivalent while TLC finds no error and
  as unexpected when it does, so the first check written for the rule shows at the next boundary.
- **The configurations the mutants run in are the generated ones.** A mutant names a base
  configuration by name and sets only the constants it overrides, so a configuration that moves
  with its table moves the mutants that run in it; a mutant whose base is gone is reported as an
  error and not skipped.

The discussion is recorded in
[DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md](design-sessions/DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md).

[^spec-1]: `SPEC-1` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#spec-1) [completed 2026-11-11]

## Who a property is for, 2026-11-11

- **Every property a configuration checks says whether it is for a client or for the code.** The
  first line of its header comment is `Audience: client.` or `Audience: internal.`, and
  [check-audience-tags.ps1](../../tools/check-audience-tags.ps1) fails a spec in which a property
  named by a configuration has no tag, so the properties that later work writes[^mp-3][^mp-4] are
  tagged as they are written[^cm-1]. The `Witness_` definitions are evidence that a state is
  reachable and not guarantees, and carry none.
- **A property is client-visible when a `Store` user can see its violation as an outcome of the
  API.** Its tag goes on to state the guarantee in the words a user would use and to name the
  outcome it shows through: the result of a write, a read, an attach, an upgrade, a create or an
  open, a listing or discovery. A property whose violation reaches a user only through another
  property that is itself client-visible is internal. The caches, the count and the commit log are
  mechanism, and what a user sees of them is carried by the classification of a read and by the
  outcome that tells a writer recovery is required.
- **The client-visible properties are the first draft of what canopy promises, and a draft only.**
  Which of them are promised is decided separately[^odi-5]; one that is not promised stays in the
  model as the code's own check. They also say what the next refinements must keep: of the durable
  publication record[^mp-3], that the version a document is published at has its content after a
  crash and that a read is never told to wait for a writer that does not exist; of composition[^mp-4],
  writer exclusion, the classification of a read, and the outcomes that tell a writer recovery is
  required.
- **Some outcomes cannot be interpreted without help**, and the guide for dependents teaches a
  caller to act on them[^cm-4]:
  - A write whose holder was told recovery is required may or may not have taken effect: the
    failure came after the first change and before the closing publication, or the transaction was
    prepared and recovery rolls it forward, or it was not and recovery rolls it back. The caller
    learns which by reading once recovery has run, and not from the error.
  - A read may be rejected as disturbed: an operation began or ended during it, and the read says
    nothing about the data except that it was not settled.
  - An upgrade may drain, which waits for the operations in flight on the handle and its clones,
    or be refused: from inside an operation, from a schema that has moved, or for a body that did
    not complete, which leaves the handle detached. An upgrade unsafe for readers stops older
    readers being admitted, so a handle that attached before may be refused after.
  - A discovery's report and a listing's `incomplete` describe a moment that may have passed.
  - A create or open that stops says why -- another implementation's reservation, other inputs, an
    occupied root, a race lost, the store already there -- and one that resumes has finished a
    creation that a dead creator began.

## The publication record's live and durable state, modelled in `PointerPublication.tla`, 2026-11-11

- **The record is two states, what a process reads in its file and what a crash leaves, behind
  the `RecordDetail` switch.** Writing it is an append followed by an fsync, and clearing it a
  truncation followed by an fsync, and either fsync can fail. A step that fails ends the
  operation, leaving the file as it was at that step. A crash reverts the file to what was
  synced. A torn append, once detected, is the same as a lost one, so it needs no state of its
  own. With the switch off the record is written and cleared in one step, and every earlier
  configuration explores the states it did[^mp-3-2].
- **A settle makes the record durable before it acts on it.** A write whose record fsync failed
  leaves the record in the file and not durable. If the next settle, by a write or by a reopen,
  rolls the replicas forward from it, the renames become durable while the record does not, and
  a crash that follows leaves replicas that name two values with nothing to finish them. The
  next publication to fail part-way then leaves three and strands the vote (T24). So a settle
  fsyncs the record before its first rename, and a settle whose fsync fails fails its operation
  with the record still pending. The model checks that a pointer replica is renamed only once
  the record naming its version is durable, whether by a write or by a settle, and the control
  that omits the fsync fails it.
- **A failed clear leaves a stale durable record, which an unlogged write replaces and a logged
  write does not.** The unlogged write that follows rewrites the record, and syncs it, before it
  moves a pointer, so the stale one is replaced before it could be replayed. A write a
  transaction makes moves pointers and writes no record. If it follows the failed clear it moves
  them to `B`, ends, and collects what it superseded, and a crash then replays the stale record,
  which names `A`, over the pointers: they name content that no longer exists (T25).
- **A settle looks at the durable record and not only at the file.** Only the durable record
  shows a failed clear, for the file is empty. So the settle that begins every operation, a
  write of either kind and a reopen, fsyncs the record file first whether or not the file shows a
  record, rolls forward what it then finds, and clears. The control that fsyncs only when the
  file shows a record fails `DurablePointerHasBlob` and `LivePointerHasBlob`[^mp-3-3].
- **A transaction's write is modelled with its own recovery, and both guards hold beside it.**
  It is the prepare record, the pointer swaps, the end record and the collection of what it
  superseded, and a prepared transaction is rolled forward by recovery, which settles the
  record first and collects after. It is mixed with unlogged writes on one document. Neither
  guard fails `DurablePointerHasBlob`, `LivePointerHasBlob` or `IdleImpliesSettled` in the mix,
  so the record guard's holding is no longer provisional for it.
- **A logged write needs the record guard's settle and does not need resolving.** A logged
  write that skips the record guard fails the two blob invariants, by the same sequence as T25.
  One that skips resolving fails nothing, because its own recovery sets every replica to the
  version its prepare record names. That is what the model shows, and it is not a ruling that the
  code may skip the guard: the model has one process, and composition[^mp-4] is where the claim
  is tested with several.
- **What it asks of an implementation, queued in the implementation of the chosen guard[^fm-2-2].**
  Every operation that begins by settling fsyncs the record file, then rolls forward, then
  clears, and so pays one fsync whether or not a record is found. That puts the record guard's
  cost near resolving's. A form that remembers in the process that a clear failed, and fsyncs only
  then and at open, is cheaper, and holds only while one process writes the record, which the
  model of one process cannot show.
- **Every operation on the document settles first, and the model shows which ones need it.** The
  precondition check, the standalone collection pass, the unlogged write, the logged write and
  recovery each begin by settling under the record guard, and a control that omits the settle
  fails for each of them except repair and scrub[^mp-3-4]:
  - A check that skips it reads a document with a record pending that will replace the value it
    reads, which fails `ACheckReadsASettledRecord`. A compare-and-set that trusted the read would
    then succeed against a value a roll-forward is about to replace.
  - A collection pass that skips it keeps the live majority and removes the version the pending
    record names, and the settle that follows names content that no longer exists. It fails
    `DurablePointerHasBlob`.
  - A write that skips it fails `IdleImpliesSettled`, and a logged write that skips it fails the
    blob invariants (T25).
- **A repair and a scrub do not need the settle for the properties the model checks.** Each reads
  the vote again and writes its version to a replica that has none, so a record pending beside it
  is rolled forward over a replica that already agrees, and no invariant fails when the settle is
  omitted. Whether the implementation settles before them anyway, at one fsync per document a
  scrub touches, is a decision for FM-2.2[^fm-2-2] with this finding in hand. Blob repair is not
  modelled: it adds a copy of the version the vote names, which no property observes.
- **A lost pointer replica separates the guards, and no safety property.** The loss is modelled
  as the first fault, of a document that is whole and settled. Under the resolve guard the next
  write sets every replica to the majority and so restores the lost one, and every invariant
  holds. Under the record guard a write does not restore it, a publication that then fails
  part-way leaves three values and no majority until the next settle (T26), and a read in between
  is told to wait for a writer that does not exist. `IdleImpliesSettledUnlessAReplicaIsLost` is
  the form of the property the record guard keeps with a replica lost, and only a repair restores
  the replica. That is a consequence of two faults, not of the guard failing its own contract, and
  it is evidence for FM-2.2[^fm-2-2] that the resolve guard absorbs a fault the record guard
  leaves to repair.
- **A restart is modelled as recovery, and a clean open is not distinguished from it.** The model
  has one document, and reopening runs the recovery that settles the record and finishes a
  transaction. Whether a store opens without settling any record, leaving each to the first
  operation on its document, is a question about many documents and is carried into the
  composition[^mp-4].

### T24 -- a settle that rolls forward from a record that is not yet durable, a crash, and then a failed write strand the vote

Status: `[verified]`, the shortest trace of `PointerPublication-Control-SettleWithoutSyncStrandsTheVote.cfg`.
The pointer replicas are written `[1, 2, 3]`, and all three hold `A` at the start.

| Step | Actor | Event | Durable effect |
|---|---|---|---|
| 1 `ING` | writer 1 | ingests the blob for `B`, renaming and syncing it | the blob for `B` |
| 2 `REC` | writer 1 | appends a record naming `B` to the record file | -- |
| 3 `RSF` | writer 1 | the fsync of the record fails, and the write ends with an error | -- |
| 4 `OPN` | process | the store is closed and reopened, and the reopen finds the record in the file and begins to settle | -- |
| 5 `RFA` | settle | renames replica 1 to `B` and syncs its parent | replica 1 is `B` |
| 6 `RFB` | settle | renames replica 2 to `B` and syncs its parent | replica 2 is `B` |
| 7 `PWR` | EXTERNAL | power is lost before replica 3 | the record was never durable and is gone; the replicas are `B, B, A` |
| 8 `OP2` | process | reopens, finds no record, and has nothing to settle | -- |
| 9 `WR2` | writer 2 | ingests the blob for `C`, writes its record and syncs it | the record names `C` |
| 10 `PRN` | writer 2 | renames replica 1 to `C` | -- |
| 11 `FAI` | writer 2 | fails | the live replicas are `C, B, A`, no value has a majority, and nothing is running |

The settle at step 4 is the one the corrected guard prevents: it fsyncs the record first, and
step 3's failure then repeats and fails the reopen with the record pending.

### T25 -- a logged write follows a failed clear, collects the version the stale record names, and a crash replays the record

Status: `[verified]`, the shortest trace of
`PointerPublication-Control-SettleLooksOnlyAtTheFile-LivePointerHasBlob.cfg`, which runs the
record guard with a settle that fsyncs only when the file shows a record. The pointer replicas are
written `[1, 2, 3]`, and all three hold `A` at the start.

| Step | Actor | Event | Durable effect |
|---|---|---|---|
| 1 `WR1` | writer 1 | ingests the blob, then writes the record naming `A` and syncs it | the record names `A` |
| 2 `SW1` | writer 1 | swaps and syncs each replica, which stay `A` | -- |
| 3 `CLR` | writer 1 | truncates the record file | -- |
| 4 `CLF` | writer 1 | the fsync of the truncation fails, and the write ends with an error | the file is empty and the durable record still names `A` |
| 5 `TX` | transaction | begins; the settle looks at the empty file and finds nothing | -- |
| 6 `ING` | transaction | ingests the blob for `B`, renaming and syncing it | the blob for `B` |
| 7 `PRP` | transaction | appends its prepare record to the log | the prepare record |
| 8 `SW2` | transaction | swaps and syncs replicas 1, 2 and 3 to `B` | the replicas are `B, B, B` |
| 9 `END` | transaction | appends its end record | the end record |
| 10 `COL` | transaction | removes the blob for `A` and syncs | the blob for `A` is gone |
| 11 `PWR` | EXTERNAL | power is lost | the file reverts to the durable record, which names `A` |
| 12 `OPN` | process | reopens, and settles the record naming `A` | -- |
| 13 `RFA` | settle | renames replica 1 to `A` | replica 1 names `A`, whose content step 10 removed |

The corrected settle fsyncs at step 5, finds the stale record, and rolls it forward, which does
no harm because the pointers still name `A`, and then clears it durably before the transaction
moves anything.

### T26 -- a lost pointer replica and a write that fails part-way under the record guard leave no majority

Status: `[verified]`, the shortest trace of `PointerPublication-MaintenanceRecordLostReplica.cfg`.
The pointer replicas are written `[1, 2, 3]`, and all three hold `A`, with no record pending.

| Step | Actor | Event | Durable effect |
|---|---|---|---|
| 1 `LOS` | EXTERNAL | the pointer entry on replica 2 is lost | the replicas are `A, none, A` |
| 2 `WRT` | writer | begins; the settle finds no record | -- |
| 3 `ING` | writer | ingests the blob for `B` | the blob for `B` |
| 4 `REC` | writer | appends a record naming `B` and syncs it | the record names `B` |
| 5 `PRN` | writer | renames replica 1 to `B` | -- |
| 6 `FAI` | writer | fails before syncing it | the live replicas are `B, none, A`, no value has a majority, and nothing is running |

Under the resolve guard step 2 reads the majority `A` and sets every replica to it, so replica 2
is restored before the publication starts and the same failure leaves `B, A, A`.

[^mp-3-2]: `MP-3.2` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-3-2) [completed 2026-11-11]
[^mp-3-3]: `MP-3.3` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-3-3) [completed 2026-11-11]
[^mp-3-4]: `MP-3.4` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-3-4) [completed 2026-11-11]
[^fm-2-2]: `FM-2.2` in [CHECKLIST.md](CHECKLIST.md)

## Composing the publication model with the coordination contract, 2026-11-11

- **The composition is a model of its own, [SharedPublication.tla](spec/SharedPublication.tla),
  which extends [Coordination.tla](spec/Coordination.tla) and adds the documents.** Every step is
  one of the contract's actions conjoined with what it does to the documents, or a step on the
  documents alone that leaves every variable of the contract as it was, so every behaviour of the
  composition is a behaviour of the contract with documents added. It refines the contract by
  construction: nothing of the contract is restated, and there is no mapping to keep in step with
  it. `Coordination`'s own configurations and state space are unchanged, which the decision that
  composes the two models only for this check requires ("Writer exclusion and its ownership
  record, modelled in `Coordination.tla`"). The composed configurations name a scenario of writers
  with no registry, upgrade, clones, reads or discovery, and leave those actions of the contract
  out, so the variables they would change keep their initial values.
- **Documents exist only in the composed model.** [PointerPublication.tla](spec/PointerPublication.tla)
  stays the model of one document's publication, with one process and an open that recovers. Its
  variables are one document's, so the composition restates its publication over a `Docs` constant,
  and nothing checks the restatement against it: a change to the publication rules is made in both
  by hand. The configurations use two documents, and three in one configuration of the milestone
  tier. At least two are needed, because what recovery is asked is whether durable state tells it
  which document holds the work.
- **The publication rides on `Coordination`'s two cells.** The step that decides a write's
  outcome is the contract's first effect: the record's append under the record guard, since a
  settle rolls a pending record forward, and otherwise the first pointer rename. The step that
  completes its durable work is the last effect: the clear's fsync under the record guard,
  otherwise the last pointer's. A failure after the first effect fails unresolved, sets the fence
  and leaves the count odd, as the contract says of any operation; one before it ends without an
  effect. Collection runs after the last effect, and a failed collection only leaves garbage.
- **Each write may choose its form, behind `Coordination`'s `MixedWrites` switch.** A write begins
  a transaction or makes its first effect directly, and while it is between its effects the log
  shows which, since only a transaction leaves its prepare record as the log's latest; no
  variable records the choice. Recovery finishes either kind: a prepared transaction from the
  log, and an unlogged write by making the store whole. Both guards hold every invariant and
  property with the two forms mixed across processes, and recovery rolls both kinds forward in
  one behaviour[^mp-4-5].
- **`Coordination` had no recovery for an unlogged write, and it gains one behind a switch.**
  `RecStart` required `Txn`, so a write that failed unresolved left the fence set for good. The
  contract requires recovery there -- the guard's record is unfinished work -- so this was a gap
  in `Coordination`, and `UnloggedRecovery` makes a store half applied whole, publishes the count
  and then clears the fence[^mp-4-2].
- **The guards' work at the start of a write has nothing to do under the contract.** The fence
  survives a holder's death and a host restart forces recovery, so every acquirer after an
  unresolved failure, a death or a restart is told recovery is required, and only recovery may
  act. The composition checks that a holder who is not told finds every document with no record
  pending, its replicas unanimous and its live state durable, under both guards and both forms of
  write (`AHealthyHolderFindsEveryDocumentSettled`). The record guard's settle before a write,
  with the fsync of the record file that a settle needs, and the resolve guard's resolve before a
  write, with a parent fsync per replica, are therefore work the contract makes redundant. What
  each guard asks of a write is only its own publication: under the record guard the record's
  append and clear, each with its fsync, and under the resolve guard nothing beyond the renames.
  This rests on the fence being durable and on a restart forcing recovery. The model of one
  process has no fence, so its next write follows a failed one directly, which is why
  `PointerPublication` needs the settle[^mp-4-3].
- **Recovery does each guard's work on every document.** No durable state names the document an
  unlogged write was publishing. Under the record guard recovery syncs each record file -- so a
  failed clear is seen -- rolls forward what it names and clears it. Under the resolve guard it
  sets every replica of every document to its majority and syncs it: a write that renamed replicas
  without syncing them leaves the live and durable majorities different, and a crash after
  recovery would then change what the document reads as (T27). The cost of the resolve guard's
  recovery is a parent fsync for every replica of every document in the store, against the record
  guard's work on the records it finds. With no guard, recovery cannot restore a document, and a
  write that fails part-way over one that did leaves no majority.
- **Opening changes nothing.** Only a holder of writer exclusion changes a document, and a crash
  only loses what was not durable; attaching a handle, the open of a store, settles no record. A
  control in which attaching settles every record fails the property that says so.
- **Every condition that sets the fence leaves durable state from which recovery does the work,
  so a restart that loses the fence loses nothing it knew.**
  - A holder that dies leaves the ownership record. The fence that the next acquirer sets from it
    keeps every later acquirer told after a told holder releases without recovering; replacing
    the record without the fence lets a third acquirer find a document unsettled.
  - A restart leaves the coordination state untrusted, which tells every acquirer whether or not
    the fence survived; an acquirer that ignores the restart finds a record pending.
  - A transaction that fails after its prepare record, or whose prepare's append fails
    ambiguously, leaves the prepare record, from which recovery rolls it forward; rolling it back
    leaves a document partly swapped.
  - An unlogged write that fails unresolved leaves the record under the record guard and nothing
    under the resolve guard, which is why the resolve guard's recovery visits every document.
  - A failed closing publication leaves the count odd and no work on any document.

  The controls that make an acquirer ignore the first two fail under both guards, and the one
  that rolls a prepared transaction back fails under the record guard[^mp-4-4].
- **The in-process fence of `PointerPublication` is the contract's fence without a guard.** The
  contract's fence survives a death, and trust covers its loss in a restart, which is what the
  in-process fence lacked. A fence alone says that work is unfinished and not where it is, so with
  no guard recovery cannot restore a document, and the composition fails as the in-process fence
  did.
- **The probe tool copies what a chain depends on.** `tools/run-probes.ps1` copies only a spec's
  own chain into its scratch directory, which cannot parse a model that extends another spec, so
  the composition's probes need it to copy the modules the chain depends on as well[^mp-4-4].
  The module's name keeps clear of another spec's, because the tools gather a spec's modules as
  `<Name>*.tla` and its configurations as `<Name>*.cfg`.

### T27 -- the resolve guard's recovery leaves a document whose live and durable majorities differ

Status: `[verified]`, the shortest trace of
`SharedPublication-Control-RecoveryLeavesTheDocumentsResolve.cfg`, in which recovery does no work
on the documents. The pointer replicas of document 1 are written `[1, 2, 3]`, and all three hold
version `0`.

| Step | Actor | Event | Durable effect |
|---|---|---|---|
| 1 `ATT` | process 1 | attaches, takes writer exclusion, records itself and publishes the opening raise | the ownership record names 1; the count is odd |
| 2 `ING` | process 1 | ingests the blob for version `1` of document 1 | the blob for `1` |
| 3 `PR1` | process 1 | renames replica 1 to `1` and syncs its parent | replica 1 is `1` |
| 4 `PR2` | process 1 | renames replica 2 to `1` | -- |
| 5 `FAI` | process 1 | its sync fails; the write fails unresolved and sets the fence | the fence is set |
| 6 `REC` | process 1 | runs recovery: the store is made whole at the contract's level, the count published and the fence cleared, and no document is visited | the count is even; the fence is clear |
| 7 `REL` | process 1 | releases exclusion | the ownership record is cleared |

The live replicas are `1, 1, 0` and read as `1`; the durable ones are `1, 0, 0`, so a crash would
make the document read as `0` with no write in between. The resolve guard's recovery visits every
document because nothing durable says that document 1 was the one being published.

## What the contract's lack of a fairness requirement does, modelled in `Coordination.tla`, 2026-11-11

> **SUPERSEDED in part, 2026-11-12, by "Every hold ends, checked in the coordination model".**
> The cyclic workload, the `Cyclic` switch and every configuration named below that runs on it
> are retired; the findings about who is served are carried by the acquisition model, which the
> coordination model refines. Read that decision before acting on how liveness is checked here.

The contract promises no fairness between waiters: writer exclusion, the admission gate and the
tiers grant to any waiter, in any order. `CoordinationLiveness.tla` checks what that leaves --
which progress holds without any ordering, which requests can be passed over forever, and what
fairness on each acquisition would buy -- as the evidence ODI-2 and CQ-1 take as input.

- **Liveness is checked on a cyclic workload, and only there.** A bounded configuration's
  behaviours all end, so nothing can be passed over forever in it. Behind `Cyclic`, a process that
  exits may attach again, requests, reads, discoveries and listings are not counted, and the count,
  the generation and the versions wrap within `1..CycleMod`. Deaths, crashes and upgrades stay
  bounded: each is monotone, so every infinite behaviour ends in a cycle without them. Two values
  compared across a wrap can be equal when they should not be, so a cyclic configuration checks one
  temporal property and no invariant, and the bounded configurations keep checking safety. Every
  existing configuration explores the states it did with the switch off. Two witnesses show the
  workload goes on: one with a write in a cycle and one with an admission in a cycle[^mp-4-6].
- **`ContractSpec` is the fairness the contract guarantees or assumes, and no more.** Every holder
  eventually releases and every handle eventually detaches; every I/O eventually completes; and
  for each synchronization resource -- writer exclusion, the gate shared, the gate exclusive, the
  reader tier, the writer tier -- weak fairness on each process's acquisition. A lock call whose
  lock stays grantable to it returns: that is an I/O completing, which the contract assumes, not
  an order among waiters. Waiters stay unordered. One whose take is grantable only now and then,
  because another waiter wins each time the resource frees, can be passed over forever, and that
  is the contention the contract allows.
- **Fairness on the resource alone is too weak to be the contract.** With weak fairness only on
  the disjunction of a resource's acquisitions by every process, a handle holding the gate shared
  can wait for the reader tier forever while nothing holds that tier exclusively -- its take is
  grantable in every state -- because another handle's repeated takes satisfy the resource's
  fairness. No lock behaves so: a shared request on a lock held only shared is granted. That
  starvation would say nothing about contention, so the contract's fairness is per process.
- **Release and I/O completion need strong fairness, not weak.** A holder's own choices interrupt
  them: an operation nested under the hold disables the outer operation's next step until it
  returns, and a holder can nest again and again, so the outer step is enabled only infinitely
  often and never continuously. Weak fairness lets such a holder keep the lock forever, which is
  not the assumption the contract states -- that every operation eventually ends. The same holds
  for a handle's exit, which a new request interrupts. An operation's last effect has its own
  strong fairness for the same reason[^mp-4-6].
- **The store as a whole makes progress under `ContractSpec`.** While anyone waits for a resource,
  some resource is acquired again and again -- with two writers, and with an upgrader unsafe for
  readers beside a reader. An upgrade that holds every tier it needs completes or finds the schema
  moved, and a recovery under way ends. Each fairness condition dropped on its own makes the store
  stop, which shows each one is needed: a holder that never releases, an I/O that never
  completes, or a free resource that no waiter is given each leaves a waiter forever. These are
  the controls `Coordination-Control-Live-Without*`.
- **The slow liveness checks are evidence, run on demand.** A liveness check over the cyclic
  graph takes a quarter of an hour, and under strong fairness hours, because TLC expands every
  fairness condition and checks each of the graph's cycles against them. Those configurations --
  the progress properties, the per-request properties that take long, and every `FairSpec` and
  `UpgraderFairSpec` result that holds -- are in the `evidence` tier: the sweep leaves them out,
  and they are run when a change touches what their findings rest on, the liveness module or the
  steps that take and release writer exclusion, the gate and the tiers. The controls, the
  witnesses and the counterexamples that come quickly stay in the item tier and guard the
  formulation on every run.
- **The negative control in which an upgrader keeps its shared tiers while it waits for the gate
  exclusively deadlocks in the cyclic workload too**, so the rule that an upgrader gives up its
  tiers before it waits for the gate is still what keeps two upgraders from waiting on each other
  when they go on attaching.
- **Under `ContractSpec` a writer and an upgrader can each be passed over forever, and an
  attaching handle cannot.** A writer waiting for exclusion is passed over by another writer that
  is granted it each time it frees (T28). An upgrader waiting for the gate exclusively is passed
  over by a handle that takes the gate shared each time it frees (T29). In both, the waiter's take
  is grantable only between the other's holds, which the contract's fairness does not cover. A
  handle waiting to attach is admitted or refused beside a second writer that goes on attaching,
  and beside an upgrader unsafe for readers: a handle's takes are shared, and only an upgrade holds
  the gate or a tier exclusively, so its takes stay grantable apart from an upgrade's holds, which
  end. The checklist expected all three to fail; the attaching handle's does not, which removes it
  from the points of conflict below as a starvation, though not as a wait[^mp-4-7].
- **The points of conflict -- which waits contend with which holds -- are these**, as the input to
  CQ-1 and ODI-2:
  - *Writer exclusion*: a writer's wait against every other writer's holds, each an operation
    long, nested operations included. Any writer can be passed over forever (T28), and with it
    anything that needs exclusion: a recovery is driven only by a writer that gets exclusion, so a
    recovery that is never driven is a writer that is never served.
  - *The admission gate, exclusive against shared*: an upgrader's wait for the gate against every
    attaching handle's shared hold, each as long as the attach takes to pass the reader tier. One
    handle that goes on attaching can pass an upgrader over forever (T29); two whose holds overlap
    keep the gate held without a gap, so that no fairness on the upgrader's own take can serve it
    (T30).
  - *The tiers, exclusive against shared*: an upgrader holding the gate waits for every attached
    handle to detach. No new handle can attach past the gate it holds, so the wait ends when the
    longest attachment does -- a wait as long as the handles choose, but not a starvation.
  - *The gate and the tiers, shared against exclusive*: a handle that waits to attach waits out an
    upgrade's exclusive holds. Upgrades end, so this too is a wait and not a starvation.
- **Strong fairness on each acquisition serves the writer and the upgrader beside one handle,
  and not an upgrader beside two.** `FairSpec` makes every acquisition strongly fair. Under it an
  upgrader beside one reader is served, since the gate frees between the reader's attaches and a
  take enabled again and again is made, and a waiting writer is served, which the acquisition
  model shows with three writers. TLC cannot carry `FairSpec`'s formula for three
  processes, so the upgrader beside two readers is checked under `UpgraderFairSpec`, which keeps
  only the fairness its service depends on -- every handle exits, every I/O completes, a shared
  take that stays grantable is made, and the upgrader's own takes are strongly fair -- and which
  `FairSpec` implies. It fails (T30), and the failure stands under `FairSpec` too: in T30's cycle
  every acquisition that is enabled is made, and the upgrader's take of the gate is never
  enabled, so strong fairness on it asks nothing[^mp-4-8].
- **Withholding shared admission while an upgrader waits serves it.** Under
  `UpgraderFairSpecExclusiveFirst`, which is `UpgraderFairSpec` with no handle passing the gate
  shared while an upgrader waits for it exclusively, the upgrader beside two readers is served:
  the holders drop the gate, it frees, and stays free until the upgrader takes it. This is an
  option for ODI-2, not a rule of the contract. `FairSpec` and the exclusive-first variant prove
  eventual service and nothing about how often a waiter is passed over first.

### T28 -- a writer is passed over forever by another writer under the contract's fairness

Status: `[verified]`, the shortest trace of `Coordination-Live-AWaitingWriterIsServed.cfg`, two
writers on the cyclic workload. Steps 13 to 26 repeat forever; the cycle holds two writes because
the count and the generation wrap.

| Step | Actor | Event | Durable effect |
|---|---|---|---|
| 1 `ATT1` | process 1 | passes the gate shared, takes the reader tier, drops the gate, takes the writer tier and is admitted | -- |
| 2 `REQ1` | process 1 | requests writer exclusion and waits | -- |
| 3 `ATT2` | process 2 | attaches the same way | -- |
| 4 `REQ2` | process 2 | requests writer exclusion and waits | -- |
| 5 `GRT1` | process 2 | is granted exclusion and records itself | the ownership record names 2 |
| 6 `REL1` | process 2 | releases without an operation; exclusion is free and process 1's take is grantable | the record is cleared |
| 7 `GRT2` | process 2 | requests again and is granted before process 1's take is made | the record names 2 |
| 8 `REL2` | process 2 | releases without an operation | the record is cleared |
| 9 `GRT3` | process 2 | requests again and is granted first | the record names 2 |
| 10 `REL3` | process 2 | releases without an operation | the record is cleared |
| 11 `GRT4` | process 2 | requests again and is granted first | the record names 2 |
| 12 `OPN1` | process 2 | opens an operation | the count is odd |
| 13 `CLS1` | process 2 | writes nothing and closes | the count is even |
| 14 `REL4` | process 2 | releases; exclusion is free | the record is cleared |
| 15 `EXIT` | process 2 | detaches | -- |
| 16 `ATT3` | process 2 | attaches again | -- |
| 17 `GRT5` | process 2 | requests and is granted first | the record names 2 |
| 18 `REL5` | process 2 | releases without an operation | the record is cleared |
| 19 `GRT6` | process 2 | requests and is granted first | the record names 2 |
| 20 `OPN2` | process 2 | opens an operation | the count is odd |
| 21 `CLS2` | process 2 | writes nothing and closes | the count is even |
| 22 `REL6` | process 2 | releases | the record is cleared |
| 23 `GRT7` | process 2 | requests and is granted first | the record names 2 |
| 24 `REL7` | process 2 | releases without an operation | the record is cleared |
| 25 `GRT8` | process 2 | requests and is granted first | the record names 2 |
| 26 `OPN3` | process 2 | opens an operation | the count is odd |

Process 1's take is grantable at every release and never continuously, so weak fairness on it
asks nothing; process 1 waits forever while process 2 writes.

### T29 -- an upgrader is passed over forever by a handle that goes on attaching

Status: `[verified]`, the shortest trace of `Coordination-Live-AWaitingUpgraderIsServed.cfg`, an
upgrader unsafe for readers and a reader on the cyclic workload. Steps 4 to 8 repeat forever.

| Step | Actor | Event | Durable effect |
|---|---|---|---|
| 1 `ATT1` | process 1 | passes the gate shared, takes the reader tier, drops the gate, takes the writer tier and is admitted | -- |
| 2 `UPG` | process 1 | starts an upgrade, drains, releases its tiers and waits for the gate exclusively | -- |
| 3 `BEG1` | process 2 | begins to attach; the gate is free and process 1's take is grantable | -- |
| 4 `GTS1` | process 2 | takes the gate shared before process 1's take is made | -- |
| 5 `RDT1` | process 2 | takes the reader tier and drops the gate; the gate is free again | -- |
| 6 `ADM1` | process 2 | is admitted | -- |
| 7 `EXIT` | process 2 | detaches | -- |
| 8 `BEG2` | process 2 | begins to attach again | -- |

The upgrade never starts its body, and the schema it was to move stays where it is.

### T30 -- an upgrader is passed over forever by two handles whose holds of the gate overlap

Status: `[verified]`, the shortest trace of
`Coordination-Live-AWaitingUpgraderIsServedFairTwoHandles.cfg`, an upgrader unsafe for readers and
two readers on the cyclic workload under `UpgraderFairSpec`. Steps 7 to 17 repeat forever, and in
every state of them a reader holds the gate shared.

| Step | Actor | Event | Durable effect |
|---|---|---|---|
| 1 `GTS1` | processes 2 and 3 | each begins to attach and takes the gate shared | -- |
| 2 `RDT1` | processes 2 and 3 | each takes the reader tier, still holding the gate | -- |
| 3 `ATT1` | process 1 | passes the gate shared beside them, takes the reader tier, drops the gate, takes the writer tier and is admitted | -- |
| 4 `DRP1` | process 3 | drops the gate; process 2 still holds it | -- |
| 5 `UPG` | process 1 | starts an upgrade, drains, releases its tiers and waits for the gate exclusively | -- |
| 6 `ADM1` | process 3 | is admitted | -- |
| 7 `EXIT1` | process 3 | detaches | -- |
| 8 `GTS2` | process 3 | begins to attach and takes the gate shared, which process 2 still holds | -- |
| 9 `DRP2` | process 2 | drops the gate, which process 3 holds | -- |
| 10 `ADM2` | process 2 | is admitted | -- |
| 11 `EXIT2` | process 2 | detaches | -- |
| 12 `BEG` | process 2 | begins to attach again | -- |
| 13 `RDT2` | process 3 | takes the reader tier, still holding the gate | -- |
| 14 `GTS3` | process 2 | takes the gate shared, which process 3 holds | -- |
| 15 `DRP3` | process 3 | drops the gate, which process 2 holds | -- |
| 16 `ADM3` | process 3 | is admitted | -- |
| 17 `RDT3` | process 2 | takes the reader tier, still holding the gate | -- |

Process 1's take of the gate is never enabled, so no fairness on it can make it; every other
acquisition enabled in the cycle is made in it, so the behaviour is fair under `FairSpec` as well.

## The acquisition layer is a model of its own, 2026-11-12

> **SUPERSEDED in part, 2026-11-12, by "The coordination model refines the acquisition model"
> and "Every hold ends, checked in the coordination model".** The last point's duplication is
> resolved: the coordination model is checked to take and release as this model does, and its
> slow liveness configurations are retired.

Who is served by writer exclusion, the admission gate and the tiers depends only on how they are
taken and released, and on holds ending -- not on what a holder does while it holds. Checked
inside `Coordination.tla`, those results carry the count, the generation, the records, recovery
and the upgrade body through every state, which costs hours under strong fairness, and any change
to that machinery leaves nothing that says they still hold. So the acquisition layer is a model of
its own, `spec/Acquisition.tla`, and the layers are split by assume-guarantee:

- **The acquisition layer guarantees, assuming every hold ends**, everything the coordination
  model found about service: the store makes progress; a writer and an upgrader can each be
  passed over forever under the contract's fairness (T28, T29), and an attaching handle cannot;
  under strong fairness on each acquisition a writer and an upgrader beside one handle are
  served, and an upgrader beside two handles whose shared holds of the gate overlap is not (T30);
  and each fairness condition dropped stops the store. What a process does while it holds is
  abstract -- it holds, then releases -- and the model has no counter to wrap, so its invariants
  (one writer at a time, the gate and each tier exclusive against shared) and its liveness are
  checked together, every configuration in seconds. Its results are established once and
  re-checked only when the acquisition steps change[^mp-4-9].
- **A layer above owes it only that every hold ends**: every write releases exclusion, every
  handle eventually detaches, every upgrade holding its tiers finishes. Those are properties of
  one operation, one attachment or one upgrade body, checkable on bounded configurations, which
  is what keeps a change above the acquisition layer from reopening its results.
- **Withholding shared admission while an upgrader waits serves it under the contract's fairness
  alone**, without strong fairness: once no handle passes the gate shared, the holders drop it,
  it stays free, and a take that stays grantable is made. Strong fairness on the upgrader's take
  is not what serves it; the gate staying free is.
- **One fairness condition per process covers all its takes.** Each process waits for at most one
  thing at a time and leaves a wait only by taking it, so fairness on the disjunction of a
  process's takes is fairness on each of them. Likewise the steps that are no one's choice once
  under way are each enabled from the process's own state alone, so one condition covers them.
  With these, `FairSpec` for three processes is within what TLC can expand, which in the
  coordination model it was not.
- **The coordination model's acquisition steps are, for now, a duplicate of these.** Both models
  state the same takes and releases; the correspondence holds by inspection until the
  coordination model takes and releases with the acquisition model's own steps, at which point
  its slow liveness configurations, whose findings this model now carries, are retired or kept
  as a cross-check of the composition. That merge-or-delete decision is scheduled work, not a
  standing duplication[^mp-4-10].

[^mp-4-2]: `MP-4.2` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-2) [completed 2026-11-11]
[^mp-4-3]: `MP-4.3` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-3) [completed 2026-11-11]
[^mp-4-4]: `MP-4.4` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-4) [completed 2026-11-11]
[^mp-4-5]: `MP-4.5` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-5) [completed 2026-11-11]
[^mp-4-6]: `MP-4.6` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-6) [completed 2026-11-11]
[^mp-4-7]: `MP-4.7` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-7) [completed 2026-11-12]
[^mp-4-8]: `MP-4.8` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-8) [completed 2026-11-12]
[^mp-4-9]: `MP-4.9` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-9) [completed 2026-11-12]
[^mp-4-10]: `MP-4.10` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-10) [completed 2026-11-12]

## The coordination model refines the acquisition model, 2026-11-12

`Coordination.tla` takes and releases writer exclusion, the admission gate and the tiers as
`Acquisition.tla` does, and says so in a check: a refinement mapping in
`CoordinationAcquisition.tla` names the acquisition state of every coordination state, and
`AcquisitionRefined` -- the acquisition model's initial state and next-state relation under the
mapping -- holds in every main configuration. A step that takes or releases differently from the
acquisition layer fails that property instead of drifting unseen[^mp-4-10].

- **The coordination model keeps its own steps and is checked against the acquisition model's;
  it does not conjoin them.** Its controls deliberately take and release against the acquisition
  rules -- an upgrader that keeps its tiers, a writer tier taken first, a gate held through the
  writer tier -- so its own guards must stay, and a conjoined acquisition step would only repeat
  them. Where the two disagreed, a conjunction would remove the disagreeing behaviours from the
  state graph, which hides drift; a property reports it. The property changes no state count, so
  every configuration explores the states it did, and listing it in a configuration is the
  switch. It is listed in the main configurations, and in no control and no cyclic configuration.
- **Where the coordination model is finer, the mapping decides which acquisition state it is.**
  A handle registering has passed its check and is not yet admitted, so it is `tiers`, and its
  admission is the registry's last step. A handle unregistering still holds its tiers: one being
  dropped is still attached, and letting go of them is its exit; an upgrader's is still draining,
  and letting go of them ends its drain. A detached handle and a dead process hold nothing and are
  exited. A writer holds exclusion from the take of the lock to its release, with the ownership
  record, the operation and recovery all inside that one hold. The states only the controls reach
  are not mapped.
- **An upgrade body's hold of writer exclusion is not an acquisition.** The upgrader holds the
  writer tier exclusively while it holds exclusion, so no handle that may write is attached and
  none can wait for exclusion: the hold is uncontended, and the mapping hides it. The upgrade body
  is the acquisition model's abstract hold of the tiers.
- **Upgrade starts are counted where the coordination model counts them.** It bounds them only
  with the upgrade body (`UpSteps > 0`); without it, a cancelled drain or an upgrade that found the
  schema moved lets an upgrader start again without bound. The acquisition model's
  `CountUpgrades` says whether starts are counted, and is mapped to whether the body is modelled.
- **The acquisition model has every way a wait or a hold ends other than by service, each behind
  a switch that is off in every earlier configuration.** `Escapes`: a waiter gives up, a writer
  withdraws or is refused, a drain is cancelled. `Refusals`: the check refuses a handle, and an
  upgrade holding its tiers finds the schema moved. `Deaths`: a process dies, or the host
  crashes, and everything it holds is released. None is fair. The mapping turns `Refusals` on
  always, `Escapes` with the coordination model's own, and `Deaths` when any death or crash is
  allowed.
- **The acquisition model's results hold with those steps on, so the coordination model inherits
  them for the behaviours it has.** Each result that holds -- progress, an upgrade holding its
  tiers completing, an attaching handle served, a writer and an upgrader served under strong
  fairness, an upgrader served when shared admission is withheld -- is checked again with all
  three switches on (`Acquisition-OtherEnds-*`). The results that fail keep failing, since the
  added steps are not fair and so only add behaviours. Progress needs one change of wording: a
  handle that begins to attach and gives up, again and again, waits with nothing acquired, yet no
  one is starved, so where waiters may leave the store's progress is that a resource is acquired
  or a waiter leaves again and again (`StoreMakesProgressOrWaitersLeave`).
- **With upgrade starts not counted, an attaching handle can be passed over forever.** That is
  the acquisition model the coordination model refines when it has no upgrade body, and every
  other result above holds in it (`Acquisition-OtherEnds-Uncounted-*`). But an upgrader that
  attaches, upgrades and attaches again holds the gate exclusively again and again, and a handle
  waiting to attach can take the gate only between those holds, which the contract's fairness
  does not cover (`Acquisition-Uncounted-AttachPassedOver`). So "a handle waiting to attach is a
  wait and not a starvation", in the points of conflict above, rests on upgrades being finitely
  many: the shared-against-exclusive conflict at the gate is a starvation when upgrades recur
  without end. This is an input to ODI-2 beside T28 to T30.
- **Where a process may die, a step that lets go of everything a process holds refines a
  death.** So the refinement is sharp for such a step only in a configuration where no process
  dies, and a probe of that shape runs in one.
- **What the coordination model owes in return is that every hold ends.** The acquisition
  model's service results assume it, and the refinement says nothing about it, since it is a
  property of behaviours that go on and the refinement a property of each step.

## Every hold ends, checked in the coordination model, 2026-11-12

The acquisition model's results about who is served assume that every hold ends. The
coordination model discharges that assumption on bounded configurations, and its liveness
checks on a cyclic workload are retired[^mp-4-11].

- **Each hold ending is a property of one process's own steps.** A write releases writer
  exclusion; an attach drops the gate and is admitted or refused; a drain ends; a detach lets go
  of the tiers; an upgrade holding its tiers finishes or gives up; a recovery under way ends. A
  death ends any of them. None needs another process to be served, so none needs fairness among
  waiters, nor a behaviour that goes on forever: each is checked on a bounded configuration
  (`Coordination-HoldsEnd-*`) under `HoldSpec`.
- **`HoldSpec` is the fairness of a process's own steps.** Its I/O completes, its operation and
  its registry work move on once begun, a clone's operation ends, and -- strongly, because the
  holder's own choice interrupts it -- a holder releases. One condition covers each process's
  own steps, since each leads the process out of where it is apart from the loop its own choice
  makes, a recovery stopped and started again, which is why release has its own. Each condition
  dropped lets a hold last forever, which a control for each property shows
  (`Coordination-Control-HoldsEnd-*`).
- **These are the acquisition model's assumptions under the refinement mapping.** A write's hold
  of exclusion is the acquisition model's `held`; an attach's admission, a drain, a detach and an
  upgrade holding its tiers are the steps that model takes as completing I/O and as holds that
  end. A handle's choice to detach at all is the client's, not the store's, and stays an
  assumption of both models.
- **The cyclic workload is retired, and the `Cyclic` switch with it.** Its liveness
  configurations found who is served under the contract's fairness, which the acquisition model
  now establishes in seconds with every way a wait or a hold ends other than by service, and
  which the coordination model inherits by refinement once its holds end. Kept as a cross-check,
  they would cost hours each, and one never finished. Their controls and witnesses went with
  them, since they only showed those checks could fail and the workload went on. The counters
  no longer wrap, so every configuration explores the states it did.

[^mp-4-11]: `MP-4.11` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#mp-4-11) [completed 2026-11-12]

## No caller code runs under exclusion but the commit observer, 2026-11-12

How long writer exclusion is held is the store's to decide, never a caller's. The surface the
models describe is held to that, and the parts of the API that break it are recorded here with the
work that brings them into line.

- **Caller code may run under exclusion only if it waits on nothing.** A callback that does
  nothing is plainly safe, so the line is not "no caller code" but this rule. A callback made
  while exclusion is held:
  - *waits on nothing*: it takes no lock and makes no call that can block on another thread, a
    process or a device -- no I/O, no sleep, no blocking send, no join or await. It may hand
    data off without waiting, such as a non-blocking send to a bounded queue, and it decides
    for itself what to do when the hand-off is full;
  - *does not call into the store*, through any handle;
  - *returns*: it does not panic or unwind through the store;
  - *does work bounded in time*.

  The rule is what makes a callback simple to model. A callback that waits on nothing depends
  on no state but its own thread's, so in the models it is one step of the holder, enabled
  whenever it is reached: it is among the holder's own steps, so every hold still ends under
  the fairness of a process's own steps, and it can form no cycle of waits with exclusion, so
  it adds no deadlock. Its bound makes it a bounded addition to a hold's length in CQ-1, where
  the bound is a parameter of the workload; the models have no time, and the rule asks only that
  the callback returns. A lock
  of the callback's own that nothing holds while waiting would also be safe, but showing it
  needs an argument about lock order the models would have to carry, so the rule leaves it out.
- **A transformation is data, not a closure.** `Store::transform` takes a closure and runs it
  between its read and its commit, under exclusion, so the caller decides how long the hold
  lasts. That is a misspecified form of the intended API: a transformation is stated as
  primitives in data, which the store applies without returning control to the caller, so its
  cost is the store's and bounded by it. A closure that kept the rule above would be safe, but
  nothing about a closure says it does, so the intended form stays data.
- **A transaction is built without exclusion and submitted once.** `Store::transact` takes an
  operation list and an options value. The options are optional and empty today; they are where
  a later mode, such as a different way of locking, is asked for. The list is built with no lock
  held, by a builder the store hands out, and the builder is only a way to make the value: it
  never holds exclusion between calls. Under exclusion the store checks every operation's
  precondition and applies the list atomically, or rejects it whole, so a hold is the store's
  own work.
  - *A read is recorded as a check.* Each document the builder reads is recorded with what it
    held -- its digest, or its absence -- as an operation that changes nothing and that the
    commit checks with the rest. So a transaction is safe against everything it read, not only
    what it writes. This is the entry the swap set lacks: today a precondition rides only on a
    document the set changes.
  - *Read-modify-write retries without the lock.* A helper runs a caller's closure against a
    fresh builder with no lock held and submits what it built; a commit rejected because a read
    changed is discarded and the closure runs again, up to a bounded number of attempts, after
    which it fails, as a snapshot does. `Store::transform` takes this form; the closure form,
    which runs under exclusion, is retired.
  - *Primitives the store evaluates come later, where retries are too costly.* An operation the
    store computes from the current value under exclusion -- add, maximum, append -- never
    conflicts, which is what a document everyone changes needs. They are added only where CQ-1
    shows retries failing too often. A document's body is opaque to the store, so such an
    operation works on bytes unless canopy comes to own a body format, which is a larger decision
    of its own[^odi-6].
  - *Conflicts are the contention transactions see.* Transactions that keep invalidating each
    other are passed over in the optimistic form, so how often a commit is rejected is a measure
    for CQ-1 beside a snapshot's failures, and the bound on attempts is what keeps it from
    becoming a livelock; whether anything more is owed is ODI-2's question.
- **`Store::plan_and_apply` is outside the modelled surface.** It runs a caller's plan, reads
  included, under exclusion, for as long as the plan takes. It is withdrawn from the public
  surface the models describe, and from the public API itself[^api-1].
- **The commit observer is the callback the API makes under exclusion, and it keeps the rule.**
  It is a conceptual exposure: the observer runs on the writing thread inside the write. The
  models take the rule as an assumption about it and do not model what it does. Its author is
  held to the rule, which its documentation states; the store does not check it.
- **In the coordination model the rule is a step and three controls**[^odi-7]. Under the
  `Callbacks` switch, which is off in every configuration that does not name it, the store calls
  back once an operation's effects are done, inside the operation's bracket and under exclusion:
  `Observe`, a step of the holder enabled whenever it is reached, after which the operation
  closes. `ACallbackChangesOnlyWhereItsHolderIs` states that the step changes nothing but where
  its holder is. Each clause has a control: a callback that writes to the store violates that
  property; one that waits until nobody holds exclusion, which its own holder does, deadlocks;
  and one that never returns keeps its hold forever, so that a hold of exclusion ends fails
  under the fairness of a process's own steps. With the rule kept, every invariant and property
  holds and every hold ends.
- **Nothing nests under a hold.** Exclusion is never held by anything outside the store across
  a call, and no caller code that runs under it may call into the store, so no operation of a
  caller's runs inside another. The models have no nested operation: a hold is one request and
  at most one operation. That the count is raised only at the outermost edges of the store's own
  internal calls is the implementation's to keep, and its tests show it.
- **So a hold's length is the store's own work.** Under exclusion a write spends the store's I/O
  (staging, the commit log's records, the swaps and their syncs), the evaluation of a
  transformation's primitives, and at most the observer's stated time. The first is measured on
  the target hardware, not assumed; the rest are bounds the store or the observer's contract
  sets. This is what CQ-1 models as the hold time.
- **The workloads canopy is built for**, as the engineer states them, and as CQ-1's input:
  - *A service*: one process whose asynchronous workers make concurrent requests, a number the
    service may or may not throttle. Contention is within the process.
  - *A cooperative database*, like an IDE's project file or a PDB: a few readers, and now and then
    a burst of workers reading and writing, up to the number of processors as processes or
    threads, and at worst that number squared (that many processes of that many threads each).
  - *A ubiquitous metadata store*: many attached clients, nearly all only reading, with very
    occasional writes. Readers do not take exclusion; a read retries while writes disturb it, and
    fails after a bounded number of attempts.
  - Schemas are upgraded at most about once a month, so upgrades contend with attachments, not
    with each other.

[^odi-6]: `ODI-6` in [CHECKLIST.md](CHECKLIST.md)
[^api-1]: `API-1` in [CHECKLIST.md](CHECKLIST.md)
[^odi-7]: `ODI-7` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#odi-7) [completed 2026-11-12]

## How long the contract makes a caller wait, 2026-11-12

The TLA+ models say what the coordination contract lets happen; `crates/contention-sim` predicts
how often and for how long, for the three workloads in "No caller code runs under exclusion but the
commit observer", against a budget the engineer agreed before it ran[^cq-1]. Its report is
regenerated by `cargo run -p contention-sim --release`; what follows are the findings, which are
expected to survive a re-run on other hardware because they are stated against the measured hold.

- **The budget.** A service's write waits at most ten times the median hold at the 99th
  percentile and a hundred times at the 99.9th; a cooperative database's burst member waits at
  most one second at the 99th; a metadata store's snapshot fails with probability at most one in
  a million; a monthly upgrade reaches its body within sixty seconds. The service bounds are
  multiples of the hold, so that they move with the hardware.
- **The hold is the store's own work, measured.** A write's hold of exclusion is the durable
  transaction path, dominated by its syncs: on the development machine its median is about 37 ms
  with one pointer replica and about 52 ms with three, its 99th percentile about 1.6 times the
  median, and roughly one write in three hundred about eight times as long. The simulator draws
  holds from that shape. Recovery is folded in as rarer, longer holds, and a callback's time adds
  to every hold.
- **Unordered service is a uniform random choice among waiters**, with a swept probability that a
  request arriving while a released lock is handed to a woken waiter takes it first. Barging made
  no measurable difference: a handoff takes microseconds against holds of tens of milliseconds.
- **Method.** One seeded discrete-event simulation per configuration (SplitMix64, as the tests use),
  ten independent replications, a warm-up discarded, and 95% intervals across replications. The
  M/G/1 mean wait (Pollaczek-Khinchine) is the check: under Poisson arrivals every discipline's
  simulated mean matches it, as the conservation law requires, and only the tails differ.
- **A service is bounded by utilization, not by order.** Every discipline meets the budget up to
  a utilization of about 0.6 and breaches it from about 0.7 -- with these holds, roughly twenty
  writes a second -- first-come-first-served included. Unordered service's tail is longer, its
  99.9th percentile about twice first-come-first-served's at high load, and bounded bypass recovers
  most of the difference. So for a service the remedy for the budget is throttling below that
  utilization or shorter holds; an order among waiters moves the breach point little.
- **A cooperative database's bursts are where order matters.** A burst of as many writers as
  processors breaches the budget under unordered service, while first-come-first-served and
  bounded bypass meet it; first-come-first-served within a process does not help when each process
  has one writer in the burst. A burst several times the number of processors breaches under every
  discipline, because draining its writes takes longer than the budget: no order can help, only
  fewer or shorter writes.
- **A snapshot that spins fails whenever a write overlaps it.** The store's snapshot retries after
  a yield, sixteen times, which together take microseconds; a write holds the count odd for tens of
  milliseconds, so a read that begins during a write, or during which one begins, fails outright.
  Its failure probability is about the write rate times the mean hold, which meets the budget only
  below about one write in eleven hours. A snapshot that instead waits for the count to be even,
  up to a deadline of a second, never failed in the simulation and took at most about half a second.
  This is a defect against the budget, not a contention trade-off[^rd-1].
- **A monthly upgrade waits for the longest-lived attachment.** Gate holds are a couple of
  milliseconds, so overlapping shared holds keep an upgrader from the gate only at thousands of
  attaches a second, and withholding shared admission shortens that wait without changing the
  outcome. What dominates is the drain of the tiers: the upgrade's body waits until every handle
  already attached detaches, which for minute-long attachments is minutes, and for hour-long ones
  hours. So the budget is met only where attachments are short-lived, whatever the gate's
  discipline; for a metadata store whose clients stay attached, an upgrade needs its attached
  handles to let go when asked, which is an avoidance ODI-2 weighs before any fairness.
- **A callback's time is paid by every writer.** It lengthens every hold, so a few milliseconds
  against a hold of tens raises the utilization and the tail with it, and a callback as long as
  half a hold pushes a service at moderate load past saturation.

[^cq-1]: `CQ-1` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#cq-1) [completed 2026-11-12]
[^rd-1]: `RD-1` in [CHECKLIST.md](CHECKLIST.md)

## A change to the coordination state is delivered as a notification

Work that needs the coordination state to change before it can go on -- a snapshot that found a
write in progress, and needs it to end so that its read is undisturbed -- does not block a thread
waiting for it. It registers interest through a primitive of the coordination interface: notify
when the published modification count differs from this value, sampled earlier, or when this
deadline passes. A notification rings the store engine's doorbell (see "A client reaches the store
through a session and its queue pairs"), the same doorbell a client's submission rings, so the
engine wakes for a change in the coordination state exactly as it wakes for new work, and takes up
the entry that was waiting on it.

The interface states the contract and never how the change is noticed, so that one implementation
may look for it and another be told of it, and in neither does waiting work hold a thread. The
file-based implementation's default keeps one poller per process for every registration on a store,
sampling the count with backoff while any registration is outstanding and none while there is none,
so the cost of waiting is one thread per process however much work waits. On Windows an
implementation can instead request an opportunistic lock on the published count with overlapped
I/O, which the OS breaks when a writer renames a fresh count over it, so the registration is
notified from an I/O completion when the write completes and exclusion is released, with no thread
polling or blocked; where the lock is not granted it falls back to the poller. A write holds the
count odd for a durable transaction, tens of milliseconds, which is why neither form spins.

A notification is only a hint: the engine samples the count again and decides from what it reads,
so a spurious or lost notification costs time, never correctness, and the deadline bounds what a
lost one costs. The primitive is built with the interface[^mp-5]; the synchronous snapshot waits on
it until the queues replace it[^rd-1]; the Windows notified form is queued work of its own[^rd-2].

[^rd-2]: `RD-2` in [CHECKLIST.md](CHECKLIST.md)

## A client reaches the store through a session and its queue pairs

All work is put on a queue, and the client awaits its completion entry. A client acquires a session
on a store, and from the session one or more queue pairs: a submission queue the client writes and
the store's engine reads, and a completion queue the engine writes and the client reads. Nothing the
store does runs on a client's thread; a synchronous call, where one is offered, is a convenience
that submits one entry and waits on the completion doorbell. Sessions and queues are strictly
in-process.

Each queue has one producer and one consumer. On the client's side both ends of a pair have one
owner, who need not be one thread but is responsible for exclusive access; the types make that
exclusivity hold by construction -- an end is `Send` and not `Sync`, and pushing or popping takes it
`&mut` -- so a misuse is a compile error rather than a race. On the store's side the engine is the
one consumer of every submission queue and the one producer of every completion queue.

- **Two doorbells.** A push only queues. The submission doorbell is rung by the client, once for a
  batch, to tell the engine there is work; the engine's waking is scheduled work, so ringing it
  when the engine is already scheduled costs nothing more. The completion doorbell is rung by the
  engine when a completion queue goes from empty to not empty, and is a waitable object -- an event
  on Windows, its portable counterpart elsewhere -- so the client waits on it however it likes:
  in a thread pool's wait, beside other handles in one multiple-object wait, or by pumping from a
  thread of its own. A client is never made to dedicate a thread to a queue.
- **The completion doorbell is a level, armed by a fixed protocol.** It means "there is something
  to take", not "something arrived", so it survives being ignored by a wait that reported another
  handle. A consumer takes everything, clears the doorbell, checks the queue again, and waits only
  if that check finds it empty; skipping the check, or clearing after it, is a permanent hang, not
  an occasional stall. The producer's publish-then-test and the consumer's clear-then-check are the
  store-buffer shape, which release and acquire do not order, so each side carries a sequentially
  consistent fence; and the doorbell's own record of being rung is cleared only after the event is
  reset, so that it never claims rung over a doorbell that is dark. These rules, and the
  measurements behind skipping a redundant ring, are those of the doorbell in the
  [windows-waitable-queues](https://github.com/MikeGrier/windows-threadpool-sys/tree/main/crates/windows-waitable-queues)
  crate, whose design this follows; the pumped delivery of
  [durable-ioring](https://github.com/MikeGrier/windows-threadpool-sys/tree/main/crates/durable-ioring)
  is the portable form of the same pattern, draining before it waits.
- **Submitting reserves the completion.** An entry is accepted only if its completion queue has a
  free slot, which the submission takes; with none free the submission is refused to the submitter,
  on its own thread, at the moment it can act on it. So posting a completion never waits, and the
  engine never waits on a client: a slow or absent reader of a completion queue cannot lengthen a
  hold, which is the rule that caller code under exclusion waits on nothing (see "No caller code
  runs under exclusion but the commit observer") applied to completions. Every accepted entry
  receives exactly one completion while its pair exists.
- **A pair's entries take effect in the order they were submitted.** Each entry takes effect after
  every earlier entry of its own pair, so a client sees its own writes in the order it made them.
  Between pairs nothing is ordered, and the engine may run one pair's reads together once the writes
  before them have taken effect.
- **An atomic operation is never cancelled.** Once submitted it is not broken; once the engine has
  taken exclusion for it, it runs to completion whatever the client does. Dropping a pair is not
  cancellation: entries the engine has not started are withdrawn and never run, entries it has
  started complete, and their completions are discarded. A client that drops a pair with a write in
  flight therefore does not learn that write's outcome, the same position as a write whose outcome is
  unknown, and finds out the same way, by reading.
- **A long enumeration ends when its handle is dropped.** An enumeration holds no exclusion, so
  ending it releases only what a reader holds; it is dropping a handle, not cancelling an operation.
- **No waiting holds a thread.** An entry that must wait -- for exclusion, for the admission gate,
  for the modification count to be even -- is set aside by the engine with a registration on the
  coordination interface's notification, which rings the engine's doorbell when the state changes
  (see "A change to the coordination state is delivered as a notification"), so the engine's
  threads do work and never wait for it.

What this changes in the models is small, because the coordination model's process is an OS process
and never modelled threads: the engine is the one actor in a process that takes its locks, and its
writes are serialized by writer exclusion, so the entries of all its pairs are that process's
operations one at a time, as the model already has them. Three things are new. The queues and their
doorbells -- the reservation, exactly one completion, per-pair order, what dropping a pair does, and
the arming protocol under which no completion is waited past -- are in-process and independent of
the coordination state, so they get a small model of their own rather than enlarging the
coordination model's state space[^qs-1]. The liveness the coordination model checks rests on waiting
work being taken up again, which a notification makes an assumption about the notifier; the model
gets the notification as a step of its own, under fairness, with a control in which a notification
is lost[^nt-1]. And the store-buffer hazard is invisible to TLA+, whose steps are sequentially
consistent, and to tests, which do not produce the interleaving: the fences are checked by a checker
of the Rust memory model, chosen as a toolchain decision when the queues are built[^api-3].

[^qs-1]: `QS-1` in [CHECKLIST.md](CHECKLIST.md)
[^nt-1]: `NT-1` in [CHECKLIST.md](CHECKLIST.md)
[^api-3]: `API-3` in [CHECKLIST.md](CHECKLIST.md)

## The ring: a portable protocol over one platform primitive, behind a contract spec

> **SUPERSEDED in part, 2026-11-13, by "The doorbell's signal is sealed".** The bullet "Native
> waiting, by `cfg`" has the completion doorbell implement `AsHandle` and `AsFd`; through those safe
> code can change the signal, so the doorbell lends its platform object only through `unsafe`
> accessors.

The queue pairs and doorbells of "A client reaches the store through a session and its queue pairs"
follow the ring of
[durable-ioring](https://github.com/MikeGrier/windows-threadpool-sys/tree/main/crates/durable-ioring),
the most refined form of the pattern, and its doorbell rules from
[windows-waitable-queues](https://github.com/MikeGrier/windows-threadpool-sys/tree/main/crates/windows-waitable-queues).
They are built here, not taken as dependencies, and are meant to be the form those crates follow in
turn.

**Where it lives.** The ring is a module of canopy, `canopy::ring`, published with canopy and not as
a crate of its own: it is not offered as a product, and a module keeps it out of the set of
crates that must be published and versioned apart. The one platform primitive it needs lives in
acadia, which holds the repository's operating-system calls and its `unsafe`, so canopy stays free
of both. Lifting the ring into a crate later moves the module and nothing else.

**Specialized by type, at no cost.** There are three layers, and only the lowest knows the platform:

- **A signal, in acadia.** A sealed trait, `Signal`, with raise, lower and a bounded wait, and one
  implementation per platform: a manual-reset event on Windows; an `eventfd` on Linux, non-blocking
  and not in semaphore mode, which is readable while its counter is not zero and lowered by a read,
  so it is a level as the event is; and a mutex and condition variable for every other target and
  for tests. `PlatformSignal` names the platform's implementation by `cfg`.
- **The protocol, in `canopy::ring`.** `Doorbell<S: Signal = PlatformSignal>` holds everything
  that makes a doorbell correct -- the record of being rung that lets a redundant ring skip its
  system call, the reset before that record is cleared, the sequentially consistent fence on each
  side of the store-buffer shape -- once, for every platform. The queues are generic in the same
  way. Every use is monomorphized: no trait object, no branch on the platform at run time, nothing
  a hand-written Windows or Linux version would not also pay.
- **Native waiting, by `cfg`.** The completion doorbell implements `AsHandle` on Windows and `AsFd`
  on Linux, so a client composes it with a thread pool's wait, a multiple-object wait or `epoll`
  without the ring knowing its event loop. Its portable `wait`, the pumped form, exists everywhere.
  The platform appears in the public surface only as the standard library's own platform traits,
  so portable code never names a platform type.

The separation is the pattern's point. windows-waitable-queues joins its doorbell's protocol to
`CreateEventW`, so its correctness argument cannot be reused on another platform, and its fences
cannot be model-checked without the kernel object, which a checker of the Rust memory model cannot
run. With the signal as a type parameter, the protocol is written and argued once, and checked with a
signal the checker can run; durable-ioring's backend, whose notification is an associated type, is
the same idea one level up.

**A spec barrier at each layer.** Each layer has two TLA+ modules: a contract, which states only what
a user of the layer can observe, and an implementation, which states the mechanism and is checked by
TLC to refine the contract, as the coordination model refines the acquisition model. For the
doorbell, the contract is that work raised is taken and that a waiter with something to take is
woken, spurious wakes allowed; the implementation has the record of being rung, the signal's state
and the consumer's take, clear, check again and wait, each step one atomic operation or fence point
of the code. For the ring, the contract is exactly one completion per accepted entry, per-pair order,
a submission refused when its completion has no slot, and what dropping a pair does; its
implementation uses the doorbell only through the doorbell's contract. Every spec above a layer --
the ring's over the doorbell's, the coordination model's notification over the ring's -- composes
with the contract alone, so (a) no consumer can name the mechanism, and its specs stay valid through
any change the refinement still admits, and (b) the mechanism is validated where it lives, by the
refinement, by controls that break each rule and must fail it, and by probes.

The barrier has a floor, and it is stated rather than assumed: TLA+'s steps are sequentially
consistent, so the implementation spec is sound only if the code realizes each of its steps
atomically, which the fences are for. That is checked below the barrier, by a checker of the Rust
memory model running the protocol over a signal it can model, with the correspondence between each
spec step and the code that realizes it written beside the code; and the code is held to the
contract by tests that drive scripted interleavings over a scripted signal and check every outcome
against it[^rg-1][^rg-2][^qs-1].

[^rg-1]: `RG-1` in [CHECKLIST.md](CHECKLIST.md)
[^rg-2]: `RG-2` in [CHECKLIST.md](CHECKLIST.md)

## The contract promises no order among waiters at any point of conflict, 2026-11-12

The coordination contract promises no fairness: at no point of conflict is a writer, an upgrader or
an attaching handle promised to be served, and no order among waiters or bound on how often one is
passed over is promised. The evidence is "What the contract's lack of a fairness requirement does",
"The acquisition layer is a model of its own", "The coordination model refines the acquisition
model" and "How long the contract makes a caller wait"; the decision is the engineer's[^odi-2].

- **Writer exclusion stays unordered.** A writer can be passed over forever (T28), and a burst of
  about as many writers as processors breaches the wait budget under unordered service where
  first-come-first-served and bounded bypass meet it. That breach is accepted. The remedies are a
  caller's: fewer or shorter writes, and throttling below the utilization at which the budget
  breaks, since a service is bounded by utilization in every order and a burst several times the
  number of processors breaches in every order. The avoidance the contract has is that a hold is
  the store's own work and nothing else ("No caller code runs under exclusion but the commit
  observer").
- **The admission gate stays unordered.** An upgrader can be passed over by a handle that goes on
  attaching (T29), and by two whose shared holds overlap whatever fairness its own take has (T30).
  An upgrade on a busy store waits for a lull in admissions, as the contract states. Gate holds
  are a couple of milliseconds, so that wait is long only at thousands of attaches a second.
- **The drain of the tiers is a wait, and attached handles are not asked to let go.** An
  upgrade's body waits until every handle already attached detaches, so it waits as long as the
  longest-lived attachment: minutes where attachments last minutes, hours where they last hours.
  A metadata store whose clients stay attached meets the monthly upgrade's budget only when its
  clients detach of their own accord; an embedder that needs upgrades to proceed arranges that.
- **An attaching handle's service rests on upgrades being finitely many.** A handle waiting to
  attach waits out an upgrade's exclusive holds of the gate and the tiers. With finitely many
  upgrades -- at most about one a month in every workload canopy is built for -- that is a wait
  and not a starvation; it becomes a starvation only when upgrades recur without end
  (`Acquisition-Uncounted-AttachPassedOver`). The contract states that dependency instead of
  adding fairness for attaching handles.
- **Optimistic transactions and snapshots are outside this decision.** A transaction's rejected
  commits are bounded by its attempts, and which primitives the store evaluates to avoid them is
  ODI-6's question[^odi-6]. A snapshot failing under an overlapping write is a defect against the
  budget, fixed by waiting on the notification[^rd-1].
- **The alternatives weighed and not taken**, so that they return only with new evidence:
  - *bounded bypass on writer exclusion*, the weakest form that meets the burst budget, which
    would constrain every implementation and whose bookkeeping would first have to be modelled
    and its bound checked;
  - *eventual service on writer exclusion*, strong fairness on each writer's take, which does not
    address the burst tail, since uniform random service already serves every waiter eventually;
  - *group commit*, an engine committing several queued transactions in one hold that shares
    their syncs, which would make cross-process contention one waiter per process;
  - *exclusive-first admission*, no handle passing the gate shared while an upgrader waits for it
    exclusively, which serves the upgrader under the contract's fairness alone;
  - *asking attached handles to let go* when an upgrade drains;
  - *fairness for attaching handles* against upgrades that recur.
- **No work follows from this decision.** The coordination interface and its implementations take
  exclusion, the gate and the tiers with no order among waiters, as MP-5, MP-8 and MP-11 are
  written. The implementation is benchmarked against CQ-1's predictions and the same budget, and a
  prediction its measurements contradict reopens this decision[^cq-2]. What is not promised is
  advertised as such[^odi-5], and dependents are told what waiting to expect, the drain
  included[^cm-4].

[^cq-2]: `CQ-2` in [CHECKLIST.md](CHECKLIST.md)

## A timeline is replayed by TLC from a scenario table, 2026-11-12

A timeline in this file is checked against the model when it is written, and after that only if its
scenario is replayed. So every timeline whose model still has the configuration it was read from
carries a scenario: a table, beside the spec, that lists for each of the timeline's step ids the
actions of the model that make up that step, with their arguments. A generator writes from those
tables a replay module for each spec and one configuration per timeline. The replay module extends
the spec with a step counter and, for each timeline, a specification whose next-state relation
allows only the scenario's next action; the configuration takes the constants of the configuration
the timeline was read from, so TLC replays exactly the behaviour the table describes against the
current spec[^odi-4].

- **A replay is a witness.** Its configuration states that TLC must report the scenario's end
  reached as a violation, the form `tools/run-tlc-all.ps1` already reads, so a timeline whose
  behaviour the model no longer has fails the sweep like any witness whose state became
  unreachable. The scenario names actions, not states, so the replay passes whenever the model
  still allows the sequence. One that stops early finds no error, and TLC's search depth says
  which action it stopped at, which the replay module's comments map to the step.
- **What a timeline leaves out is in the table, marked.** A timeline omits what does not bear on
  its point -- attachments its status line only mentions, a bystander's steps. The table holds
  the actions the model needs for them, before the first step or as a step marked omitted where
  the model needs them later, with a note saying why; the timeline's own steps are replayed as
  written.
- **A stale timeline is attributed to the change that made it stale.** Each timeline has a
  specification of its own, so its configuration reaches the definitions its actions name and no
  other timeline's; the impact analysis selects it whenever one of them changes, and a failing
  replay is reported together with the changed definitions it exercises.
  The replay costs seconds; scoping it by dependency is about attribution, not time.
- **A timeline is superseded, never edited, when its replay fails.** A failed replay is a design
  point gone stale; it gets a banner and a replacement timeline with a new number, under the rules
  for a design point, and the stale timeline's scenario is retired with it.
- **The scenario tables are in the form the trace corpus adopts.** The corpus of TLC behaviours
  that M7 exports and replays against the code includes every timeline's replay, so one artifact is
  the document's evidence, the model's replay and the replay against the code.
- **A timeline read from a configuration that has been retired is not replayed.** Its section
  carries a banner saying that the configuration is retired, and its status stays as it was when it
  was written. The cyclic workload's timelines, T28 to T30, are such timelines: their behaviours go
  on forever, and their configurations were retired with the workload.

[^odi-4]: `ODI-4` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#odi-4) [completed 2026-11-12]

## Runs that follow a spec edit are selected by what the edit reaches, 2026-11-12

Once a model's rules are authored, a spec edit is usually local -- to a property, a witness, a
scenario -- and running every configuration for it spends hours to learn nothing. So the sweep and
the probes can run only what the edit can affect, and a scheduled run keeps the whole set honest[^spec-2].

- **What a configuration reaches is computed from the spec's text, by name.** Every definition's
  references are the names in its body that the spec defines; a module the spec extends shares its
  namespace, and an instanced module has its own, reached through `I!D`. A subscript such as
  `[][Next]_vars` or `WF_vars(A)` names `vars`. A configuration reaches what its `.cfg` names and
  everything that refers on from there. A bound or `LET` name that shadows a definition is still
  taken as a reference, so a selection is never smaller than what TLC evaluates. SANY's semantic
  export would resolve those exactly, at the cost of a Java pass per run; over-approximation by
  name costs only selecting a little more, so the analysis does not use it.
- **A configuration is selected when what its outcome depends on has changed** since it last came
  out as its header requires: its own text, the TLC jar, the declarations of a namespace it
  reaches (everything outside a definition), or a definition it reaches. A comment is not part of
  a definition. A probe is selected by the same rule over its base configuration, plus the mutant
  as written and the definitions its edits fall in.
- **The record of green runs is derived state and is never checked in.** It lives under
  `.scratch/tlc/record`, one file per spec and per tool, keyed by the spec's directory so that a
  scratch copy never shares one with the real spec, and in the scheduled workflow's cache. Beside
  each configuration's digest it keeps the distinct-state count of a pass, so an edit meant to
  leave a configuration exploring the same states is checked by the count it reports.
- **A run without selection ignores the record.** The milestone boundary's sweep and probe run are
  full runs; selection is for the work-item loop.
- **The full sweep and the probe run are scheduled weekly** on a hosted Windows runner, and run
  only when the specs, their configurations and scenario tables, the tools or the pinned TLC
  differ from the last green run, which the workflow's cache key digests. Whether a hosted runner
  finishes within its time limit is decided from its first run[^spec-2-4].

[^spec-2-4]: `SPEC-2.4` in [CHECKLIST.md](CHECKLIST.md)

## `STRANDED-VOTE` is guarded by a durable publication record, 2026-11-12

canopy guards against `STRANDED-VOTE` with a durable publication record. Both guards that hold were
checked in every model -- one document's publication, the composition with the coordination
contract, logged and unlogged writes mixed, several processes, deaths and crashes -- and the
choice between them is the engineer's[^fm-2-2]:

- **Recovery's work is bounded by the work left pending.** The record names the document and the
  version a failed write was publishing, so recovery settles the records it finds. The resolve
  guard's recovery cannot tell which document was being published and must sync every replica of
  every document in the store (T27), a cost that grows with the store.
- **The record is a format change, so it lands behind the format barrier.** An older binary would
  ignore a pending record and could strand the vote the record was guarding, so the genesis format
  version advances and existing roots are migrated before any record can be written.
- **Every operation on a document settles first, repair and scrub included.** The model shows
  repair and scrub need no settle for any property it checks; they settle anyway, at one fsync
  per document a scrub touches, so that no operation acts on a document with a record pending and
  the rule has no exceptions to remember.
- **A lost pointer replica is restored by a repair, not by the next write.** That is the one way
  the guards differ that no safety property shows (T26): a write under the record guard does not
  restore a replica it finds missing, and a publication that then fails part-way leaves no
  majority until the next settle.

The rules the record follows -- an append and a truncation, each with an fsync, and a settle that
fsyncs the record file before it acts -- are "The publication record's live and durable state,
modelled in `PointerPublication.tla`".

- **Where it lives.** The record is `.publication` at the store root
  ([store/publication.rs](src/store/publication.rs)), one NDJSON line per publication. The settle
  runs in the outermost entry to the writer gate, so every operation that takes the gate settles
  without remembering to, and in `open` before recovery replays the commit log. Its name is made
  durable in the root once per open.
- **Genesis version 6 is the barrier, and version 5 is migrated rather than refused.** A version-5
  root holds nothing a version-6 build reads differently -- it has no record, which reads as none
  pending -- so `open` refuses it with `StoreError::MigrationRequired` and `Store::migrate`
  advances it: under the root lock, with the marker read again under the lock, one atomic
  replacement of the marker made durable last. The root must be offline, since the lock cannot
  exclude an older binary that read the marker before the migration. The migration record,
  implementation record and coordination state belong to the coordination layer's format
  change[^mp-6].
- **`PointerPublication-Settled.cfg` checks the protocol as built**, the record guard beside
  logged writes, and every invariant and property holds of it. The unguarded protocol's
  counterexample is kept as the negative control `PointerPublication-Control-Unguarded.cfg`.

## A pointer vote nothing was modifying reports its disagreement, 2026-11-12

`read_document` brackets its pointer vote with two samples of the modification count, and when the
vote finds no majority -- for a pointer or for absence -- and `Store::undisturbed` holds of the
two samples, it returns `StoreError::PointerUnavailable` instead of `Read::Contended`[^fm-2-3].
This is the read classification the model checks ("The modification count, the generation and
reads, modelled in `Coordination.tla`"): two samples equal and even say no operation was in
progress across the observation, so the disagreement it saw is the store's state, not a writer's
intermediate one.

- **Contention is something waiting resolves; this is not.** A caller's loop over `Contended`
  (`read_settled`, `read_within`) waits for the writer to finish. With no writer, the replicas
  stay as they are until an operation settles them, so reporting it as contention would leave the
  loop spinning until its budget ran out and then blame a busy writer.
- **Under the publication record the case is narrow.** A failed publication leaves its document
  holding at most its old version and the recorded one, which keeps a majority while every
  replica is present. It loses it only when a replica is missing as well (T26), and the next
  operation that settles -- a write, a repair, a scrub -- restores a majority. Any other stable
  disagreement is damage.
- **The count is the process's own.** It sees this process's writers only, which is all a root
  has while one process holds it (`D116`). Readers in other processes depend on the count being
  published in the coordination state, which is the coordination layer's work.

[^fm-2-3]: `FM-2.3` in [CHECKLIST.md](CHECKLIST.md)

## The doorbell's contract and its mechanism, modelled in `Doorbell.tla`, 2026-11-12

The doorbell's spec barrier is two modules[^rg-1]. `spec/DoorbellContract.tla` states only what a
user of a doorbell observes: producers publish work, a consumer takes everything published, waits
and is woken; every item published is taken (`WorkIsTaken`), and a consumer waiting while work is
there is woken (`AWaiterIsWoken`), a wake with nothing to take allowed. `spec/Doorbell.tla` is the
mechanism, one step per atomic operation or fence point of the code, and TLC checks that it refines
the contract (`ContractRefined`, with the contract's two guarantees under the mapping) for one,
two and three producers.

- **A ring publishes, then swaps the record of being rung to set, and raises only if it was
  clear.** A ring while the doorbell is already rung therefore makes no system call; the witness
  `Witness_ARingSkipsItsRaise` shows such a ring is reachable.
- **The consumer takes, lowers the signal, clears the record, checks again, and waits only on
  finding nothing.** Finding work, it takes it and checks again without lowering or clearing, so a
  consumer kept busy pays no system call per batch, and the refinement holds with that path in.
- **The reset comes before the clear because a producer's test can lag its publish.** A producer
  whose work the consumer has already taken may still be about to test the record. Cleared first,
  that test sets the record again and its raise is then lowered by the reset, leaving the record
  set with the signal low, so every later ring skips its raise and the consumer waits for ever.
  This holds whether or not the consumer retakes without lowering: a scratch run that lowered and
  cleared on every pass still lost the wake.
- **The controls each break one rule and lose a wake** (`AWaiterIsWoken` violated): clearing
  before the reset, skipping the check after the clear, checking before the clear, and a producer
  that tests before it publishes.
- **The signal is a level and a spurious wake is never owed.** A raise holds until the consumer
  lowers it, which is what acadia's `Signal` provides, and a wake with the signal lowered -- a
  pumped wait's bound passing -- is a step the mechanism may take and the contract allows, with no
  fairness on it.
- **The barrier's floor is the code's atomicity.** TLA+ steps are sequentially consistent, so the
  model is sound only where the code makes each step atomic and fences the publish from the test
  and the clear from the check; that is checked below the spec, by a checker of the Rust memory
  model over the code[^rg-2].

## The doorbell in code, `canopy::ring::Doorbell`, 2026-11-12

> **SUPERSEDED in part, 2026-11-13, by "The doorbell's signal is sealed" below.** The bullets on
> native waiting, on registering with a thread pool and on not lending the `Event` say the doorbell
> lends its event through `AsHandle` and its descriptor through `AsFd`, and that canopy's tests hand
> a thread-pool wait a clone of the `Event`. Safe code can change the signal through either, so
> the doorbell lends its platform object only through `unsafe` accessors, and the tests use them.

`canopy::ring::Doorbell<S: Signal = PlatformSignal>` is `Doorbell.tla` in code[^rg-2]. Each step of
the spec is one atomic operation or fence of a method named for it, and the public surface is the
steps composed in the only orders the protocol allows.

- **A producer calls `ring`, which is `Test` then `Raise`.** `test` is a sequentially consistent
  fence and then a swap of the record to set, so whatever the producer published into its own queue,
  at whatever ordering, is ordered before the record is read; `raise` follows only if the record was
  clear.
- **The consumer's side is `rearm`, `wait` and `pump`.** `rearm` is `Reset` then `Clear`, in that
  order for the reason "The doorbell's contract and its mechanism" gives; `clear` is a sequentially
  consistent store followed by a sequentially consistent fence, so the clear is ordered before the
  check that follows it. `pump(timeout, take)` is the whole consumer round over a `take` that empties
  the caller's queue: take, rearm, take again until a take finds nothing, return if any found work,
  and otherwise wait and go round. A pump with a zero bound never waits, which is the form a consumer
  woken by a thread pool's wait or `epoll` runs.
- **The steps stay `pub(crate)`.** A caller outside canopy cannot reorder them; the tests drive them
  one at a time.
- **Native waiting goes through acadia.** The doorbell over the platform's signal lends its event
  through `AsHandle` on Windows and its descriptor through `AsFd` on Linux. acadia's `wait_native`
  waits on a borrowed one with the platform's own wait without lowering it, so canopy's library makes
  no system call and holds no `unsafe` for it.
- **Registering with a thread pool is a client's concern, and canopy's tests do it as a client
  would.** On Windows the tests register the doorbell with `windows-threadpool-sys`'s
  `ThreadpoolWait` (one-shot, armed again by each callback through its activation), handing it a
  clone of the `win-sync-sys` `Event` acadia's `EventSignal` holds, which it takes without
  `unsafe`, as the repository's rule for thread-pool interactions requires
  ([DESIGN-NOTES.md](../../DESIGN-NOTES.md) "Windows thread-pool interactions go through the
  windows-threadpool-sys crates"). On Linux they register it with an `epoll` set a pool thread
  waits on, through `libc`. Both are dev-dependencies; the library stays free of
  operating-system calls and `unsafe`.
- **The doorbell does not lend its `Event`.** A clone of the event can be reset as well as waited
  on, and a reset outside the protocol can leave the doorbell rung with its signal low, so that no
  ring wakes the consumer. Only canopy's tests reach the signal. A client registers the completion
  doorbell with a thread pool through a handle that can only wait, such as `win-sync-sys`'s
  `WaitOnlyEvent`, which the queue pairs provide[^api-3].
- **The code is held to the contract by an exhaustive search over its own steps.** The tests run the
  doorbell over a condition-variable signal, one step of one actor at a time, through every state
  one, two and three producers and the consumer can reach, with the consumer's rounds bounded; a state
  is the record and the signal read from the doorbell, the queue's counts and where each actor is,
  and a run is brought to a state by the doorbell's own `test` and `raise`. At every final state a
  consumer waiting while work is there must find the signal raised. The same search, with the
  consumer's or a producer's order changed as each of `Doorbell.tla`'s controls changes it, must find
  a lost wake, which shows the search can fail.
- **The fences are assumed correct until a checker of the Rust memory model checks them.** The
  search runs each step to completion, as TLA+ does, so it cannot see a step's stores reordered; that
  floor is the fences', and the engineer owns establishing it[^rg-2-1].

[^rg-2-1]: `RG-2.1` in [CHECKLIST.md](CHECKLIST.md)

## The doorbell's signal is sealed, 2026-11-13

Two requirements, the engineer's: the analysis of the doorbell may treat its layer as sealed --
`Doorbell.tla` assumes nothing but the protocol's own steps changes the signal -- and only an
`unsafe` caller can break that. So no safe method of the doorbell lends the platform object behind
its signal, or anything from which safe code can reach it.

- **Lending the object safely would break the seal, on both platforms.** `AsHandle` and `AsFd` are
  safe traits, and the standard library's I/O safety guarantees only that a borrowed handle or
  descriptor stays open, not what is done with it; changing an event's or an `eventfd`'s level is
  not memory-unsafe, so any crate may offer it safely. On Windows the event's own handle carries
  the right to reset it. A wait-only handle (`win-sync-sys`'s `WaitOnlyEvent`, `SYNCHRONIZE` alone)
  is refused a set or reset by the kernel, but `win-handles-sys`'s safe
  `duplicate_handle_with_access` can ask for a duplicate with more access, which Windows grants
  the event's creator, so safe code holding the wait-only handle can get one that resets. A
  client's `windows-threadpool-sys` wait hands its handle back to safe code through
  `WaitActivation::handle`. On Linux, safe code holding the descriptor can make a `File` of a
  duplicate and read it, which lowers the signal, or write it, which raises it; and waiting needs
  the descriptor, so no wait-only form exists.
- **The platform object is lent only through `unsafe` accessors.** On Windows
  `Doorbell<EventSignal>::native_wait_handle` returns a new wait-only handle to the event
  (acadia's `EventSignal::wait_only`), and on Linux `Doorbell<EventFdSignal>::native_fd` borrows
  the `eventfd`. Each is an `unsafe fn` whose contract is that the caller only waits on the object
  and changes its level by no means -- no duplicate, no `File` -- nor lets code it hands the object
  to do so. They are for a consumer's own native wait: a thread pool, a multiple-object wait, an
  `epoll` set. The wait-only handle is defence in depth on Windows: a caller that breaks the
  contract by accident is still refused a reset by the kernel. `AsHandle` and `AsFd` are no longer
  implemented for the doorbell, and no accessor to its signal remains, in tests or out.
- **Callers are steered to a thread pool, through a registration in the sealed layer.** The
  engineer's direction is that consumers wait on a thread pool rather than block a thread. The
  safe way to do that is a registration acadia makes on its own object, which runs a callback when
  the signal is raised and never hands the object to client code, and which the doorbell offers
  as `watch`. Until it exists, a consumer on a thread pool uses the `unsafe` accessor, as canopy's
  tests do[^ac-4][^rg-3].
- **A wait sees the level, so the consumer rearms before the wait is armed again.** A thread-pool
  callback that re-arms its one-shot wait before the doorbell has been rearmed -- the signal
  lowered by the consumer's `Reset` -- runs again at once. A consumer that pumps from the callback
  and then re-arms the wait has done both in that order.
- **The completion doorbell of the queue pairs is a `Doorbell` over the platform's signal**, so it
  is sealed by the same type and carries the same accessors when the pairs are built[^api-3].
- **A safe native wait on Windows depends on the windows-threadpool-sys crate family.** An event
  whose later handles the kernel holds to `SYNCHRONIZE`, or a wait-only handle no safe code can
  borrow, would let the doorbell lend its event without `unsafe`; what the family would provide is
  specified in [WINDOWS-THREADPOOL-SYS-SEALED-WAITS.md](../../WINDOWS-THREADPOOL-SYS-SEALED-WAITS.md)[^rg-4].
  On Linux an `eventfd` has no wait-only form, so the descriptor stays behind `unsafe` and `watch`
  is the safe path.

[^ac-4]: `AC-4` in [CHECKLIST.md](../acadia/CHECKLIST.md#ac-4)
[^rg-3]: `RG-3` in [CHECKLIST.md](CHECKLIST.md)
[^rg-4]: `RG-4` in [CHECKLIST.md](CHECKLIST.md)
