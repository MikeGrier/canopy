# CHECKLIST -- formal state model (TLA+) and code conformance

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Formal state model in TLA+". Discussion:
[DESIGN-SESSION-2026-11-10-formal-model.md](design-sessions/DESIGN-SESSION-2026-11-10-formal-model.md).
Completed items: [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M3+ -- After this branch merges

- [ ] **SPEC-2.4** -- Verify the scheduled TLA+ workflow once it is on the default branch, which is
  where GitHub dispatches a workflow from: a manual dispatch runs the probes and the sweep, and a
  second dispatch with nothing changed skips them. Time the first run on the hosted runner, and
  decide with the engineer whether the runner stays hosted or becomes self-hosted, recorded in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "Runs that follow a spec edit are selected by what the edit
  reaches".

## M4 -- Decide and implement the `STRANDED-VOTE` fix from the model

Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Naming the stranded pointer vote
(`STRANDED-VOTE`)" and "Guards against `STRANDED-VOTE`, compared in the model". Acceptance:
[PointerPublication-Settled.cfg](spec/PointerPublication-Settled.cfg) passes.

- [x] **FM-2.1** -- Four candidate guards modelled and checked; resolving before each write and a durable publication record both hold. -> [completed 2026-11-10](COMPLETED-CHECKLIST.md#fm-21)

- [ ] **FM-2.2** -- Choose between the guards that hold with several processes (MP-4.1 to MP-4.5) --
  resolving before each write, or a durable publication record
  ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Guards against `STRANDED-VOTE`, compared in the model")
  -- and implement it in `canopy`. If the publication record is chosen, every operation on a
  document settles a pending record first: precondition checks, logged and unlogged
  publication, collection, repair and scrub. A settle makes the record durable with an fsync
  before it rolls the replicas forward, and one whose fsync fails fails its operation with the
  record still pending, as the model checks ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The publication
  record's live and durable state, modelled in `PointerPublication.tla`"). That section also
  records two findings to decide on: repair and scrub do not need the settle for any property the
  model checks, and a lost pointer replica is restored by the resolve guard's next write and by
  nothing under the record guard but a repair. Because an older
  binary would ignore a pending
  record, the format barrier and the migration of existing roots (the first part of MP-6) move
  into this milestone and land before the record can be written. Add regression tests for the `STRANDED-VOTE`
  sequence followed by warm reopen and by power loss. Make the chosen guard's configuration the
  spec's default, so [PointerPublication-Settled.cfg](spec/PointerPublication-Settled.cfg)
  checks the protocol as built, and change that configuration's header to say every invariant
  and property holds, which `tools/run-tlc-all.ps1` reads.

- [ ] **FM-2.3** -- Distinguish a stable no-majority state from contention in
  `read_document`: sample the modification count before and after the vote, and report a
  distinct outcome when `Store::undisturbed` holds, matching the model's read classification.
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/acadia` -> M1 -> `AC-1`
  > (primitives for canopy's coordination layer). See
  > [CHECKLIST.md](../acadia/CHECKLIST.md#ac-1).

## M5 -- Readers in other processes

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Several processes may open one store".

- [ ] **RG-1** -- The doorbell's spec barrier ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The ring: a
  portable protocol over one platform primitive, behind a contract spec"): `DoorbellContract.tla`,
  stating only what a user observes -- work raised is taken, a waiter with something to take is
  woken, spurious wakes allowed -- and `Doorbell.tla`, the mechanism, each step one atomic
  operation or fence point of the code: the producer's publish, its test of the record of being
  rung and its raise; the consumer's take, reset, clearing of the record, check again and wait;
  and the signal's state. TLC checks that the implementation refines the contract, with controls
  that must fail it: clearing the record before the reset, skipping the check, checking before
  clearing, and a producer that tests before it publishes. Several producers, as a completion
  queue's engine and a coordination notification both ring the engine's doorbell.
  > **CROSS-COMPONENT PREREQUISITE:** `crates/acadia` -> M1 -> `AC-3` (the platform signal)
  > lands before this; see [CHECKLIST.md](../acadia/CHECKLIST.md#ac-3).

- [ ] **RG-2** -- `canopy::ring`'s `Doorbell<S: Signal = PlatformSignal>` over acadia's
  signal, as `Doorbell.tla` specifies it, with the step each line realizes named beside it, the
  completion doorbell's `AsHandle` and `AsFd` by `cfg`, and its pumped `wait`. Decide with the
  engineer, as a toolchain decision, which checker of the Rust memory model runs the protocol over
  a signal it can model (such as `loom`), and run it with each fence removed, which must fail.
  Tests drive scripted interleavings over a scripted signal and check every outcome against
  `DoorbellContract.tla`'s rules, and run the doorbell over each platform's signal from a
  thread pool's wait and the platform's own wait.

- [ ] **NT-1** -- Model the notification in `Coordination.tla` before it is built: a waiting
  operation registers, a change to what it waits on notifies it as a step of its own under weak
  fairness, and the notified operation samples again and decides from what it reads, so the
  hold-ends and admission properties are checked against the notifier rather than assuming every
  waiter sees the change ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A change to the coordination state
  is delivered as a notification", "A client reaches the store through a session and its queue
  pairs"). Behind a switch off in every earlier configuration; a control in which a notification
  can be lost must fail the liveness it breaks, and one that notifies spuriously must leave every
  property holding.

- [ ] **MP-5** -- Define the coordination interface, a layer above `Vfs`, as specified in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "Several processes may open one store", and decide which
  crate holds it. Implement in the file-based implementation, built on `Vfs` primitives so it
  runs over `OsFs` on Windows and Linux, `MemFs` and `FaultyFs` alike:
  - the creation reservation: the creator reads the root and, finding no marker, no reservation
    and nothing that creation does not make, takes its implementation's creation exclusion and
    reads the root again under it, then publishes the reservation -- implementation, every creation
    input, nonce -- with an exclusive publish-if-absent, so it is complete or absent even if power
    is lost, and makes it and its directory entry durable before creation makes any other
    artifact, then makes each artifact durable before the next, the marker last; a mismatched
    open is refused; an open makes the marker durable before it relies on it, since its creator
    may have died between publishing the marker and making it durable; an interrupted creation is
    resumed only through the same implementation, once its creation exclusion is free, after
    reading the root again for a marker, the reservation it decided on and nothing else beneath
    it, and only with identical inputs, completing exactly creation's artifacts in creation's
    order ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Creating a store root, modelled in
    `Creation.tla`");
  - creation exclusion;
  - the admission gate, taken shared by ordinary admission only until the handle holds its
    reader tier and exclusively by an upgrade, with no order among waiters promised; and
    attachment in reader and writer tiers, held shared for a handle's life and
    taken exclusive, released when the holder dies; an upgrade taking the gate, then the reader
    tier when it is unsafe for readers, then the writer tier, never the writer tier first, which
    deadlocks against a handle holding the reader tier shared and waiting for the writer tier
    ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The admission gate and the attachment tiers, modelled in
    `Coordination.tla`");
  - the registry of attached handles: an entry registered when a handle attaches, updated when
    its tiers or draining state change, and removed when it detaches; every change to and
    inspection of an entry made under its claim lock, taken without waiting; a reuser holding
    the claim lock from before it takes the liveness lock until its token is written; a listing
    reporting an entry it cannot claim as unknown, and liveness as of its shared probe; a
    handle that finds every entry held attaching unregistered, recording that in the registry so
    that a listing says it may be incomplete from the record and not from what it sees (a claim
    lock held for a moment sends a registrant on, so an entry can be free when it does), the
    record discarded only by a host restart; registration finishing before the attach returns and
    unregistration starting before the tiers are released, an upgrade's conversion included, an
    entry the owner cannot claim when it leaves left with only its liveness lock released; a dead
    handle's entry never reported as live, and a listing saying when it may be incomplete
    ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The registry of attached handles, modelled in
    `Coordination.tla`");
  - a diagnostic view, taking no tier and granting no document access, offering the registry
    listing, store-level properties and recovery discovery;
  - the modification count, published atomically (a fresh file renamed over the published
    one) so a sample is one lock-free read of a complete value, with a dead holder's odd count
    left odd; a failed opening publication permitting no effect and a failed closing
    publication leaving the count odd and the fence set; and state that cannot be vouched for
    reading as odd until a completed recovery publishes a trusted even value; with a raise
    ordered before the holder's next filesystem effect and a sample ordered with the sampler's
    own reads, a cached sample invalidated synchronously with the publication that outdates it,
    and only the outermost operation under a hold raising it -- the ordering is assumed, not
    checked, by the model ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The modification count, the
    generation and reads, modelled in `Coordination.tla`");
  - a notification when the published count differs from a sampled value or a deadline passes,
    delivered by ringing a doorbell the registrant supplies, so that no waiting work holds a
    thread; the file-based implementation's default is one poller per process sampling with
    backoff only while a registration is outstanding, and the interface leaves room for an
    implementation notified by the OS ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A change to the
    coordination state is delivered as a notification");
  - the fence and the generation, acquisition returning the generation it found before
    advancing it;
  - the ownership record, with the fence set before an abandoned record is replaced and a
    clean release clearing the record before it drops the lock, so a record found under the lock
    always means its holder died ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Writer exclusion and its
    ownership record, modelled in `Coordination.tla`");
  - restart recognition from the OS boot identity on each platform, with state it cannot vouch
    for reported as requiring recovery;
  - abandonment, for tests: releasing a simulated process's holds without clean-release
    bookkeeping.

  Write one conformance suite that every implementation must pass, including cross-process
  cases that spawn real processes, a sampler running while the updating process is suspended or
  killed part-way through publishing the count, simultaneous creation through two
  implementations, a creator suspended before and after publishing its reservation, an
  interrupted creation resumed and one refused for different inputs, a resumer that waited while
  its creation finished, a root holding something creation does not make refused to a creator and
  to a resumer, a creator that dies after making its marker and before making it durable followed
  by an open and a host crash, registry entries reused
  after a death including a reuser paused between taking the liveness lock and writing its
  token, simultaneous listings, an entry removed by a cancelled upgrade's detachment, an attempt to reuse a live entry, the fence surviving the clean release of the
  holder that found an abandoned record, a crash between each step of creation, each count
  publication failing, a sampler paused with the published count open while it is replaced, and
  a sample taken after a restart reading odd.
  > **CROSS-COMPONENT PREREQUISITE:** `crates/acadia` -> M1 -> `AC-1` (the `Vfs` primitives this
  > needs) must land first. See [CHECKLIST.md](../acadia/CHECKLIST.md#ac-1).

- [ ] **RD-1** -- A snapshot that finds a write in progress, or is disturbed by one, is notified
  through the coordination interface when the count changes and tries again, up to a deadline,
  rather than retrying after a yield a fixed number of times, so that a write overlapping a read
  delays it instead of failing it ([DESIGN-NOTES.md](DESIGN-NOTES.md) "How long the contract
  makes a caller wait"). The synchronous snapshot waits on the notification
  ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A change to the coordination state is delivered as a
  notification"); the form that holds no thread is a snapshot submitted through a queue pair, in
  API-3. The deadline is a store option with a default no shorter than a write's tail hold;
  `SnapshotContended` reports a deadline that passed. Tests show a snapshot that overlaps a write
  succeeds once the write ends, and one held off past its deadline failing.

- [ ] **RD-2** -- A Windows implementation of the notification notified by the OS instead of
  polling -- an opportunistic lock on the published count requested with overlapped I/O, broken
  when a fresh count is renamed over it, and delivered from an I/O completion -- with the poller
  kept as the fallback where the lock is not granted, as on file systems that do not support it
  ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A change to the coordination state is delivered as a
  notification"). First confirm that a rename over the published count breaks the lock. The
  conformance suite's notification cases run against both, and a test shows a registration woken
  by the break with no thread polling or blocked.

- [ ] **MP-6** -- Unless FM-2.2 has already landed them, advance the genesis format version as
  the barrier that makes an older binary refuse the root, and add the one-way migration of an
  existing root: stated as requiring the root offline -- no older binary running against it or
  starting to open it; taking the old root lock and reading the marker again under it; durably
  recording a migration record of its identity and inputs; writing the implementation record
  and the root's other new metadata durably; publishing the new marker atomically and syncing
  it last; creating coordination state untrusted; and resuming an interrupted migration only
  with identical inputs. Make every current binary read the marker again after taking any lock.
  Refuse an unmigrated root with a distinct error naming the migration. Test an older binary
  refusing a migrated root, a migration interrupted at each step and resumed, a resume refused
  for different inputs, and a current binary paused between reading the marker and taking the
  lock while a migration runs. Split `Store::open` into attaching, which changes no store data, and explicit
  recovery, which runs only under exclusion (in this milestone, the lifetime root lock) and
  includes directory-sync debt discharge. Recovery's public API: a discovery call that reports
  whether recovery is required and why, treating work a live holder owns as not requiring it,
  without mutating anything, and making its observation of the fence, the restart state, the
  ownership record and the lock consistent -- by trying the writer lock without waiting and
  reading under it, for example -- so that a live holder's transaction is never reported as
  abandoned while its holder acquires or releases; an incremental run whose steps each do bounded,
  idempotent work and report progress (phase, units done of units known, and whether the known
  total grew); resumption by a later recovery after one is stopped or its process dies; and a
  convenience that runs to completion. Recovery reads the commit log and never the coordination
  state; rolls a prepared transaction forward through its swaps and its end record, and one that
  was begun and not prepared back by sweeping its staging before it resets the log; publishes the
  even count and the trusted state in one publication that also drops cached samples; and clears
  the fence last, so that a stop or a death at any step leaves the fence set
  ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Recovery and its discovery, modelled in
  `Coordination.tla`"). While recovery is required, apply the permission table in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "Several processes may open one store": writes, repairs,
  scrub, resets and upgrades are refused with `StoreError::RecoveryRequired`, and none of them
  replays anything; callers that relied on `open` recovering move to the explicit calls. A
  handle opened only to read cannot write. Move the modification count into the coordination
  state so a reader classifies contention against a writer in another process, including the
  partial state a dead writer left before anything recovers it. Cross-process tests spawn a real
  second process, on Windows and on Linux under WSL.

- [ ] **MP-7** -- Record the agent and the schemas a root names, as
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "Several processes may open one store" specifies. `init`
  and `open` take an agent identifier (version-independent part and version), an agent
  description, and the agent-schema version the agent speaks. Refuse an open whose
  version-independent part differs from the recorded one, or whose schemas the per-layer
  admission rules do not admit, checked after attaching. Record the version of the
  agent whose write succeeded, replaced after a successful write by a handle of a different
  version under the same hold; an open changes nothing. Each such change is a durable
  replacement under exclusion, not a transaction, and never changes the write's outcome: a
  failed write never stamps, and a failed stamp is reported alongside the write's success. Add:
  - a schema upgrade following the decision's protocol: every operation on a clone holding a
    lease on the attachment; the upgrader draining those leases while still holding its tiers,
    refusing to start from inside an operation, and cancellable while draining with the handle
    left attached; the refusal decided by the caller's own operation context and not by whether
    the handle has any lease, so that the leases of other clones are waited for
    ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Clones, leases and the upgrade drain, modelled in
    `Coordination.tla`"); then releasing its tiers, taking the admission gate, acquiring its tiers
    exclusively and checking the schemas again, a cancelled or failed conversion returning
    promptly with the handle detached, every operation on a detached handle refused until it
    attaches again, as an ordinary admission, or is dropped once its clones' operations have
    ended; the gate reopened to readers during a reader-safe
    upgrade; the per-layer admission rules applied to every layer of every attaching handle; an
    upgrade stating the oldest reader schema that can read what it leaves, which canopy judges
    against the layer's oldest admissible reader schema and never against the current one,
    recording the judgment in the intent with the layer, the floor and the target, so that one
    that some admissible reader schema cannot read takes the reader tier exclusively, draining
    every reader before it begins, and sets the layer's oldest admissible reader schema to its
    target when it completes, while a safe one leaves it alone ([DESIGN-NOTES.md](DESIGN-NOTES.md)
    "Cumulative reader compatibility, modelled in `Coordination.tla`"); the body taking writer
    exclusion after its tiers and refused, taking nothing, when it finds recovery required, then
    inside the count's bracket recording a durable intent naming the layer and its source and
    target schemas before its first step, making idempotent resumable steps that each take what
    they need from the intent and the progress durable state shows and that report progress,
    advancing the schema durably only after the last step and clearing the intent only after
    that; a failure or a cancellation in the body setting the fence and leaving the intent; and
    an interrupted upgrade resumed as the store's recovery, before the rest of it, by a handle
    speaking the target schemas, no handle that speaks only the source schemas writing while the
    intent stands ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The schema upgrade's body, modelled in
    `Coordination.tla`");
  - a management call that changes the description, the start of a surface for store-level
    settings;
  - a call that lists the attached handles from the registry, saying when the list may be
    incomplete, available both on a handle and through the diagnostic view;
  - the call that reports store-level properties: the agent identifier and description, the
    coordination implementation, the canopy-format and agent-schema versions, the oldest
    admissible reader schema, the identity and creation time, the genesis format version, and
    the replica counts and shard width.

  Test an upgrade interrupted at each step and resumed, a cancellation while draining leaving
  the handle attached, a cancelled conversion leaving the handle detached, a clone of the
  upgrader refused during the upgrade, an operation in flight on a clone finishing before the
  upgrade proceeds, an upgrade started from inside a callback refused while one started beside
  another clone's operation waits for it, readers of both versions
  attaching during a reader-safe upgrade, two reader-safe upgrades in a row with a reader of the
  first source still attached, an upgrade that an older admissible reader schema cannot read
  waiting for a reader of that schema to leave and refusing it afterwards, an upgrade of one layer
  beside handles that speak another, writers of versions A, B and A again each leaving their version
  recorded, a failed write leaving it unchanged, and a stamp whose sync fails after its rename.
  Upgrades contended between processes are tested in M6, once independent writers can coexist.
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/acadia` -> M2 -> `AC-2` (a
  > blocking exclusive lock). See [CHECKLIST.md](../acadia/CHECKLIST.md#ac-2).

## M6 -- Writers in several processes take turns

- [ ] **MP-8** -- Implement the contract's writer exclusion in every implementation: across
  every thread of every process, waiting, with no order among waiters promised, released when the
  holder dies, with the ownership record telling the next acquirer so and a clean release
  clearing it before it drops the lock ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Writer exclusion and
  its ownership record, modelled in `Coordination.tla`"); extend the conformance suite to cover
  it, including a release that overlaps another acquirer, on Windows and Linux.
  > **CROSS-COMPONENT PREREQUISITE:** `crates/acadia` -> M2 -> `AC-2` (a blocking exclusive
  > lock) must land first. See [CHECKLIST.md](../acadia/CHECKLIST.md#ac-2).

- [ ] **MP-9** -- Replace the in-process writer gate and the root lock with the contract's
  exclusion, acquired once by the outermost write and by `init`, with callbacks receiving the
  read-only view WS-2 decided. When exclusion reports that the previous holder died holding it,
  report `StoreError::RecoveryRequired` and replay nothing; recovery runs only through MP-6's
  API, under its own capability, keeping the dead holder's count odd until it finishes and
  clearing the fence only after it succeeds. Reload each handle's commit-log durability
  knowledge, directory-sync debt state and recorded agent version whenever the generation has
  moved since that handle last held exclusion, including for two handles opened in one process.
  Stop holding the root exclusively for a handle's lifetime, and retire `D116` in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) once nothing relies on it.

- [ ] **MP-10** -- Cross-process integration tests, on Windows and on Linux under WSL: writer
  processes interleaving writes and transactions; a writer killed while holding exclusion
  before its first effect, mid-publication, mid-transaction and after its last effect; the
  survivors refused with `RecoveryRequired` until an explicit recovery converges the store; a
  recovering process killed after each of its steps -- the first swap, the swaps, the end record,
  the sweep, the count's publication -- and another process resuming its recovery; discovery
  called beside a live holder's transaction, which it never reports as abandoned, and beside a
  dead holder's, which it always does; a schema
  upgrade waiting for attached writers to leave, and one that is unsafe for readers waiting for
  attached readers of any schema to leave, with the registry naming them and never listing
  a killed process as attached, and new handles waiting while it holds the gate; an upgrader killed
  after its intent, between its steps, after its last step and after the schema advanced, and its
  upgrade resumed by a writer that speaks the target schema while a writer that speaks only the
  source schema is refused; an upgrader that finds recovery required refused, taking nothing; a
  failure injected in the body leaving the fence and the intent; two upgraders in different
  processes at once,
  without deadlock; a commit-log reset never overlapping another process's
  transaction; two handles opened in one process taking turns; and an open through a different
  coordination implementation refused.

- [ ] **CQ-2** -- Benchmark contended writes and upgrades across processes on Windows and Linux
  at the utilisations and with the workloads CQ-1 studied, measuring the wait-time distribution
  and how often a waiter is passed over, and compare them with CQ-1's predictions against the
  same budget on the wait. Record the comparison in
  [DESIGN-NOTES.md](DESIGN-NOTES.md); a prediction the measurements contradict reopens ODI-2's
  decision.

- [ ] **MP-11** -- A shared-memory implementation of the whole coordination contract --
  exclusion, attachment, the registry and the counters -- for stores on a local filesystem on
  Windows and Linux, passing the same conformance suite. Specify how it creates and re-creates
  its state (a Windows mapping backed by the paging file vanishes when its last user exits),
  treating any state it cannot vouch for as requiring recovery, and how it simulates
  abandonment. Test last-process exit, host restart and a failed creation separately, and
  benchmark reader sampling and writer handoff against the file-based implementation.
  > **CROSS-COMPONENT HANDOFF:** this work lives with the coordination interface, in the crate
  > MP-5 chose.

## M7 -- Conformance harness

- [ ] **FM-3.1** -- Export TLC behaviours as JSON traces (simulation mode for breadth, the
  error trace for counterexamples) and check a corpus into the crate's test data. The corpus
  includes every timeline's replay from SPEC-2.2's scenario tables, so that FM-3.2 replays the
  timelines against the code ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A timeline is replayed by TLC
  from a scenario table").

- [ ] **FM-3.2** -- A Rust test that replays each trace step against `Store` on
  `MemFs` + `FaultyFs` (failures as injected faults, crash as `simulate_crash`, reopen as
  `Store::open`, one `Store` per model process, a process's death as the coordination
  implementation's abandonment of that process's holds -- never a clean drop, which would
  release ownership and restore the count's parity) and after every step compares a projection of the store's live and durable
  state with the trace's model state.

- [ ] **FM-3.3** -- Prove the harness bites: remove the namespace sync that collection performs
  before deleting, and confirm the replay of the `DANGLING-DURABLE` counterexample trace fails at
  the step the model predicts.

## M8 -- Extend the protocol model

- [ ] **FM-4.1** -- The rest of transactions beyond what M3 models. `Coordination.tla` models a
  transaction's records in the commit log, its staging as one unit that is swept, its swaps of
  the namespace, the rollback of one that fails before its prepare record, and recovery's
  roll-forward and roll-back ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A transaction and the commit
  log, modelled in `Coordination.tla`" and "Recovery and its discovery, modelled in
  `Coordination.tla`"). What is left is the content beneath those: the collection of what a
  transaction ingested, and how rollback and the sweep of its staging act on it.

- [ ] **FM-4.2** -- Node paths and node removal: documents and nodes as one namespace tree,
  the collision rules (a write against a node removal in the same set, a node removal at or
  beneath an existing document), and the directory-intent record (`.dir-sync-debt`) with
  torn appends.

- [ ] **FM-4.3** -- Repair and scrub, including the modification count a repair holds odd
  while it runs.

## M9 -- Filesystem reference model

- [ ] **FM-5.1** -- A TLA+ reference model of the durability semantics `MemFs` implements
  (directory entries and their sync state, file data and its sync state, rename, crash),
  checked against `MemFs` with the M7 harness. `MemFs`'s recent defects are the first cases it
  must reproduce: one inode left under two names by a crash, a directory replacing a durable
  file, a moved or nested directory inside a carried snapshot, and an unnamed directory beneath
  one.
  > **CROSS-COMPONENT HANDOFF:** this work lives in component `crates/acadia`; its
  > checklist is created when this item starts and the item moves there.

## M10 -- The model clients rely on

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "The model clients rely on, and the model the code is
held to". Discussion:
[DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md](design-sessions/DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md).
These items follow the primary deliverable, the milestones above: what is promised rests on ODI-2's
decision about fairness and on what MP-4 and CQ-1 find, the refinement is written against the state
MP-3 and MP-4 change, the oracle for tests at the API uses M7's harness, and the documentation for
dependents describes the multi-process behaviour of M6. CM-1, which needed none of them and steers
MP-3 and MP-4, was done in M3.

- [ ] **API-2** -- Transactions built without exclusion and submitted once
  ([DESIGN-NOTES.md](DESIGN-NOTES.md) "No caller code runs under exclusion but the commit
  observer"): an operation that only checks a document's digest or absence, beside writes and
  removals; a builder the store hands out that records each document it reads as such a check and
  holds no lock; `Store::transact`, taking the operation list and an options value that is empty
  today; and `Store::transform` as the helper that runs a closure against a fresh builder without
  the lock and retries a commit rejected by a changed read, up to a bounded number of attempts.
  The closure form of `transform` is retired with it.

- [ ] **QS-1** -- The ring's spec barrier ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The ring: a
  portable protocol over one platform primitive, behind a contract spec"): `RingContract.tla`,
  stating what a client observes, and `Ring.tla`, the mechanism, which uses the doorbell only
  through `DoorbellContract.tla` and is checked by TLC to refine the contract. Together they
  model a session's queue pairs and their two doorbells, independent of the coordination model: a push that only queues and a submission doorbell rung
  for a batch; submission reserving a completion slot and refused when none is free; the engine as
  the one consumer of submissions and producer of completions, woken by its doorbell; an entry
  started in order within its pair; the completion doorbell rung on empty to not empty and armed
  by the client's take, clear, check again, wait; a pair dropped by its client at any point; and
  the engine's work on an entry either withdrawn before it starts or run to completion.
  Properties: every accepted entry gets exactly one completion while its pair exists; posting a
  completion is never blocked; a pair's entries take effect in submission order; an entry
  withdrawn by a drop never takes effect, and one started does; a client waiting on the completion
  doorbell is never left waiting with a completion queued, nor the engine with a submission rung
  for ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A client reaches the store through a session and its
  queue pairs"). Controls: submission without a reservation, which must overflow; a drop that
  cancels a started entry, which must break the atomicity property; an arming that skips the
  check after clearing, and one that checks before clearing, each of which must leave a client
  waiting past a completion.

- [ ] **API-3** -- Sessions and queue pairs as QS-1 specifies them, through which all work is
  submitted, with API-2's transactions, reads, snapshots and enumerations as entries: a session
  acquired on a store, pairs acquired from it, ends `Send` and not `Sync` with `&mut` push and pop,
  completions reserved at submission, a submission doorbell rung by the client for a batch, a
  completion doorbell that is a waitable object on each platform, and the engine setting aside a
  waiting entry with a registration on the coordination notification, which rings the engine's
  doorbell, so that no waiting holds a thread ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A client
  reaches the store through a session and its queue pairs"). The synchronous methods become a
  convenience that submits one entry and waits on the completion doorbell. The queues are
  `canopy::ring`'s, generic over the signal as RG-2's doorbell is, following
  [durable-ioring](https://github.com/MikeGrier/windows-threadpool-sys/tree/main/crates/durable-ioring)'s
  ring, with the step of `Ring.tla` each line realizes named beside it, and checked by RG-2's
  memory-model checker. Tests: exactly one
  completion per entry under many pairs, per-pair order, a full completion queue refusing a
  submission, a pair dropped with entries unstarted and started, a client waiting on the
  completion doorbell from a thread pool's wait, many waiting entries holding no engine thread,
  and sabotages that reverse the arming protocol's clear and check, and the doorbell's reset and
  flag, each of which must hang.

- [ ] **API-1** -- Withdraw `Store::plan_and_apply` from the public API, moving its callers to
  API-2's transactions, whose checks stand for what a plan read, so that ODI-5 decides what canopy
  advertises from a surface without it ([DESIGN-NOTES.md](DESIGN-NOTES.md) "No caller code runs
  under exclusion but the commit observer").

- [ ] **ODI-6** -- Decide which primitives the store evaluates under exclusion -- operations
  computed from a document's current value, such as add, maximum and append, which never
  conflict -- from where CQ-1 shows read-modify-write retries failing too often, with the bound on
  each one's cost, and whether canopy owns a body format so that a primitive can work on more than
  bytes ([DESIGN-NOTES.md](DESIGN-NOTES.md) "No caller code runs under exclusion but the commit
  observer"). Record the decision; each primitive adopted becomes a work item here.

- [ ] **ODI-5** -- Decide what canopy advertises to the code that depends on it. From CM-1's
  client-visible guarantees and the results of MP-4, CQ-1 and ODI-2, decide which guarantees are
  promised and which are explicitly not -- an order or a bound on waiting, the outcome of a write
  whose holder was told recovery is required until recovery has run, a networked filesystem --
  and how the promise is versioned, since a change to it is a change a dependent can feel.
  Decide how it is delivered: prose in the crate documentation, a TLA+ module a dependent can
  compose with, or both. A promise is a commitment, so this is the engineer's decision; record it
  in [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] **CM-2** -- The client model: a small TLA+ module over what a `Store` user can observe --
  the logical contents, which handles are attached and in what role, each request's outcome,
  whether the store needs recovery -- and none of the mechanism: no count, fence, generation,
  commit log or lock order. Its invariants are ODI-5's promised guarantees in a user's terms, and
  it promises no more, so the implementation keeps the freedom the promise leaves. It follows
  the conventions of the other models: controls that reverse a promise and must fail, witnesses
  that show each outcome a caller can meet is reachable, and configurations whose headers state
  the outcome [run-tlc-all.ps1](../../tools/run-tlc-all.ps1) must find.

- [ ] **CM-3** -- Check that the code's model refines the client model. For each form of the
  coordination model that is kept -- the abstract and the detailed write and upgrade -- write in
  the spec the mapping from its state to the client model's, and have TLC check in its main
  configurations that every behaviour projects onto a behaviour of the client model. The mapping
  is where the commit point is written down: when a write counts as having happened, including
  when its holder was told it failed. The probes at the milestone boundary include mutants that
  break a promise, and each must fail the refinement.

  Verify: the refinement holds in each main configuration, and a control that breaks a promise
  fails it.

- [ ] **FM-3.4** -- Check `Store` against the client model at the API: sequences of calls on
  `MemFs` + `FaultyFs`, with injected faults, crashes and several `Store`s over one root, and the
  client model as the oracle -- every outcome the code returns must be one the model allows from
  the state its earlier calls put it in. FM-3.2 compares internal state step by step; this
  compares only what a user can see, so it keeps passing through any change of mechanism that
  CM-3's refinement still admits.

- [ ] **CM-4** -- Tell dependents how to use canopy reliably. From ODI-5's decision and the
  client model, write in the crate documentation the contract as a user relies on it and a guide
  to acting on each outcome: what a caller does when told recovery is required or a read is
  disturbed, how to learn whether a write whose outcome is unknown took effect and how to retry
  it safely, what waiting to expect under contention, and what is not promised. Each statement
  names the property of the client model it rests on, so a reader can follow a promise to its
  check.

## M10+ -- TLA futures

Work on how the models are checked that waits until the milestones above are done.

- [ ] **SPEC-3** -- Generate mutants of a spec mechanically, to find the rules no property checks.
  The probe corpus under `spec/probes/` is written by hand, one mutant per rule someone named, so
  it shows whether each named rule's property can fail and says nothing about a rule nobody
  named. Build in `tools/` a generator that applies a small set of operators to every action of a
  spec -- drop a conjunct, negate a guard, leave a primed variable unchanged, widen a set
  membership -- and runs each mutant against the configurations that reach the action, the way
  `cargo mutants` is run over code. A mutant no configuration catches is triaged into a new
  property, a new scenario or bound that exposes it, or a recorded equivalent, and the triage is
  what the pass is for. Deferred by the engineer on 2026-11-11 until the milestones above are
  done: building the generator is substantial work, and a full pass is expected to take days of
  TLC time, which the current work cannot spare. When it is taken up, its runs belong with
  SPEC-2's scheduled full run, scoped by SPEC-2's impact analysis.

  Verify: a spec edit that removes a guard checked by no property yields a surviving mutant, and
  one checked by a property does not.
