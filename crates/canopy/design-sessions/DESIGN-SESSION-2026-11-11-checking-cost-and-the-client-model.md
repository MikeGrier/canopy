# Design session 2026-11-11 -- the cost of checking a model, and the model clients rely on

Resulting decisions: [DESIGN-NOTES.md](../DESIGN-NOTES.md) "The model clients rely on, and the
model the code is held to", which also records the decision on `ODI-3`, and "Probe mutants are
data in the repository, matched so that they survive edits to the spec". Practice recorded in the
repository instructions under "TLA+ / TLC": the tiers, the cadence of a work item's checks, and
probes at the milestone boundary. Work queued in [CHECKLIST.md](../CHECKLIST.md): see the open
questions at the end. The figures are observations of 2026-11-11 on one host and are not expected
to stay true.

## 1. What a work item's checks cost

By the end of MP-2 the full sweep of `Coordination.tla`'s configurations took about 50 minutes,
and each work item had been running it, or something close.

The engineer's remarks, sorted as the engineer asked:

- **Intent.** The per-item cost of a full verification is too high, unless the item changes the
  model itself and the cost is the cost of doing business. At one hour per work item the engineer
  is unhappy and wants something lighter, even at the price of some realignment at the end of a
  milestone. The checks are guardrails to keep the work on the road, not lawyers micromanaging
  every adjustment. A person working in the code would want checks adaptive to what changed and
  proportionate to it: a small change's tests in seconds, under 30. A full sweep of an hour, after
  the PR has merged, is welcome. While the rules of the model are being authored a higher cost is
  accepted.
- **Edict.** Adopt, as practice, that each work item names its own check, and build the tier tag.
  The engineer also wants the adoption of the formal-model discipline to go faster.

Measured for the levers: the 14 heaviest configurations cost about 33 of the 50 minutes, and the
rest -- controls, witnesses, no-deadlock runs and light mains -- about 17, at 2 to 4 seconds each,
mostly JVM start-up. Running 24 light configurations eight at a time took 29 seconds against 62.

The levers offered, cheapest first:

1. Each work item names its own check: a `Verify:` line giving the `-Filter` patterns of its
   configurations, run with `-FailFast`.
2. A tier tag on the heavy mains, so that mid-milestone runs skip them.
3. Change-driven selection from the diff: only witnesses changed, run the witnesses; only property
   definitions changed, run the configurations that list them; actions or state changed, run the
   light tier. More tooling, and shared-definition edits still need a boundary run.
4. Trading rigor for speed: capped bounds, or time-boxed random simulation. This needs the
   engineer's approval, and the repository's rule on reproducibility makes randomness a decision.

Chosen: 1 and 2. Built: a `Tier: milestone` line in a configuration's header, marking the heavy
mains; `-Tier item|milestone|all`, `-List`, `-FailFast` and several `-Filter` patterns in
`tools/run-tlc-all.ps1`; and a report of item-tier configurations that ran slowly, so the tags
stay honest. Recorded in the instructions as "Tiers" and "Cadence".

One lesson of the milestone was about order: sweeps were killed and restarted when the spec was
edited under them. Targeted runs while editing, one sweep when the edits stop.

## 2. Probes and timelines

The engineer asked what they are, what they cost and what moving them to the milestone boundary
would cost.

**A probe** is a mutation test of the model. A scratch copy of the spec has one rule broken by
editing its text, the configuration that should expose the break is run, and the property written
for that rule must fail. A mutant that survives shows a check that cannot fail. Controls do not
replace probes, since controls and properties are written by one hand and can share a blind spot.
**A timeline** is documentation: a table of one scenario, a row per event, written from a TLC
trace for a human reader. It is not a check, and it is confirmed against the model once, when it
is written.

Costs, measured from the timestamps of this session over MP-2.1 to MP-2.4 (about five hours;
approximate, since time is attributed by kind of call):

- probes, about 5 percent: about 10 minutes of TLC and 6 of writing and reading;
- timelines, about 5 percent, for 10 timelines;
- the staged runs of an item's new configurations plus the state-count regression of the old
  ones, about 25 percent;
- full sweeps with their waiting, about 30 percent.

Probes ran on idle cores during the sweeps, so most of their cost was hidden. With sweeps out of
the per-item loop each of the two becomes about 7 to 8 percent of what remains. An earlier remark
that they cost the most after the sweeps was wrong.

What they found, from the records of the first ten items of M3 (about 150 probes): ten misses, in
seven items. Three items needed a check fixed because it could not fail: no property caught the
serving of an ineligible request; a lease property was built on the model's own lease count; and
scenarios in which both layers' upgrades had one target hid a fault that moved the wrong layer.
Two items recorded blind spots, and two had wrong expectations. None on record was a bug in the
model. In MP-2.4 a survivor arrived after the sweep had begun, which killed and restarted it, about
13 minutes. The 23 timelines found no defect in the model; they explained findings that the
controls had produced.

The engineer's reply: "Probes: aha mutation testing. Great technique. Not to be used on every
build and work item. Best applied based on diffs and dependency graphs on somewhat meaningful
boundaries. Definitely move this to milestone boundary." **Edict:** probes move to the milestone
boundary. Recorded as "Probes" in the instructions: written and run at the boundary, chosen from
the milestone's diff, before the boundary's sweep, with a full run alongside the full sweep.

On timelines the engineer asked whether a run scoped by the delta and a dependency graph exists,
with a full run much less often -- weekly, and only if something changed. None exists, because a
timeline has no executable form. A replay would take seconds, so scoping it by dependency would
attribute a stale timeline to a change rather than save time. The trace corpus of M7 is the same
artifact: one scenario could be the document's evidence, the model's replay and the code's replay.
Queued as a decision with those options.

## 3. What a dependency graph can scope

A throwaway prototype built the graph of which definitions refer to which, from the names in the
modules, and the definitions each configuration reaches from the names its `.cfg` lists. The
MP-2.3 and MP-2.4 edits both changed `Init` and `vars` (MP-2.3 also `Next`), so every one of the
225 configurations that existed before depended on a changed definition. A graph over names cannot
see that a new disjunct is dead behind a switch such as `Txn`, so an edit to a definition every
configuration reaches selects every configuration.

In the current spec 171 of 451 definitions are reached by every configuration, 123 by between a
tenth and a half, and 146 by a tenth or fewer, 135 of those by exactly one: edits local to a
property, a scenario or a witness would select little. The chain uses `EXTENDS` only, and the
pinned `tla2tools.jar` carries SANY's XML exporter, which would give a resolved graph where
`INSTANCE` is used. Continuous integration runs on hosted runners, and a weekly schedule already
exists for code scanning; whether a hosted runner finishes the sweep within its limit is not
known.

The engineer: "right now we're authoring the rules and sometimes these edits will have high
impact and that's the cost of this phase of the project and that's OK. It just can't be the
standard." Recorded in the instructions' cadence paragraph, and the machinery queued as
impact-scoped runs, with the weekly gated full run as the proposal it is.

## 4. Two tracks, and the model clients rely on

`ODI-3` asked whether the model keeps its abstract write and one-step upgrade beside the detailed
forms. Observed that day by flipping the switches in scratch runs: two writers, two requests,
nobody dies, about 2,600 states abstract against 13,600 with transactions; a safe upgrader with a
writer and a reader, about 40,000 against 690,000, 4 seconds against 31; a registry configuration
with an upgrade body at the usual scale passed 35 million states and was shrunk. Of 264
configurations 138 used the abstract write, 126 transactions and 60 an upgrade body, and the spec
had about 60 mentions of the switches. The options were to keep both, to retire the abstract
forms, or to merge them by defining the abstract form as a coarsening of the detailed one and
having TLC check it.

The engineer asked whether maintaining both tracks, one tracking the code and one strictly
abstract, is an undue burden. Before that was analysed the engineer asked whether a model that is
what is advertised to clients -- their model of the operation, not exposed to every implementation
detail -- would be the same thing as the higher-level abstract model.

**No.** The two differ on a different axis. The abstract forms are coarser steps over the same
internal state: the abstract write is two cells changed in turn inside the count's bracket, and it
still has the count, fence, ownership record, tiers and generation. A client model has fewer
variables, not coarser steps: what a `Store` user can observe -- the logical contents, which
handles are attached and in what role, each request's outcome, whether the store needs recovery.
It hides mechanism and not outcomes: a write whose holder was told recovery is required may or may
not have taken effect, a read may be rejected as disturbed, an upgrade may drain or refuse readers.

| Model | Audience | Contains | Relation |
|---|---|---|---|
| Client (advertised) | `Store` users | logical contents, outcomes, guarantees | the implementation must refine it |
| Coordination, detailed | the code, the replay of M7 | count, fence, commit log, tiers, registry | tracks the code |
| Coordination, abstract | the checker | the same variables, coarser steps | cheaper variant of the one above |

Some existing properties are client guarantees (`SettledReadSawNoPartOfAnOperation`,
`EveryRequestEndsInExactlyOneOutcome`, `AnAttachedHandleCanReadTheData`, `AtMostOneHolder`);
others are internal (`CountIsOddWhileAnOperationIsUnresolved`, `StagingIsAlwaysAnnouncedByTheLog`,
`GenerationAdvancesOnEveryAcquisitionAndOnlyThen`). TLC can check that every behaviour of the
detailed model, projected onto what clients see, is a behaviour of the client model, and the
projection is where the commit point is written down. The check adds a small cost and leaves the
code's model free to change while the refinement holds. It is independent of `ODI-3`: a client
model is not a second track of the same kind, and its refinement machinery is what the merge
option needs.

The engineer's decisions:

- **Queue working it out**, with both angles: downward, into the reliability of the code; upward,
  making it easier for code that depends on canopy to use it reliably.
- **`ODI-3`: keep both forms going for now**, as long as the other mechanisms discussed -- the
  tiers, the dependency-graph-based runs -- are acted on. Merging or retiring stays possible and is
  not scheduled. Asked whether it needed a trigger for revisiting, the engineer said it does not:
  poor work-item throughput is what will drive inspecting the impediments again.

Sequencing proposed with the items: classify the properties by audience first, since it costs
little and tells the work on the durable publication record and on composition which guarantees to
keep; decide what is promised after the decision on fairness, since the promise about waiting
rests on it; write the refinement after the shared state settles, since that work still changes
what the projection is written against; the oracle for tests at the API with the conformance
harness; the documentation with the behaviour it describes.

The engineer then placed the client model's work after the primary deliverable: deciding the
promise, the client model, the refinement, the oracle for tests at the API and the documentation for
dependents form M10, after the milestones that deliver the store's behaviour, and the classification
by audience stays in M3.

## 5. Moving the tools into the repository, and where the mutants live

`SPEC-1` moved the tools that keep the configurations honest out of the git-ignored scratch
directory into `tools/`: the generator of the `Coordination*.cfg` files, divided along its tables
(constants, main configurations, controls and witnesses), the generator of the `Creation*.cfg`
files, the sweep's self-test, the counterexample summary and the header formatter. A library holds
what the sweep and the probe runner share. Regenerating every configuration changes no file. The
state counts that a scratch script compared by hand are not moved: they are numbers that were
true on one day, and the record of the last green run in `SPEC-2` is where a count belongs.

The probes were five scripts, each carrying its own copy of the edit machinery and the mutants as
quoted spec text. Observed on 2026-11-11: of the 132 mutants with a text edit, 23 did not match
the current text. Most of the staleness a first measurement showed -- about one mutant in three --
came from two causes that the matching now absorbs: a frame (the `UNCHANGED` tuple) that had gained
a variable since the mutant was written, and a function update written `[f EXCEPT ![k] = v]` that
the model now writes with the `Set` operator. The rest were rules whose variables or text had been
rewritten. Nine mutants named a staged configuration of an item, not a configuration of the
repository; each is mapped to the repository configuration with the same constants.

Moving the corpus found three more ways a mutant goes wrong that the text matching alone does not
show, which the first full run of the migrated corpus, 149 runs eight at a time in 13 minutes,
reported:

- **A match on a prefix of a construct that has since grown.** A mutant that replaced the two
  disjuncts of an admission rule still matched after a third was added, and left the third dangling:
  TLC could not parse the module and the run said ERROR, not caught.
- **An edit that spells out a derived tuple.** A mutant that changes `count` while `coord` also
  holds the owner and the fence names the other components in its frame; when a derived tuple gained
  a component the mutated action left a variable undefined, and the run said ERROR.
- **A match at the wrong place.** A bare frame edit matched the one action that now has the shape,
  which is reachable only under a control, instead of the attach step it was written for; it too
  came out as ERROR here, but a mutant like it could as easily have survived. That every mutant of
  a corpus has come out as recorded, by the property written for its rule, after it is moved or
  repaired is what shows its sites are the intended ones.

The 23 that did not match and the 4 that came out as ERROR were re-expressed against the current
text while the corpus moved. One rule that is broken in two places in a definition needed a list of
edits, which the runner now takes. After that every mutant came out as recorded: caught by the
property written for its rule, or equivalent where the mutant says nothing checks the rule.

The options for the mutants' home, with what each costs:

- **Scripts, as they were.** Each script is its own tool, the mutants are text inside code, and a
  script discarded with its scratch directory discards the evidence that a property can fail.
  Rejected.
- **A `Control` constant in the spec for each mutant,** as the negative controls are. A control is
  a rule the sweep checks every time; about 150 probes would grow the spec by a switch each, and
  the sweep by a configuration each, to check what is wanted once a milestone. Whether a rule is
  worth a control is decided about the rule when it is authored, and does not bear on where the
  mutants live. Rejected as the home of the corpus.
- **Data in the repository beside the spec, matched modulo the frame.** Chosen. The corpus is
  found in the checkout, runs with one command, and goes stale only where the text it quotes was
  rewritten.

The question of whether a one-line list of mutants is kept with each item is answered by the
corpus: the mutants are chosen at the boundary from the milestone's diff, and each is a file entry,
not a line in a checklist item. The decision is in [DESIGN-NOTES.md](../DESIGN-NOTES.md) "Probe
mutants are data in the repository, matched so that they survive edits to the spec".

The engineer asked how this mutation testing relates to `cargo mutants`, which is applied to refine
unit tests, run in CI on what changed, and run in full now and then to find what the tests missed.
Here the primary testing is TLC checking the model's properties; witnesses show a state is
reached, the way coverage does; a negative control is a mutant kept in the spec behind a switch and
checked at every sweep; and the probes test the properties, as `cargo mutants` tests the tests --
which is why they found weak checks and no defect in the model. The difference is that the probes
are written by hand, one per rule someone named, so nothing finds a rule nobody named. A generated
pass would, and it was queued as `SPEC-3` in [CHECKLIST.md](../CHECKLIST.md) under "TLA futures".
The engineer deferred it until the milestones of the checklist are done: the generator would take a
while to build and a full pass would take days of TLC time.

## Open questions

- **The form of a timeline:** prose, a replay on the model, or a member of the trace corpus of M7.
  `ODI-4` in [CHECKLIST.md](../CHECKLIST.md).
- **The weekly full run:** the cadence is a proposal, and whether a hosted runner finishes the
  sweep decides whether the runner is hosted or self-hosted. `SPEC-2` in
  [CHECKLIST.md](../CHECKLIST.md).
- **Where the mutants live and when they are written:** answered by `SPEC-1` -- section 5 and
  [DESIGN-NOTES.md](../DESIGN-NOTES.md) "Probe mutants are data in the repository, matched so that
  they survive edits to the spec". `SPEC-1` in
  [COMPLETED-CHECKLIST.md](../COMPLETED-CHECKLIST.md#spec-1).
- **What canopy promises, and how the promise is delivered and versioned.** `ODI-5` in
  [CHECKLIST.md](../CHECKLIST.md).
