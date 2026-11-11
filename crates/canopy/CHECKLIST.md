# CHECKLIST -- formal state model (TLA+) and code conformance

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Formal state model in TLA+". Discussion:
[DESIGN-SESSION-2026-11-10-formal-model.md](design-sessions/DESIGN-SESSION-2026-11-10-formal-model.md).
Completed items: [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M3 -- Model several processes

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Several processes may open one store".

- [x] **ODI-1** -- Writers of different versions: schema versions with one-way exclusive upgrades, attachment in the contract, and a diagnostic registry of attached handles. -> [completed 2026-11-10](COMPLETED-CHECKLIST.md#odi-1)

- [ ] **MP-1** -- Extend [PointerPublication.tla](spec/PointerPublication.tla) with a set of
  processes that interact only through the coordination contract:
  - the creation reservation, published complete or not at all and made durable before any other
    artifact, by a creator already holding its implementation's creation exclusion, and an
    interrupted creation resumed only with identical inputs;
  - the admission gate in its shared (ordinary) and exclusive (upgrade) modes, and attachment in
    its reader and writer tiers, acquired in the contract's order, with no handle waiting for the
    gate while it holds a tier, and the per-layer admission rules over `oldest` and `current`;
  - a handle's clones sharing its attachment, each operation holding a lease on it; an upgrade
    draining the leases while still holding its tiers, refusing to start from inside an
    operation, and cancellable while draining with the handle left attached; and a conversion
    cancelled or failed after the drain leaving the handle detached;
  - writer exclusion with its ownership record, the fence set before an abandoned record is
    replaced;
  - the modification count bracketing whole logical operations, left odd by an unresolved one,
    and published atomically -- each sample modelled as an individual read, so that a control
    which assembles a sample from separately written slots is refuted; a failed opening
    publication with no effect, a failed closing publication leaving the count odd and the fence
    set, and state that cannot be vouched for reading as odd until recovery;
  - the registry's claim and liveness locks, every change to and inspection of an entry made
    under its claim lock taken without waiting, entries following attachment rather than the
    handle's life, and a listing never reporting a dead handle live;
  - the fence and the generation, with each handle's caches kept only when the generation it
    finds is the one it last advanced to;
  - every wait -- writer exclusion, the admission gate, the attachment tiers -- with no queue or
    ticket in the state: whenever access is free, acquisition is enabled for every waiting
    process and the checker chooses among them, so every safety invariant is checked against
    every service order; and every wait as an individual request, which ends served, withdrawn,
    dead with its process, or refused as ineligible.

  Each process keeps its own execution state. A process's death is distinct from a host crash:
  it loses only that process's state, leaves the live filesystem as it was, and abandons its holds
  rather than releasing them cleanly. Death may happen at any step, including before a holder's
  first effect, mid-publication, after its last effect but before release, between taking a
  registry entry's claim lock and its liveness lock, while publishing the count, and while an
  upgrade drains its clones' leases. Opening
  attaches without changing store data; acquiring exclusion after a holder died sets the fence,
  reports that recovery is required, and recovers nothing. Readers in any process take
  sequential samples and classify with them; an optimistic snapshot taken between two swaps of
  one transaction, or after a transaction failed part-way, must not be accepted. Add
  configurations with two and three processes, and a control in which a cached sample is
  invalidated late (an unacknowledged lease break), which must violate the read classification.

- [ ] **MP-2** -- Model the part of transactions that processes interact through: `begin` /
  ingest / `prepare` / namespace apply / `end`, the commit-log reset (refused while the log holds
  unresolved work), the fence set by an ambiguous failure, and recovery's roll-forward under its
  own capability (the fence refuses ordinary writes, not recovery, and is cleared only after
  recovery succeeds). Work counts toward recovery only once abandoned; discovery while a live
  holder is mid-transaction reports it as not requiring recovery. Recovery runs as idempotent
  steps that a stopped or dead recoverer leaves for the next one to resume. Model the schema
  upgrade the same way: the upgrader releasing its tiers, then taking the gate, then the tiers
  exclusively, and checking the schemas again; the gate reopening to readers during a
  reader-safe upgrade; the per-layer admission rules, with each completion setting `oldest` and
  a later upgrade unsafe for any schema from `oldest` up draining every reader first; the
  durable intent; resumable steps; the version advanced only after the last step; and an
  interrupted upgrade resumed as the store's recovery by a handle speaking the target schemas. Include a holder dying with a transaction in flight while another
  process is attached, a recoverer dying between steps, an upgrader dying mid-migration, two
  upgraders at once, a cancelled conversion, an upgrader waiting while an attached writer waits
  for exclusion, a clone of the upgrader trying to write during the upgrade, and two reader-safe
  upgrades in a row with a reader of the first source still attached.

- [ ] **MP-3** -- Refine the durable publication record in the model: separate its live and
  durable state, so writing and clearing it are a mutation followed by an fsync that can fail,
  and mix logged with unlogged writes on one document, settling a pending record before every
  precondition check, publication, collection and repair. Include a failed clear followed by
  another operation and then power loss.

- [ ] **MP-4** -- Check both guards that hold with one process (resolve before each write; a
  durable publication record) and every existing invariant with several processes, and check
  that every condition that sets the fence is recoverable from durable state, so a host restart
  -- which forces recovery -- loses nothing the fence knew. Then squeeze out what the absence of
  a fairness requirement does. Liveness needs its own configurations: a finite-state, cyclic
  workload in which processes keep issuing writes, admissions and upgrades -- the existing
  bounded-attempt configurations stop contention after a few writes -- with deadlock checking
  on and no symmetry reduction. Two specifications:
  - `Spec` carries only the fairness the contract guarantees or assumes: weak fairness on each
    holder's release and on I/O completion (the stated assumptions that every handle eventually
    detaches and every I/O eventually succeeds), and, for each synchronization resource
    separately -- writer exclusion, each mode of the admission gate, each attachment tier --
    weak fairness on the disjunction of that resource's acquisitions, so that when it stays
    available some waiter for it is served, with no fairness on any individual acquisition.
    Under it, check that the store as a whole makes progress, that an upgrade holding its tiers,
    not cancelled and whose process survives completes, and that a driven recovery completes;
    and check, for each kind of request -- a writer's, an upgrader's, an attaching handle's --
    that a request which persists and stays eligible is eventually served, expecting each to
    fail. A request withdrawn, killed or refused is not starved. Record every counterexample as
    a named starvation schedule: who is passed over, by whom, and what that leaves undone (an
    upgrade that never starts, a recovery never driven, a writer never served). From the
    schedules, list every point of conflict -- which waits contend with which holds -- as the
    input to CQ-1 and ODI-2.
  - `FairSpec` adds strong fairness on each individual acquisition. Every behaviour of
    `FairSpec` is one of `Spec`, so it inherits every safety and store-progress result; check
    which per-request properties then hold, which shows what fairness can buy. Strong fairness
    cannot help an exclusive acquirer that is never enabled because overlapping shared holders
    keep a resource continuously occupied, so check the upgrader's property separately under a
    variant that withholds shared admission while an exclusive waiter is pending -- an option
    for ODI-2, not a contract rule. `FairSpec` proves eventual service, never a bound on how
    often a waiter is passed over. Run it on the small configurations, since strong fairness is
    costly to check.

  Use temporal properties for the stuttering controls, alongside deadlock negative controls
  that must fail -- an upgrader waiting for the gate while holding a tier. Record the results in
  [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] **CQ-1** -- Build a quantitative model of contention for the points of conflict MP-4
  lists:
  - a queueing baseline for a single exclusion, stated with the assumptions its results need --
    stable, work-conserving, non-preemptive M/G/1 with service times independent of the order
    of service and of finite variance -- and applied only where they hold;
  - a seeded discrete-event simulation (in Rust, using the seeded generator the tests use) of
    the store's structure, with waiters unordered among threads and processes alike as the
    contract allows, first-come-first-served within a process compared as an implementation
    option, upgrades waiting at the admission gate behind a stream of admissions, and recovery.

  Specify the workload before running it: the arrival processes (Poisson and bursty), the
  hold-time distributions, the process and thread counts, attachment lifetimes, the frequency
  of upgrades, and for unordered service the probability law by which a waiter is chosen and
  how often an arriving thread barges ahead; with warm-up and confidence intervals stated.
  Agree with the engineer, before the simulation runs, a budget on the wait that both CQ-1 and
  CQ-2 use -- such as the probability of waiting longer than a deadline, or a high percentile.
  Predict, for each point of conflict, the tail of the wait and the distribution of times a
  waiter is passed over, under unordered service, first-come-first-served service and bounded
  bypass, and the contention at which unordered service breaches the budget while FIFO meets
  it. Record the predictions, the workload and the method in
  [DESIGN-NOTES.md](DESIGN-NOTES.md). If the simulation cannot answer a question exactly
  enough, raise a probabilistic model checker (PRISM or Storm) as a toolchain decision rather
  than adopting one silently.

- [ ] **ODI-2** -- From MP-4's starvation schedules and conflict points and CQ-1's predictions,
  decide, for each point of conflict, first whether the conflict can be avoided -- shorter
  holds, finer-grained exclusion, keeping readers out of the wait -- and only where it cannot,
  whether any fairness returns to the contract, choosing the weakest form that addresses the
  predicted problem (for example a bound on how many times one waiter may be passed over)
  rather than first-come-first-served handoff, with `FairSpec`'s result for that property as
  the evidence. A numeric bound such as bounded bypass is adopted only after its bookkeeping has
  been modelled and the bound itself checked, since `FairSpec` proves only eventual service. The
  contract lacks a fairness requirement today on the engineer's direction; reinstating one is to be driven by the evidence, not by intuition. Record the decision in
  [DESIGN-NOTES.md](DESIGN-NOTES.md); avoidance or a fairness requirement adopted becomes work
  in MP-5, MP-8 and MP-11, and the prediction it rests on is tested by CQ-2.

## M4 -- Decide and implement the `STRANDED-VOTE` fix from the model

Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Naming the stranded pointer vote
(`STRANDED-VOTE`)" and "Guards against `STRANDED-VOTE`, compared in the model". Acceptance:
[PointerPublication-Settled.cfg](spec/PointerPublication-Settled.cfg) passes.

- [x] **FM-2.1** -- Four candidate guards modelled and checked; resolving before each write and a durable publication record both hold. -> [completed 2026-11-10](COMPLETED-CHECKLIST.md#fm-21)

- [ ] **FM-2.2** -- Choose between the guards that hold with several processes (M3) --
  resolving before each write, or a durable publication record
  ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Guards against `STRANDED-VOTE`, compared in the model")
  -- and implement it in `canopy`. If the publication record is chosen, every operation on a
  document settles a pending record first: precondition checks, logged and unlogged
  publication, collection, repair and scrub; and because an older binary would ignore a pending
  record, the format barrier and the migration of existing roots (the first part of MP-6) move
  into this milestone and land before the record can be written. Add regression tests for the `STRANDED-VOTE`
  sequence followed by warm reopen and by power loss. Make the chosen guard's configuration the
  spec's default, so [PointerPublication-Settled.cfg](spec/PointerPublication-Settled.cfg)
  checks the protocol as built.

- [ ] **FM-2.3** -- Distinguish a stable no-majority state from contention in
  `read_document`: sample the modification count before and after the vote, and report a
  distinct outcome when `Store::undisturbed` holds, matching the model's read classification.
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/acadia` -> M1 -> `AC-1`
  > (primitives for canopy's coordination layer). See
  > [CHECKLIST.md](../acadia/CHECKLIST.md#ac-1).

## M5 -- Readers in other processes

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Several processes may open one store".

- [ ] **MP-5** -- Define the coordination interface, a layer above `Vfs`, as specified in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "Several processes may open one store", and decide which
  crate holds it. Implement in the file-based implementation, built on `Vfs` primitives so it
  runs over `OsFs` on Windows and Linux, `MemFs` and `FaultyFs` alike:
  - the creation reservation: the creator takes its implementation's creation exclusion, then
    publishes the reservation -- implementation, every creation input, nonce -- with an
    exclusive publish-if-absent, so it is complete or absent, and makes it and its directory
    entry durable before creation makes any other artifact; a mismatched open is refused; an
    interrupted creation is resumed only through the same implementation, once its creation
    exclusion is free, after re-reading the reservation, and only with identical inputs,
    completing exactly creation's artifacts in creation's order;
  - creation exclusion;
  - the admission gate, taken shared by ordinary admission only until the handle holds its
    reader tier and exclusively by an upgrade, with no order among waiters promised; and
    attachment in reader and writer tiers, held shared for a handle's life and
    taken exclusive, released when the holder dies;
  - the registry of attached handles: an entry registered when a handle attaches, updated when
    its tiers or draining state change, and removed when it detaches; every change to and
    inspection of an entry made under its claim lock, taken without waiting; a reuser holding
    the claim lock from before it takes the liveness lock until its token is written; a listing
    reporting an entry it cannot claim as unknown, and liveness as of its shared probe; a
    handle that finds every entry held attaching unregistered; a dead handle's entry never
    reported as live, and a listing saying when it may be incomplete;
  - a diagnostic view, taking no tier and granting no document access, offering the registry
    listing, store-level properties and recovery discovery;
  - the modification count, published atomically (a fresh file renamed over the published
    one) so a sample is one lock-free read of a complete value, with a dead holder's odd count
    left odd; a failed opening publication permitting no effect and a failed closing
    publication leaving the count odd and the fence set; and state that cannot be vouched for
    reading as odd until a completed recovery publishes a trusted even value;
  - the fence and the generation, acquisition returning the generation it found before
    advancing it;
  - the ownership record, with the fence set before an abandoned record is replaced;
  - restart recognition from the OS boot identity on each platform, with state it cannot vouch
    for reported as requiring recovery;
  - abandonment, for tests: releasing a simulated process's holds without clean-release
    bookkeeping.

  Write one conformance suite that every implementation must pass, including cross-process
  cases that spawn real processes, a sampler running while the updating process is suspended or
  killed part-way through publishing the count, simultaneous creation through two
  implementations, a creator suspended before and after publishing its reservation, an
  interrupted creation resumed and one refused for different inputs, registry entries reused
  after a death including a reuser paused between taking the liveness lock and writing its
  token, simultaneous listings, an entry removed by a cancelled upgrade's detachment, an attempt to reuse a live entry, the fence surviving the clean release of the
  holder that found an abandoned record, a crash between each step of creation, each count
  publication failing, a sampler paused with the published count open while it is replaced, and
  a sample taken after a restart reading odd.
  > **CROSS-COMPONENT PREREQUISITE:** `crates/acadia` -> M1 -> `AC-1` (the `Vfs` primitives this
  > needs) must land first. See [CHECKLIST.md](../acadia/CHECKLIST.md#ac-1).

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
  without mutating anything; an incremental run whose steps each do bounded, idempotent work and
  report progress (phase, units done of units known, and whether the known total grew);
  resumption by a later recovery after one is stopped or its process dies; and a convenience
  that runs to completion. While recovery is required, apply the permission table in
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
    left attached; then releasing its tiers, taking the admission gate, acquiring its tiers
    exclusively and checking the schemas again, a cancelled or failed conversion returning
    promptly with the handle detached; the gate reopened to readers during a reader-safe
    upgrade; the per-layer admission rules applied to every attaching handle, with each
    completion setting the oldest admissible reader schema and an upgrade unsafe for any schema
    from it up draining every reader first; a durable intent; resumable steps reporting
    progress; the version advanced only after the last step; and an interrupted upgrade resumed
    as the store's recovery by a handle speaking the target schemas;
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
  upgrade proceeds, an upgrade started from inside a callback refused, readers of both versions
  attaching during a reader-safe upgrade, two reader-safe upgrades in a row with a reader of the
  first source still attached, writers of versions A, B and A again each leaving their version
  recorded, a failed write leaving it unchanged, and a stamp whose sync fails after its rename.
  Upgrades contended between processes are tested in M6, once independent writers can coexist.
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/acadia` -> M2 -> `AC-2` (a
  > blocking exclusive lock). See [CHECKLIST.md](../acadia/CHECKLIST.md#ac-2).

## M6 -- Writers in several processes take turns

- [ ] **MP-8** -- Implement the contract's writer exclusion in every implementation: across
  every thread of every process, waiting, with no order among waiters promised, released when the
  holder dies, with the ownership record telling the next acquirer so; extend the conformance
  suite to cover it, on Windows and Linux.
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
  recovering process killed between steps and another process resuming its recovery; a schema
  upgrade waiting for attached writers to leave, with the registry naming them and never listing
  a killed process as attached, and new handles waiting while it holds the gate; an upgrader killed
  mid-migration and its upgrade resumed; two upgraders in different processes at once,
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
  error trace for counterexamples) and check a corpus into the crate's test data.

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

- [ ] **FM-4.1** -- The rest of transactions beyond what M3 models: rollback of an unprepared
  transaction, collection of what it ingested, and the sweep of its staging.

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
