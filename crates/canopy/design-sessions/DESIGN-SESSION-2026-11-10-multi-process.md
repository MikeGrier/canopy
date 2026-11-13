# Design session 2026-11-10 -- the write surface, several processes, and store identity

Resulting decisions, all in [DESIGN-NOTES.md](../DESIGN-NOTES.md): "The public write surface
belongs on `Store`" (its inventory and "Choices made for the surface"), "Several processes may
open one store" (the coordination contract and its consequences), and the superseding banner on
`D116`. Work queued in [CHECKLIST.md](../CHECKLIST.md): milestones M2 to M6. Open questions are
listed at the end, each with its checklist counterpart.

## 1. What `WriteMonitor` is for

The inventory read `monitor.rs` and classified each capability by what it needs. The writer gate
(FIFO, re-entrant, shared by every `Store` clone and monitor) already serialises everything the
actor was introduced for. Only one capability needs a thread: draining the repair backlog after
the last caller's reply, so no write pays for it (`D339`). Nonce minting needs a home on `Store`;
the commit feed is better placed on `Store`, where it would see direct writes too.

The engineer's choices:

- **Repair drain:** an explicit call, no thread.
- **Which verbs commit through the log:** every verb that is neither management nor repair.
- **Unlogged writes:** `write_document_replicated` stays unlogged by design (`D332`), but still
  publishes pointers, so the `STRANDED-VOTE` guard covers it.

### Nonces: per call, or a source supplied at open

The word "install" caused confusion and was dropped: it means only that `Store::open` is handed a
nonce-making function (today's `NonceSource`, `Box<dyn FnMut() -> Nonce + Send>`) and calls it
when a write needs a nonce. Either way the caller decides how nonces are made, so `D61` holds.
Per-call nonces let the caller tie them to its own ids but make every call site responsible for
uniqueness; a supplied source is safe by construction and lets repair mint its own, at the cost
of that correlation (recoverable by returning the minted nonce in outcomes). Not yet decided.

### `write_pointer`

Worked through at the engineer's request. As a public primitive it can create a dangling durable
pointer directly (it never checks that the named blob exists or is durable -- the `D327`
ordering is left to the caller), it never collects the blob it replaces (`D328`), it has no
precondition, and it is a third unlogged publisher in `STRANDED-VOTE`'s scope. The only thing it
offers that a swap cannot is pointing a document at an existing blob, such as reverting -- unsafe
anyway, because collection may already have deleted the older blob. Recommendation:
crate-private.

Later in the session the engineer asked why it needed to be public at all. It does not: it was
public only because the store was built layer by layer and each layer's building block was
exposed as it landed. The engineer agreed to make it crate-private, together with `ingest_blob`
and `retain_blobs` (the latter can delete a blob a live pointer still names).

## 2. Several processes

The engineer had hoped the store could eventually admit modification from several processes, and
raised it now so it reaches the model before more is built on a single-process assumption.

### Feasibility

Today `Store::open` takes the root's exclusive lock for the handle's life (`D116`), so another
process cannot even open the store to read. The in-process state that depends on that: the writer
gate, the modification count (`D335`), the recovery fence, the commit log's durability cache,
the directory-sync debt, the repair backlog. Both platforms provide what is needed --
`LockFileEx`; `flock` or open-file-description locks (never process-associated `fcntl` locks) --
with release on death. Uncontended locking is one system call, small beside the fsyncs a write
already performs.

### Where the shared state lives

- **A file under locks** was the engineer's suggestion. It rests on specified OS behaviour rather
  than on atomics in memory shared between processes (which the Rust memory model does not
  formally cover). Process death is survived through the page cache without an fsync.
- **Leases and oplocks** as a cache for the count were examined and rejected as a correctness
  mechanism. A read-only lease break is, as far as is known, unacknowledged on Windows: the
  writer proceeds while the reader still believes its cached value. On Linux a lease break blocks
  the writer's open, inverting priority (`lease-break-time` is 45 s by default), and requires
  ownership or `CAP_LEASE`. They remain possible behind a contract whose clause is "a sample is
  never stale", if the model shows the ordering holds.
- **The engineer then asked for an abstraction** that admits either a straightforward file-based
  implementation or a high-performance shared-memory one, as long as it satisfies the contract.
  The model checks the contract once, and one conformance suite holds every implementation to it.
- **The abstraction owns every synchronization**, not only the counters, because their ordering
  guarantees are stated against each other.
- **It is a layer above `Vfs`, complementing it**: it decides who may act and when; canopy still
  performs every data operation through `Vfs`. The engineer pointed out that the question "should
  canopy stop using `Vfs::lock_exclusive`" had no point -- a new interface above `Vfs` does not
  obviate `Vfs`. An implementation is correct only for the `Vfs` and root the store uses, so the
  store builds its coordination from them through a factory.

### Host restart

The engineer asked whether the fence must survive a host restart, since an in-flight transaction
must be resolved on reopen. It must be resolved, but from durable state -- the commit log and the
`STRANDED-VOTE` guard's record -- not from the coordination state. A count raised without an
fsync can roll back to an even value over a writer that was mid-modification, so the
coordination state is never trusted after a restart: recovery is then required.

### Network filesystems

The engineer ruled them out explicitly, without detection, since detection is an unbounded
chase. The README, the crate docs, and the docs of `Store`, `init` and `open` disclaim them.

### Review of the plan

A review of the plan (GPT-6 Astra, read-only) raised thirteen findings, all accepted and folded
into the decision and checklist: one coordination implementation per root, recorded at creation;
attaching that never mutates the root (open-time recovery could otherwise treat another
process's live transaction as abandoned); recovery holding a capability the fence does not
refuse; an ownership record kept by the implementation, since no OS lock reports an abandoned
owner; a dead holder's odd count kept odd until recovery finishes; per-handle caches tied to a
generation advanced on every acquisition; lock-free sampling; callback re-entry handed to the
write-surface decision; restart recognition and state re-creation per implementation (a Windows
paging-file mapping vanishes with its last user); the publication-record guard's scope widened to
every operation on a document, with its "holds" made provisional; transaction behaviour pulled
forward into the multi-process model; and hygiene fixes.

### Recovery

The engineer made recovery public and asked that it be discoverable first, then run
incrementally with progress the caller can gauge, since recovering a badly damaged store can be
large. Recovery is therefore never charged to an open or to a write: a store that needs it
refuses writes with `StoreError::RecoveryRequired`. Each step is idempotent and resumable after
a stop or a death; a convenience runs it to completion.

## 3. Store identity and versions

The engineer asked that the root record the software managing it, so an administrator can find
that software. Refined over several exchanges:

- **Agent identifier** in two parts, a version-independent part and a version; **agent
  description**, human-readable. The version-independent part must match or the open is
  refused. An open by the recorded version changes nothing.
- **canopy imposes no construction on versions**, so it can compare them only for equality; any
  ordering must come from the agent. The obvious policy (newer stamps, equal proceeds, older is a
  compatibility check) is the agent's to apply.
- **Management:** for now only the description can be changed. Other store-level properties are
  expected over time, kept as settings; one call reads store-level properties.

### The concurrent-version problem

Version 1 opens; version 2 opens and stamps; version 1 is still attached. Re-applying the policy
whenever a handle acquires exclusion after the stamp moved was suggested. The engineer judged the
whole notion "a bit of a mess", perhaps to be disallowed, without a good solution offhand, and
framed three places this could end:

1. **Writers know about each other** actively and negotiate the locking and logging protocol, so
   a joining writer may downgrade or decline to join.
2. **The version is a schema version, not a software version**: a simple linearly increasing
   number. A schema upgrade is a dramatic one-way transition performed by exactly one writer with
   exclusive ownership of the store, perhaps loosened for readers.
3. **Retreat to in-process only.** The engineer doubted this solves anything: the same problems
   remain between threads, though in-memory queues and easy synchronization may make them
   simpler. Hence working it out now, to see what the base inter-worker abstraction needs.

### Analysis

"Version" conflates three things: canopy's on-disk format and protocol (canopy's; correctness
depends on it), the agent's document schema (the agent's; its correctness depends on it), and the
agent's software version (no correctness dependence). If compatibility is carried by the two
schema versions, the software version is informational, and version 1 and version 2 of one agent
can write side by side while both speak the current schema. That is choice 2, applied to both
schema layers.

Choice 3 is not a different design: the contract already treats threads and processes alike, so
in-process only means shipping only the in-process implementation of the same contract.

All three choices need something the contract lacks. Writer exclusion answers "who writes now";
none of the choices works without "who is attached at all". Proposed addition: **attachment**,
in two tiers (reader and writer), each held shared by every open handle for its life and taken
exclusive by an upgrade -- the reader tier only when the change is not safe for readers. The OS
releases an attachment on death, so a crashed process never blocks an upgrade. With it, a
running version 1 writer holds its writer attachment shared and an upgrade waits for it to
leave, so no live writer has the schema changed beneath it. A **registry** of attached handles
(each with its agent identifier and the schema range it speaks, liveness tied to its attachment)
would let an administrator see who blocks an upgrade under choice 2 and is the membership choice
1 needs.

Recommendation: choice 2 now, keeping choice 1 reachable through the registry; add attachment to
the contract and the model now; make the agent's software version informational, with
compatibility carried by the canopy-format and agent-schema versions.

## Open questions

Each has a counterpart in [CHECKLIST.md](../CHECKLIST.md).

1. Which of the three ends -- negotiation, schema versions with exclusive upgrade, or in-process
   only -- the store aims for; whether attachment joins the contract now; whether the registry
   joins it now or only when negotiation is on the table; and what the agent's version then
   means[^odi-1].
2. Nonces per call or from a source supplied at open[^ws-2].
3. The exact verb list, `write_pointer`'s visibility, and what a callback that receives the store
   may do on the thread holding exclusion[^ws-2].

## Outcome

- **Choice 2 is adopted.** Compatibility is carried by two schema versions -- canopy's on-disk
  format and protocol, and the agent's document schema -- each a linearly increasing number. A
  schema upgrade is one-way and made by one writer holding exclusive attachment.
- **The agent's software version is the agent's alone.** The engineer had been treating it as
  carrying schematic significance for canopy, and agreed it does not. canopy records the version
  of the agent that last wrote, never orders it, and leaves any refusal to the agent.
- **Attachment joins the coordination contract**, since an exclusive upgrade cannot be enforced
  without it.
- **The registry joins the contract too.** The engineer wanted it if it was not particularly
  expensive. Its cost falls only on opening and closing a handle -- an entry written and locked,
  no fsync, never touched by a read or a write -- plus moderate implementation work (entries
  whose data lies outside the locked range, since Windows refuses reads of a locked range;
  reuse of dead entries). It is best-effort except that a dead handle's entry is never reported
  as live. This answered the first open question[^odi-1].

## Nonces

Settled after the outcome above. A nonce names a write's staging and a transaction's log
records, so correctness needs only that no two operations with traces on disk at once share one,
across every process. A sequence from a file would need durable increments and a counter trusted
across restarts, which the coordination state is not -- and orphaned staging from unlogged writes
is not swept, so a rolled-back counter could reuse a live name. 128 random bits need no shared
state; a collision is far less likely than an undetected hardware error, and canopy makes one
fail an operation rather than corrupt anything (exclusive creation of each nonce's staging
directory; a `begin` whose id the log already holds is refused).

The engineer agreed: the source is supplied at open, canopy calls it once per operation and
reports the nonce used, and a random source ships behind a cargo feature. Unit tests use a
seeded pseudo-random generator with a fixed seed; longer-running integration tests seed it from
a hardware entropy source. That integration tests use randomness at runtime is the engineer's
explicit approval, recorded in [DESIGN-NOTES.md](../DESIGN-NOTES.md) as the repository's testing
rules require. This answered open question 2.

## The verb list

Settled last. Proposed and accepted as recorded in [DESIGN-NOTES.md](../DESIGN-NOTES.md):
`update_document` and `apply_transaction` already express the monitor's `write` and
`conditional_write`, because `Swap` carries the precondition; `transform` and `plan_and_apply`
commit through the log. Two choices resolved the remaining questions together:

- **`snapshot` became optimistic** -- re-run while the modification count shows a disturbance --
  so a long snapshot never blocks a writer in another process, in keeping with readers taking
  nothing.
- **Callbacks receive a read-only view** rather than the store. Re-entry stops being a question:
  a write from inside a callback does not compile, and its reads take no exclusion. Only a clone
  of the store captured by the closure can still deadlock against its own thread's exclusion,
  which the documentation warns of and a test exercises.

The commit feed reports every write, logged or not, but not repairs. This answered open question 3.

## Second review of the plan

A second read-only review (GPT-6 Astra) raised fifteen findings, all accepted and folded into
[DESIGN-NOTES.md](../DESIGN-NOTES.md) and [CHECKLIST.md](../CHECKLIST.md):

- **The modification count must bracket whole logical operations**, not filesystem steps: an
  optimistic snapshot taken between two swaps of one transaction otherwise sees an even,
  unmoved count and accepts half a transaction. Nested operations track depth so an inner one
  never restores even parity.
- **A schema upgrade needs its own crash-consistency protocol** -- a durable intent, resumable
  steps, the version advanced only at the end -- because exclusive attachment prevents
  concurrency, not an interrupted migration.
- **Attachment needs an order and an admission gate.** An upgrader holding its own shared tiers
  would block on itself, and one holding writer exclusion while waiting for attachment would
  deadlock against an attached writer. The upgrader converts its attachment behind the gate
  (OS locks do not convert atomically), new handles wait behind a pending upgrade, and the
  schema check moves after attaching.
- **Recording the implementation does not arbitrate creation**, since two implementations'
  creation locks need not exclude each other; creation is reserved first with an exclusive
  create that every `Vfs` provides.
- **A checksummed count retried on a torn read waits forever on a suspended writer**; two
  alternating slots keep a complete value readable.
- **Nonce collision detection must cover the real artifact layout** -- flat blob staging files
  and quarantine destinations, not only staging directories -- and must tell recovery's
  deliberate reuse of a transaction's staging from a collision.
- **Dropping handles is not a process's death**: a clean drop releases ownership and restores
  the count's parity. Tests abandon holds instead, cross-checked against real kills.
- **Recovery stays explicit everywhere**: acquiring exclusion after a death reports
  `RecoveryRequired` rather than replaying, and the publication-record guard no longer says
  `open` settles records.
- **"Recovery required" needed a definition**: unfinished work counts only once abandoned, and
  a permission table says what may run meanwhile.
- **Attaching changes no store data**, but registers -- coordination bookkeeping is allowed.
- **"First acquisition" cannot record the last writer** (A, then B, then A again); the version
  is compared after every successful write.
- **Registry entries carry a per-handle token**, so a listing never mistakes a reused entry for
  its dead predecessor.
- **The commit-log reset and `quiesce` are scheduled and tested**, refusing while work is
  unresolved.
- **A seed alone cannot replay a race**: unit tests give each actor its own stream and control
  the fault schedule; integration tests record the fault trace where actors race.
- **Hygiene**: the `Vfs` additions became items in acadia's own
  [CHECKLIST.md](../../acadia/CHECKLIST.md), so the handoffs link both ways; work-item citations
  here became footnotes.

[^odi-1]: `ODI-1` in [COMPLETED-CHECKLIST.md](../COMPLETED-CHECKLIST.md#odi-1) [completed 2026-11-10]
[^ws-2]: `WS-2` in [COMPLETED-CHECKLIST.md](../COMPLETED-CHECKLIST.md#ws-2) [completed 2026-11-10]

## Third review of the plan

A third read-only review (GPT-6 Astra) examined what the second round added and raised fourteen
findings, all accepted:

- **Two upgraders could deadlock**, one holding the admission gate while waiting for the other's
  attachment. No handle now waits for the gate while holding a tier: an upgrader releases its
  tiers first, takes the gate, converts, and checks the schemas again.
- **Alternating count slots do not guarantee a fresh sample**: a reader can keep a stale slot
  while rejecting a torn newer one. The count is now published atomically (a fresh file renamed
  over the published one) and a sample is one read; the model reads individual slots in a
  control that must be refuted.
- **Reporting a dead holder could erase the evidence** when the death left the count even: the
  next holder's clean release would clear ownership. The acquirer now sets the fence before
  replacing the abandoned record.
- **The creation reservation had an unprotected interval**, and `Vfs` has no exclusive file
  create. A creator now takes its implementation's creation exclusion before publishing a
  complete reservation with a publish-if-absent primitive that acadia adds.
- **A failed transaction unwinding the count to even** would let a snapshot accept its partial
  result; an unresolved operation now leaves the count odd until recovery.
- **The admission rules contradicted upgrade resumption and reader-safe upgrades**; one
  admission table now covers ordinary and upgrading states, the gate reopens to readers during a
  reader-safe upgrade, and resuming an interrupted upgrade is the store's recovery.
- **The registry could still report a dead handle live** when a reuser held the liveness lock
  before writing its token; an entry is now taken under a claim lock, and its liveness lock is
  acquired only after its token is written.
- **An interrupted creation needed a resumable state**: the reservation records every creation
  input, and a resume requires identical inputs.
- **A stamp could half-succeed**; it is now separate from the write's outcome, and the recorded
  version may lag but never names a failed write.
- **The generation comparison defeated its own fast path**; acquisition returns the generation
  it found before advancing it.
- **acadia's single milestone created a landing cycle**; AC-1 and AC-2 are separate milestones.
- **Safety checks cannot reject a waiter that waits for ever**; the model checks progress under
  stated fairness, with deadlock checking on and negative controls.
- **Checklist introductions carried rationale**; they are now pointers to the decisions.

## Fourth review of the plan

A fourth read-only review (GPT-6 Astra) raised thirteen findings. One was not about the plan: a
partial rename of the crate's old name had left a stray character in acadia's design notes, and
two test files still carried the old name; both were corrected. The twelve plan findings were
accepted:

- **An older binary could bypass the new protocol**, opening the root under the old lock and
  ignoring coordination state it does not know. Several processes is now a new format behind the
  genesis version barrier, with a one-way migration for existing roots run under the old lock.
- **The creation reservation was atomic but not durable**; it is now made durable before any
  other artifact of creation.
- **Detaching did not quiesce the upgrader's clones**, which share its attachment; the upgrade
  now stops and drains them first.
- **Reader-safe compatibility did not survive completion or successive upgrades**; it is
  recorded cumulatively as the oldest admissible reader schema, and an upgrade unsafe for it
  drains every reader.
- **A restarted store could hand a snapshot an old even count**; state that cannot be vouched
  for reads as odd until recovery.
- **Publishing the count became fallible**; a failed opening permits no effect, and a failed
  closing leaves the count odd and the fence set.
- **Cancellation could not promise reattachment**; it returns with the handle detached, and
  reattaching is a separate admission.
- **Reopening the gate was not a reader-admission protocol**; the gate now has a shared mode for
  ordinary admission, held only until the reader tier, and an exclusive mode for upgrades.
- **The registry's claim and probe rules were underspecified**; entries are taken without
  waiting, listings probe in shared mode, and a full registry leaves a handle unregistered.
- **Diagnosing a blocked upgrade required the attachment it was blocking**; a diagnostic view
  takes no tier.
- **The progress promises exceeded weak fairness**; the assumptions are stated, upgraders are
  served in arrival order, and the model records that writer exclusion across processes is not
  arbitrated, so a writer can be passed over indefinitely.
- **Contended upgrades were tested before independent writers could coexist**; those tests move
  to the writer milestone.

## No fairness in the contract

Settled after the fourth review. The engineer asked whether writes must be served in order;
they need not -- correctness needs only that writes do not overlap, since each write is atomic
and carries its own preconditions. Order among waiters is fairness, and today it comes from an
implementation detail: the in-process writer gate is a ticket queue, while the cross-process root
lock admits one process and refuses the rest. At the level of the contract, exclusion is simply a
wait for access. The engineer removed fairness from the contract altogether as too constraining:
neither writer exclusion nor the admission gate promises any order among waiters, so an upgrade
on a busy store may wait for a lull. The model records the schedules in which a waiter is passed
over indefinitely as a stated limit. The engineer clarified that the contract lacks a fairness
requirement; it does not require a lack of fairness, and an implementation remains free to be
fair.

### Fairness in the model

Prose can say "no fairness is promised"; the specification has to say it exactly. Every wait is
modelled without a queue or ticket, with acquisition enabled for every waiting process whenever
access is free, so safety is checked against every service order. The base specification carries
weak fairness only on releases, on I/O completion and on the disjunction of all acquisitions --
so someone is served when access stays free, but no one in particular. Each per-waiter property is
then checked and expected to fail, and each counterexample is recorded as a named starvation
schedule; a second specification adds strong fairness on each individual acquisition, inherits
every result of the first, and shows what fairness would buy. The engineer wants the model to
squeeze out what the absence of fairness does, and to reinstate any fairness requirement only on
that evidence rather than on intuition[^odi-2].

[^odi-2]: `ODI-2` in [CHECKLIST.md](../CHECKLIST.md)

### Fairness: evidence before requirement

The engineer expects that reasonable contended-write behaviour will likely need some fairness,
but wants evidence of the ramifications first -- including predictions of the contention at
which fairness becomes significant -- and, on the way there, wants the points of conflict
identified and ways found to avoid needing fairness for them. Fairness is a great principle that
also causes terrible problems, so it is to be used with care and possibly only in limited
amounts.

The model checker cannot predict rates: it shows that starvation can happen and how, not how
often. So the evidence is gathered in three layers: the starvation schedules and conflict points
from TLC; predictions from a quantitative model (a queueing baseline, where serving waiters in
random order keeps the mean wait of first-come-first-served but widens its tail, and a seeded
discrete-event simulation of the store's structure, comparing unordered service,
first-come-first-served and bounded bypass); and benchmarks that test the predictions. The
decision then prefers avoiding a conflict to arbitrating it, and the weakest fairness that
addresses the measured problem to first-come-first-served handoff, whose hazards -- convoys,
cross-process handoff cost, priority inversion, a heavier contract -- are why it is a last resort.

## Fifth review of the plan

A fifth read-only review (GPT-6 Astra) raised sixteen findings, all accepted:

- **The format barrier had a race no lock can close**: an older binary that has read the old
  marker but not yet taken the root lock is invisible to a migration, and today's `open` does
  not read the marker again. Migration now requires the root offline as a stated operator
  precondition, and current binaries read the marker again after taking any lock.
- **The publication-record guard would have preceded its barrier**; if that guard is chosen,
  the barrier and migration land with it.
- **Prompt cancellation conflicted with safe draining.** Operations hold leases on the
  attachment; the upgrader drains them while still holding its tiers, refuses to start from
  inside an operation, and cancelling while draining leaves the handle attached.
- **Migration needed a durable commit and resumption protocol**: a migration record, new
  metadata, then the marker published and synced last; coordination state is created untrusted.
- **Cumulative compatibility contradicted the admission table**; admission is now a per-layer
  rule over the oldest admissible reader schema and the current one.
- **A reuser holding the liveness lock before writing its token** reopened the false-live
  window; every change to and inspection of a registry entry now happens under its claim lock,
  and an entry that cannot be claimed is reported as unknown.
- **Registry entries followed the handle, not its attachment**; they now register on attach and
  unregister on detach, with tiers and draining state recorded.
- **The fairness formulation needed precision**: weak fairness is scoped per synchronization
  resource; liveness is stated over individual requests, distinguishing service from
  withdrawal, death and refusal; strong fairness cannot help an exclusive acquirer against
  overlapping shared holders, so a writer-preferring admission variant is checked as an option;
  strong fairness proves eventual service, not a bypass bound; and liveness needs dedicated
  cyclic configurations with deadlock checking on and no symmetry reduction.
- **The quantitative model assumed an in-process queue the contract removed**; unordered threads
  are the baseline and FIFO within a process an implementation option. The queueing result's
  assumptions are stated (stable, work-conserving, non-preemptive M/G/1), the workload is to be
  specified in full, and a budget on the wait shared by CQ-1 and CQ-2 defines "significant".
- **The introduction claimed the on-disk format unchanged**; the document and blob layout is,
  while the root's metadata and the coordination protocol are new.
