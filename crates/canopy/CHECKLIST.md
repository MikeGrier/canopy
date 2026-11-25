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

- [ ] **G-3** -- Close the deferred unreduced `SharedPublication` pairs: run
  `tools/check-shared-publication-pairs.ps1 -Tier all -Closure`, so that every positive
  configuration in the generator's `$boundaryClosure` table is closed in the unreduced model as
  well as through the view ([DESIGN-NOTES.md](DESIGN-NOTES.md) "Unreduced closures wait for a frozen
  boundary"). Before starting, estimate the cost from the latest measurements and agree it with the
  engineer. Waits on a frozen milestone or release candidate of the publication composition.

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
  > shared implementation's step hook). See [COMPLETED-CHECKLIST.md](../acadia/COMPLETED-CHECKLIST.md#ac-5).

- [x] **M11-5** -- `Coordination`'s writer actions bound to the code: every writer trace replays against acadia's contract through each implementation whose behaviour it is, with the model refined to the code's recovery and crash. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#m11-5)

- [x] **M11-6** -- The SharedPublication Store replay binds document and recovery steps to both coordination implementations. -> [completed 2026-11-16](COMPLETED-CHECKLIST.md#m11-6)

- [x] **M11-7** -- Bound the remaining Coordination bridge actions and classified every control timeline. -> [completed 2026-11-20](COMPLETED-CHECKLIST.md#m11-7)

- [x] **M11-8** -- The conformance replay detects removal of collection's namespace sync at the durable-changing `CollSync` action. -> [completed 2026-11-20](COMPLETED-CHECKLIST.md#m11-8)

- [x] **M11-9** -- Establish the recovery quotient through an executable reduced/unreduced pair gate. -> [completed 2026-11-22](COMPLETED-CHECKLIST.md#m11-9)

- [x] **M11-10** -- Recovery completion rechecks claims after an intervening holder. -> [completed 2026-11-20](COMPLETED-CHECKLIST.md#m11-10)

- [x] **M11-11** -- Check the soundness of `SharedPublicationCheck`'s `QuotientView` one step at a time. -> [completed 2026-11-22](COMPLETED-CHECKLIST.md#m11-11)

- [x] **M11-12** -- Collapse unnamed blob versions in `QuotientView`. -> [completed 2026-11-22](COMPLETED-CHECKLIST.md#m11-12)

## M12 -- Extend the publication model

- [x] **M12-1** -- Extend the publication model to multi-document transaction swap sets. -> [completed 2026-11-23](COMPLETED-CHECKLIST.md#m12-1)

- [x] **M12-2** -- Extended publication swap sets with namespace removal, directory creation and torn directory intent recovery. -> [completed 2026-11-25](COMPLETED-CHECKLIST.md#m12-2)

- [ ] **M12-3** (was `FM-4.3`) -- Add repair and scrub to the publication composition. Model the
  writer exclusion and modification count a repair holds odd, the live and durable replica checks,
  repair of a version another pointer or record still names, report-driven repair, and a scrub's
  fault and retry behavior. State how each action interacts with an active holder and recovery, and
  prove that repair never makes an unnamed blob version nameable. Extend the quotient view and
  one-step perturbations, add discriminating controls and witnesses, and bind the new actions to the
  Store conformance bridge and trace corpus. **Verify:** the repair, scrub, publication and soundness
  filters, audience tags, replay corpus and the in-scope crate's tests.

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
