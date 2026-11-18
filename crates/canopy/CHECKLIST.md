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

- [ ] **G-2** -- Settle what an identical creator that arrives while another creation is still in
  progress reports, and make the code and
  `tests::lock::identical_concurrent_creators_never_replace_a_completed_store` agree on it. The
  test requires `AlreadyInitialized`; [DESIGN-NOTES.md](DESIGN-NOTES.md) "Writer exclusion
  belongs to an operation" promises it only to a creator that finds the completed marker, and `init_through`'s
  first occupancy check, taken before exclusion, refuses a partly built root as `NotEmpty`. See
  [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md), 2026-11-17 update. Either waiting for
  exclusion and rechecking, or narrowing the test to the documented contract, settles it. Waits on
  the engineer choosing between them.

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
  leases, reads and discovery, and the escapes -- a waiting writer withdrawing, a handle giving up
  attaching -- bound as M11-5 binds the writer actions, so that every `Coordination` trace replays.
  Before binding a writer's withdrawal, agree with the engineer how a pending request is cancelled
  in production: a cancellable acquisition contract, or a client request loop over nonblocking
  acquisition. The model's `Withdraw` must exercise that production path, not a test-only
  simulation. The acquisition contract is in acadia's
  [exclusion.rs](../acadia/src/coord/exclusion.rs); Store's acquisition is in
  [coordinated.rs](src/store/coordinated.rs). Also classify the negative-control timelines'
  expected departures explicitly in their scenario tables before requiring the full corpus to
  pass; T14's early count publication is not a behaviour of correct recovery.

- [ ] **M11-8** (was `FM-3.3`) -- Prove the harness bites: remove the namespace sync that
  collection performs before deleting, and confirm that the replay of a trace of the protocol as
  built in which collection syncs an entry whose live pointer is ahead of its durable one fails
  at that `CollSync` step. The `DANGLING-DURABLE` counterexample is not that trace: it is read
  from a model without the record guard, and with the record guard the model reaches no
  `DANGLING-DURABLE` state even without the sync (decided with the engineer, 2026-11-15).

- [ ] **M11-9** -- Establish the recovery quotient, so that the publication composition is checked
  through a reduction shown to match its reference, not one resting on its argument alone
  ([DESIGN-NOTES.md](DESIGN-NOTES.md) "State-space reduction for recovery-unit verification" and
  "A reduction keeps the original it answers to"; the argument is in
  [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#recovery-quotient-argument)). First finish checking
  the removal of writer-request eligibility ([DESIGN-NOTES.md](DESIGN-NOTES.md) "A writer's
  request ends served, withdrawn or dead"), whose `Coordination` item tier and replay bridges were
  checked before it was committed: the `SharedPublication` and `SharedPublicationCheck` item
  tiers, `ReplayCoordination`, `ReplaySharedPublication`, and the rewritten probe
  `withdraw-wrong-outcome`. Each `SharedPublicationCheck` configuration and its unreduced
  `SharedPublication` counterpart reach
  the same outcome -- every invariant and property holding, or the same one violated -- and the
  controls and the collection, refusal, completion and stale-progress witnesses keep theirs under
  the view. Then make the view's configurations the ones the sweep checks on each revision of the
  model, and keep each unreduced counterpart runnable as the reference, in a tier chosen from its
  measured cost. Until then the unreduced configurations stay as they are.

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

- [x] **M13-1** (was `FM-5.1`) -- `MemFs` has an independent durability reference model and replay bridge in Acadia. -> [completed 2026-11-17](../acadia/COMPLETED-CHECKLIST.md#m13-1)

## M14 -- The model clients compose with

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "The queued client contract is the public model".
Discussion: the opaque contract discussion recorded in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) and
[DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md](design-sessions/DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md).

M14's primary deliverable is an opaque TLA+ contract that a dependent can instantiate without
importing canopy's ring, coordination, publication or filesystem mechanism. The asynchronous API
is one implementation of that contract. The ring and coordination models refine the client
contract; clients refine neither and depend on none of their variables.

These items follow the primary deliverable, the milestones above: the observable store outcomes
rest on ODI-2's decision about coordination fairness and on what MP-4 and CQ-1 find, the refinement
is written against the state MP-3 and MP-4 change, the oracle for tests at the API uses M11's
harness, and the documentation for dependents describes the multi-process behaviour of M7 to M10.
CM-1, which needed none of them and steers MP-3 and MP-4, was done in M3.

**Parallel track (decided with the engineer, 2026-11-16).** The store's hardening, M11 to M13,
stays the first and primary work. M14-0 is complete, so M14-1, M14-2 and M14-3 may proceed in
another session or on another machine without waiting for the hardening, provided they take
nothing from it: no long checks on the machine doing the hardening, no edits to existing specs,
generated configurations or traces the hardening is changing, and where both touch the same code
the hardening's change goes first. M14-1's contract and toy dependent remain standalone until the
hardening has settled the observables they project. M14-7 starts from a model whose writer requests
end served, withdrawn or dead. The rest of M14 follows its dependencies as written.

- [x] **M14-0** (was `ODI-5`) -- The queued client contract, request lifecycle, ambiguity rules and ready-pair scheduling are decided. -> [completed 2026-11-16](COMPLETED-CHECKLIST.md#m14-0)

- [x] **M14-1** (was `CM-2`) -- The standalone client contract, negative controls and mechanism-free toy dependent are implemented. -> [completed 2026-11-16](COMPLETED-CHECKLIST.md#m14-1)

- [x] **M14-2** (was `API-2`) -- Opaque transactions are built outside exclusion and `transform` retries changed reads with a bound. -> [completed 2026-11-16](COMPLETED-CHECKLIST.md#m14-2)

- [x] **M14-3** (was `QS-1`) -- The ring contract, two-doorbell mechanism, round-robin scheduler and controls are modelled. -> [completed 2026-11-16](COMPLETED-CHECKLIST.md#m14-3)

- [x] **M14-4** (was `API-3`) -- Sessions expose asynchronous queue pairs, readiness, bounded waits and parked command execution. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m14-4)

- [x] **M14-5** (was `API-1`) -- The public API no longer exposes `Store::plan_and_apply`. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m14-5)

- [x] **M14-6** (was `ODI-6`) -- Bodies remain opaque and append is the first bounded store-evaluated primitive. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m14-6)

- [x] **M14-7** (was `CM-3`) -- Ring and store outcomes refine one opaque client history. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m14-7)

- [x] **M14-8** (was `FM-3.4`) -- Store sessions are checked against the opaque client projection at the API. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m14-8)

- [x] **M14-9** (was `CM-4`) -- Dependents have a complete outcome and retry guide for the client contract. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m14-9)

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
