# CHECKLIST -- formal state model (TLA+) and code conformance

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Formal state model in TLA+ (`FM-1`)". Discussion:
[DESIGN-SESSION-2026-11-10-formal-model.md](design-sessions/DESIGN-SESSION-2026-11-10-formal-model.md).

The model is the reference for crash, partial-failure and replica-disagreement interleavings.
Code is held to it by replaying model traces against the real `Store` on `MemFs` + `FaultyFs`
and comparing a projection of the on-disk state with the model state after every step.

## M0 -- Tooling

- [x] **FM-0.1** -- JDK and pinned tla2tools.jar installed; versions and commands recorded in the repository instructions. -> [completed 2026-11-10](COMPLETED-CHECKLIST.md#fm-01)
- [x] **FM-0.2** -- `tools/run-tlc.ps1` model-checks a spec and exits non-zero on any violation. -> [completed 2026-11-10](COMPLETED-CHECKLIST.md#fm-02)

## M1 -- First slice: pointer publication, collection, crash

Scope: one document, N pointer replicas (live and durable entry each), blob replicas (live and
durable), the non-transactional write's per-replica steps (stage, rename, parent fsync, each
able to fail), an ended transaction left in the log, collection, crash (live := durable),
reopen with recovery, and the read classification (`Found` / `Absent` / `Contended` / error).

- [ ] **FM-1.1** -- Write `spec/PointerPublication.tla` and its `.cfg` modelling the scope
  above at the granularity of the code's individual `Vfs` calls, with each modelled action
  citing the function it abstracts.

- [ ] **FM-1.2** -- Safety invariants: after any crash no durable pointer majority names a
  blob missing from every durable replica (`D1`); a read reports `Contended` only while a
  modification is in progress. Check that removing the collection-time namespace sync (`RV7-1`)
  produces the `D1` counterexample, so the invariant is shown to bite.

- [ ] **FM-1.3** -- Liveness property: with no writer running, every document reaches a
  majority value or a majority of absence. Confirm TLC reports the `E1` violation (two
  consecutive failed non-transactional writes leaving `[C, B, A]`) against the current
  protocol.

## M2 -- Decide and implement the E1 fix from the model

- [ ] **FM-2.1** -- Model the three candidate fixes for `E1` (resolve the prior partial
  publication before admitting a new version; a durable per-write publication record; fence on
  a failure after the first rename) as alternative specs or a constant-selected action set.
  Record which satisfy FM-1.2 and FM-1.3, and at what cost in fsyncs, as a decision in
  [DESIGN-NOTES.md](DESIGN-NOTES.md).

- [ ] **FM-2.2** -- Implement the chosen fix in `canopy`, with regression tests for the `E1`
  sequence followed by warm reopen and by power loss.

- [ ] **FM-2.3** -- Distinguish a stable no-majority state from contention in
  `read_document`: sample the modification count before and after the vote, and report a
  distinct outcome when `Store::undisturbed` holds, matching the model's read classification.

## M3 -- Conformance harness

- [ ] **FM-3.1** -- Export TLC behaviours as JSON traces (simulation mode for breadth, the
  error trace for counterexamples) and check a corpus into the crate's test data.

- [ ] **FM-3.2** -- A Rust test that replays each trace step against `Store` on
  `MemFs` + `FaultyFs` (failures as injected faults, crash as `simulate_crash`, reopen as
  `Store::open`) and after every step compares a projection of the store's live and durable
  state with the trace's model state.

- [ ] **FM-3.3** -- Prove the harness bites: revert-probe `RV7-1` and confirm the replay of
  the `D1` trace fails at the step the model predicts.

## M4 -- Extend the protocol model

- [ ] **FM-4.1** -- Transactions: `begin` / ingest / `prepare` / namespace apply / `end` /
  collection, the fence across handles, and recovery's roll-forward and rollback.

- [ ] **FM-4.2** -- Node paths and node removal: documents and nodes as one namespace tree,
  the collision rules (`RV6-1`, `RV7-2`), and the directory-intent record (`.dir-sync-debt`)
  with torn appends (`RV6-3`).

- [ ] **FM-4.3** -- Repair and scrub, including the modification-count protocol (`RV6-8`).

## M5 -- Filesystem reference model

- [ ] **FM-5.1** -- A TLA+ reference model of the durability semantics `MemFs` implements
  (directory entries and their sync state, file data and its sync state, rename, crash),
  checked against `MemFs` with the M3 harness. `MemFs`'s recent defects (`RV7-3`, `RV7-4`,
  and the open `E2`-`E4`) are the first cases it must reproduce.
  > **CROSS-COMPONENT HANDOFF:** this work lives in component `crates/acadia`; its
  > checklist is created when this item starts and the item moves there.
