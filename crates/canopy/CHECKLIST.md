# CHECKLIST -- formal state model (TLA+) and code conformance

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Formal state model in TLA+". Discussion:
[DESIGN-SESSION-2026-11-10-formal-model.md](design-sessions/DESIGN-SESSION-2026-11-10-formal-model.md).
Completed items: [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

An item's ID is its milestone and its place there (`M6-2`, or `G-1` for a gated item); `(was ...)`
names the ID that design notes and the archive cite it by.

## Gated -- waiting on an event this checklist does not schedule

Each item here names the event it waits on; it joins a milestone once that event has happened.

- [ ] **G-1** (was `SPEC-2.4`) -- Verify the TLA+ sweep workflow on `main`, as
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "The full sweep runs when main changes what it covers"
  specifies: the push that brings it to `main` starts a run of the probes and the sweep, and a push
  that changes nothing it covers starts none. Time that run on the hosted runner, and decide with
  the engineer whether the runner stays hosted or becomes self-hosted, recorded in the same
  decision. Waits on the workflow reaching `main`.

## M9+ -- Concrete Canopy schema migrations

- [ ] **CANOPY-MIGRATION** -- Specify a concrete Canopy source/target schema change and supply its
  store-owned idempotent units to the migration engine. **BLOCKER:** the repository defines Canopy
  schema 1 but no newer schema's data or layout changes. The engineer agreed on 2026-11-14 that the
  engine must reject undefined transitions rather than invent a format change. Graduate this item
  when a concrete schema change is specified; the generic engine is the current milestone's work.

## M11 -- Conformance harness

- [x] **M11-1** (was `FM-3.1`) -- Export TLC behaviours as a JSON trace corpus: every timeline's replay, random walks of the protocol as built, and the named defects' counterexamples. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-1)

- [x] **M11-2** (was `FM-3.2`) -- The replay harness and `PointerPublication`'s bridge: every `PointerPublication` trace replays against `Store` on `MemFs` + `FaultyFs`, with the model refined to the store's commit-log handling. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-2)

- [x] **M11-3** -- `Creation`'s bridge and the step scheduler: every `Creation` trace replays against acadia's creation contract through both implementations, with the model refined to the code's decisions. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-3)

- [x] **M11-4** -- Map `Coordination`'s writer actions to the code, and agree the step hook with the engineer: the inventory, the hook's interface, the abstract write's binding, and the writer-only corpus. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-4)
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/acadia` -> M4 -> `AC-5` (the
  > shared implementation's step hook). See [CHECKLIST.md](../acadia/CHECKLIST.md#ac-5).

- [x] **M11-5** -- `Coordination`'s writer actions bound to the code: every writer trace replays against acadia's contract through each implementation whose behaviour it is, with the model refined to the code's recovery and crash. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-5)

- [x] **M11-6** -- The SharedPublication Store replay binds document and recovery steps to both coordination implementations. -> [completed 2026-11-16](COMPLETED-CHECKLIST.md#m11-6)

- [ ] **M11-7** -- The rest of `Coordination`'s bridge: the registry, the upgrade, clones and
  leases, reads and discovery, and the escapes -- a waiting writer withdrawing or refused, a handle
  giving up attaching -- with the eligibility that refusal follows, bound as M11-5 binds the writer
  actions, so that every `Coordination` trace replays.

- [ ] **M11-8** (was `FM-3.3`) -- Prove the harness bites: remove the namespace sync that
  collection performs before deleting, and confirm that the replay of a trace of the protocol as
  built in which collection syncs an entry whose live pointer is ahead of its durable one fails
  at that `CollSync` step. The `DANGLING-DURABLE` counterexample is not that trace: it is read
  from a model without the record guard, and with the record guard the model reaches no
  `DANGLING-DURABLE` state even without the sync (decided with the engineer, 2026-11-15).

## M12 -- Extend the protocol model

- [ ] **M12-1** (was `FM-4.1`) -- The rest of transactions beyond what M3 models. `Coordination.tla` models a
  transaction's records in the commit log, its staging as one unit that is swept, its swaps of
  the namespace, the rollback of one that fails before its prepare record, and recovery's
  roll-forward and roll-back ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A transaction and the commit
  log, modelled in `Coordination.tla`" and "Recovery and its discovery, modelled in
  `Coordination.tla`"). What is left is the content beneath those: the collection of what a
  transaction ingested, and how rollback and the sweep of its staging act on it.

- [ ] **M12-2** (was `FM-4.2`) -- Node paths and node removal: documents and nodes as one namespace tree,
  the collision rules (a write against a node removal in the same set, a node removal at or
  beneath an existing document), and the directory-intent record (`.dir-sync-debt`) with
  torn appends.

- [ ] **M12-3** (was `FM-4.3`) -- Repair and scrub, including the modification count a repair holds odd
  while it runs.

- [ ] **M12-4** -- Decide with the engineer how `Store::init` relates to the creation contract, and
  carry it out. `Store::init` reserves the root with `Creation::reserve`, then works under writer
  exclusion rather than the creation exclusion, and does not take over an interrupted creation, so
  the `Creation` replay holds acadia's `create` and `confirm_created` to
  [Creation.tla](spec/Creation.tla) and not `Store::init` ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The
  Creation replay"). Either `Store::init` creates through the contract, its partitions and marker
  being `create`'s artifacts, or the model gains canopy's creation as a client of the contract and
  a bridge drives `Store::init` through it.

## M13 -- Filesystem reference model

- [ ] **M13-1** (was `FM-5.1`) -- A TLA+ reference model of the durability semantics `MemFs` implements
  (directory entries and their sync state, file data and its sync state, rename, crash),
  checked against `MemFs` with the M11 harness. `MemFs`'s recent defects are the first cases it
  must reproduce: one inode left under two names by a crash, a directory replacing a durable
  file, a moved or nested directory inside a carried snapshot, and an unnamed directory beneath
  one.
  > **CROSS-COMPONENT HANDOFF:** this work lives in component `crates/acadia`; its
  > checklist is created when this item starts and the item moves there.

## M14 -- The model clients rely on

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "The model clients rely on, and the model the code is
held to". Discussion:
[DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md](design-sessions/DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md).
These items follow the primary deliverable, the milestones above: what is promised rests on ODI-2's
decision about fairness and on what MP-4 and CQ-1 find, the refinement is written against the state
MP-3 and MP-4 change, the oracle for tests at the API uses M11's harness, and the documentation for
dependents describes the multi-process behaviour of M7 to M10. CM-1, which needed none of them and steers
MP-3 and MP-4, was done in M3.

- [ ] **M14-1** (was `API-2`) -- Transactions built without exclusion and submitted once
  ([DESIGN-NOTES.md](DESIGN-NOTES.md) "No caller code runs under exclusion but the commit
  observer"): an operation that only checks a document's digest or absence, beside writes and
  removals; a builder the store hands out that records each document it reads as such a check and
  holds no lock; `Store::transact`, taking the operation list and an options value that is empty
  today; and `Store::transform` as the helper that runs a closure against a fresh builder without
  the lock and retries a commit rejected by a changed read, up to a bounded number of attempts.
  The closure form of `transform` is retired with it.

- [ ] **M14-2** (was `QS-1`) -- The ring's spec barrier ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The ring: a
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

- [ ] **M14-3** (was `API-3`) -- Sessions and queue pairs as M14-2 specifies them, through which all work is
  submitted, with M14-1's transactions, reads, snapshots and enumerations as entries: a session
  acquired on a store, pairs acquired from it, ends `Send` and not `Sync` with `&mut` push and pop,
  completions reserved at submission, a submission doorbell rung by the client for a batch, a
  completion doorbell that is a waitable object on each platform, and the engine setting aside a
  waiting entry with a registration on the coordination notification, which rings the engine's
  doorbell, so that no waiting holds a thread ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A client
  reaches the store through a session and its queue pairs"). The synchronous methods become a
  convenience that submits one entry and waits on the completion doorbell. The queues are
  `canopy::ring`'s, generic over the signal as RG-2's doorbell is, following
  [durable-ioring](https://github.com/MikeGrier/windows-threadpool-sys/tree/main/crates/durable-ioring)'s
  ring, with the step of `Ring.tla` each line realizes named beside it, and checked by the
  memory-model checker RG-2.1 chooses. The completion doorbell is a `Doorbell` over the
  platform's signal, so it is sealed: a client waits on it through `Doorbell::watch` (RG-3) or the
  synchronous wait, and reaches its platform object only through the `unsafe` accessors
  ([DESIGN-NOTES.md](DESIGN-NOTES.md) "The doorbell's signal is sealed"). Tests: exactly one
  completion per entry under many pairs, per-pair order, a full completion queue refusing a
  submission, a pair dropped with entries unstarted and started, a client waiting on the
  completion doorbell from a thread pool's wait, many waiting entries holding no engine thread,
  and sabotages that reverse the arming protocol's clear and check, and the doorbell's reset and
  flag, each of which must hang.

- [ ] **M14-4** (was `API-1`) -- Withdraw `Store::plan_and_apply` from the public API, moving its callers to
  M14-1's transactions, whose checks stand for what a plan read, so that M14-6 decides what canopy
  advertises from a surface without it ([DESIGN-NOTES.md](DESIGN-NOTES.md) "No caller code runs
  under exclusion but the commit observer").

- [ ] **M14-5** (was `ODI-6`) -- Decide which primitives the store evaluates under exclusion -- operations
  computed from a document's current value, such as add, maximum and append, which never
  conflict -- from where CQ-1 shows read-modify-write retries failing too often, with the bound on
  each one's cost, and whether canopy owns a body format so that a primitive can work on more than
  bytes ([DESIGN-NOTES.md](DESIGN-NOTES.md) "No caller code runs under exclusion but the commit
  observer"). Record the decision; each primitive adopted becomes a work item here.

- [ ] **M14-6** (was `ODI-5`) -- Decide what canopy advertises to the code that depends on it. From CM-1's
  client-visible guarantees and the results of MP-4, CQ-1 and ODI-2, decide which guarantees are
  promised and which are explicitly not -- an order or a bound on waiting, the outcome of a write
  whose holder was told recovery is required until recovery has run, a networked filesystem --
  and how the promise is versioned, since a change to it is a change a dependent can feel.
  Decide how it is delivered: prose in the crate documentation, a TLA+ module a dependent can
  compose with, or both. A promise is a commitment, so this is the engineer's decision; record it
  in [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] **M14-7** (was `CM-2`) -- The client model: a small TLA+ module over what a `Store` user can observe --
  the logical contents, which handles are attached and in what role, each request's outcome,
  whether the store needs recovery -- and none of the mechanism: no count, fence, generation,
  commit log or lock order. Its invariants are M14-6's promised guarantees in a user's terms, and
  it promises no more, so the implementation keeps the freedom the promise leaves. It follows
  the conventions of the other models: controls that reverse a promise and must fail, witnesses
  that show each outcome a caller can meet is reachable, and configurations whose headers state
  the outcome [run-tlc-all.ps1](../../tools/run-tlc-all.ps1) must find.

- [ ] **M14-8** (was `CM-3`) -- Check that the code's model refines the client model. For each form of the
  coordination model that is kept -- the abstract and the detailed write and upgrade -- write in
  the spec the mapping from its state to the client model's, and have TLC check in its main
  configurations that every behaviour projects onto a behaviour of the client model. The mapping
  is where the commit point is written down: when a write counts as having happened, including
  when its holder was told it failed. The probes at the milestone boundary include mutants that
  break a promise, and each must fail the refinement.

  Verify: the refinement holds in each main configuration, and a control that breaks a promise
  fails it.

- [ ] **M14-9** (was `FM-3.4`) -- Check `Store` against the client model at the API: sequences of calls on
  `MemFs` + `FaultyFs`, with injected faults, crashes and several `Store`s over one root, and the
  client model as the oracle -- every outcome the code returns must be one the model allows from
  the state its earlier calls put it in. M11-2 compares internal state step by step; this
  compares only what a user can see, so it keeps passing through any change of mechanism that
  M14-8's refinement still admits.

- [ ] **M14-10** (was `CM-4`) -- Tell dependents how to use canopy reliably. From M14-6's decision and the
  client model, write in the crate documentation the contract as a user relies on it and a guide
  to acting on each outcome: what a caller does when told recovery is required or a read is
  disturbed, how to learn whether a write whose outcome is unknown took effect and how to retry
  it safely, what waiting to expect under contention, and what is not promised. Each statement
  names the property of the client model it rests on, so a reader can follow a promise to its
  check.

## M14+ -- TLA futures

Work on how the models are checked that waits until the milestones above are done.

- [ ] **M14+-1** (was `SPEC-3`) -- Generate mutants of a spec mechanically, to find the rules no property checks.
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
