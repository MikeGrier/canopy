# canopy design rationale

## Operation-owned exclusion

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Writer exclusion belongs to an
operation". Keeping exclusion on an attached handle prevents independent writers from taking
turns, even though the admission tiers intentionally permit them to stay attached. Retaining the
clone-local reentrant gate preserves nested write verbs without coupling distinct attachments.

Discarding every cache on every acquisition would be correct but would redo durable-name work
on consecutive writes by one handle. Comparing the contract's found and advanced generations
retains that work only when no other holder came between. Comparing software versions without
refreshing on an intervening holder would instead let one agent silently retain another's stamp.

Reading the work list after an unguarded discovery would misclassify a writer that acquired in
between. The lower layer therefore lends a read-only inspection inside its probe, rather than
asking canopy to infer freedom from a count or acquire a bookkeeping-bearing hold for diagnosis.

Recovery phase caches cannot outlive an intervening writer unconditionally. Retaining completed
unit identities allows concurrent recoveries to progress; the modification count invalidates
those identities when a newer operation has begun. This distinguishes an old run from the work
it might otherwise erase at completion. Claims retained across scrub holds have the same lifetime
problem, and belong to the guard that excludes resolving while their operation is live.

## Process-test barriers

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Process tests stop at protocol
boundaries". Explicit acknowledgments tie a kill, recovery step or competing operation to a
particular protocol state. Large interleaved workloads exercise the shared modification count
and transaction journal, while targeted checkpoints discriminate between abandoned ownership
and an operation still in progress. Keeping recovery control in the parent also exposes stale
phase state between units.

A successful completion checkpoint is deliberately different from an interrupted unit: killing
an attached process after recovery has released exclusion must not manufacture another recovery
need. The tests preserve that distinction. Artifact damage uses validated name values because
case-insensitive filesystems can hide the difference between an input spelling and its canonical
on-disk name; execution on Linux makes that boundary observable.

## Attachment lease context

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Attachment operations are leased
and drains are cancellable". A handle-wide outstanding count cannot distinguish an upgrade
called from an operation that would wait for itself from one begun beside another clone's work.
Caller context therefore records synchronous operation ownership separately from the attachment's
drain counter. Making that context span attachments also excludes a callback waiting for another
handle whose writer is queued behind the callback's exclusion hold.

RAII leases give reads and callbacks the same attachment lifetime protection as writes without
making reads take writer exclusion. Nested work remains part of the admitted operation, rather
than failing halfway through completion when a drain begins. Whole-pass leases and per-document
writer holds remain separate, preserving the scrub's turn-taking behavior.

A drain guard keeps cancellation under the caller's control and retains its tiers while waiting.
The registry is a diagnostic record rather than the operation-admission lock: holding that lock
across registration would let a slow filesystem delay refusal. Serializing current-state payloads
without the admission or agent mutex avoids that dependency while preserving the final registry
state when cancellation races a later drain. Fallible status and observer calls make refusal
explicit without redefining draining as recovery or silently discarding an installation.

## Schema conversion and announcements

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Schema-only upgrades convert tiers
and publish admission". Releasing shared tiers before waiting on the gate preserves the lock
order and keeps cancellation from having an unbounded obligation to restore an attachment.
Incremental acquisition lets the caller cancel between lock attempts without a blocked helper
thread. Reader safety depends on the retained reader floor because judging against current would
silently abandon readers admitted by an earlier safe upgrade.

Target readers require an admission descriptor before the reader gate opens. That descriptor
cannot be rolled back after those readers attach, so cancellation of a prepared upgrade preserves
it for recovery rather than undoing the announced target. Making the descriptor part of the marker
keeps admission and schema metadata in one atomic value. Its recovery completion is schema-only:
there are no migration units to replay, and journal settlement precedes metadata advancement.

## Durable migration-unit cursors

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Caller-driven migration units have
durable intent". A marker announcement describes admission but cannot identify which migration
units remain. A separate root intent makes that work part of resolving's own durable work list.
The cursor is written after journal settlement so retrying an unacknowledged unit cannot bury a
transaction whose commit decision has already become durable.

Journal and claim cleanup precedes the recovery callback because ordinary writes and nonce claims
use those same records. Completing metadata first or treating every body as schema-only would make
an interrupted unit disappear. A callback that is not supplied is therefore a visible refusal, not
a no-op. The engine retains the schema-only baseline alongside the declared-body path.

The marker-format barrier is needed because a pre-upgrade implementation can ignore unknown fields
and clear a fence without understanding the root intent. Offline migration is preferable to a
homogeneous-deployment assumption; preserving existing format-8 metadata avoids silently resetting
reader floors while introducing the recovery contract.

A store-owned Canopy migration cannot be defined from an arbitrary unit count or a caller closure.
It needs a concrete schema change. The engineer chose to reject unsupported transitions and keep
that specification-dependent work queued, rather than invent a format transformation. The generic
engine, interruption protocol and agent units are independent of that missing specification.

## Upgrade process checkpoints

The canonical record is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Upgrade boundaries are checked across
processes". Outright process termination proves the operating system releases tier and exclusion
holds without a destructor; a thread or injected return cannot prove it. Intent and cursor
checkpoints distinguish unfinished units from a finished body whose schema or intent cleanup
is interrupted. Document batches make a resumable cursor accountable to actual migrated data.

A reader-gate acknowledgment separates a blocked admission attempt from process startup.
Registry membership is checked beside the blockers rather than inferred from a wait time.
Competing conversions are staged with both source attachments present so the release-before-gate
ordering is exercised, and a changed source is a refusal instead of a fabricated second success.

## Bounded process evidence

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Bounded process measurements retain
censoring". Compressing an exponential hour lifetime into milliseconds would change the gate and
tier workload instead of shortening its observation. A finite window preserves that distribution
and records a lower bound when a handle still blocks; it cannot establish the monthly-upgrade
budget. Conditioning the cooperative workload on one burst likewise preserves its member work
without claiming to observe repeated 120-second periods or a stationary service queue.

An armed notice must be followed by a count sample because its callback can run at the deadline
without a writer. Treating that ring as publication success would silently validate a notification
experiment whose measured writer never advanced the count. Paired seeded schedules isolate mode
differences without asserting that the operating system replays the same scheduler decisions.
The observed publication calls include the filesystem and notification behavior they really incur.

A reduction that fills absent measurements with zero turns a rare writer's empty window into a
false low-latency claim. Null measurements and absent confidence intervals keep that distinction
visible. Independent replications support intervals of the finite-window estimates; they do not
make overlapping snapshots independent Bernoulli trials or create unobserved tails. Original
simulation claims and budgets therefore remain named beside measured results and their limits,
not replaced by a universal pass/fail label.

## Windows bounded comparison

The canonical finding is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Windows finite-window comparison".
Preserving unfinished burst work prevents a short campaign from claiming that the members it
never ran would have met the budget. The oversized-burst breach survives that limitation because
the operations actually observed already exceed the absolute target. The processor-scale case
does not support the same inference, so it cannot undo the deliberate unordered baseline.

Paired metadata schedules compare the same writer arrivals across notification modes, while the
armed-notice control distinguishes a changed count from the primitive's deadline callback. Both
controls matter: a low snapshot failure count without writer overlap says nothing about waiting,
and a ring without a count change says nothing about publication. Finite windows leave those
missing conditions visible rather than treating them as evidence for the rare-failure budget.

## Linux bounded comparison

The canonical finding is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Linux finite-window comparison".
The kernel executing a workload and the filesystem carrying its durable operations are separate
variables. A Linux executable over a Windows-backed WSL mount establishes real Linux locking
and notification behavior, but its durable hold cost is not evidence for native ext4 performance.
Keeping that distinction explicit prevents a storage mismatch from being diagnosed as a fairness
defect or an OS-only latency difference.

Reducing arrival rates after a slow calibration preserves requested utilization rather than
changing the workload to produce a convenient sample count. The consequence in a bounded window
is missing service evidence, which is recorded as unknown. The burst's capacity breach is already
visible among completed operations, while the unissued member work and long-lived attachment
drains retain their censoring. The engineer chose a separate native checkout for the next run,
without overwriting the existing checkout or rewriting this campaign's storage provenance.

## Native wait safety parity

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Native wait lending matches the
sealed safety profile". Windows handle access masks do not protect a protocol once safe code can
recover a handle; Linux eventfd has no wait-only rights at all. A sealed owned capability is the
common boundary: its safe operations wait without consuming the level, and raw target recovery
requires an unsafe promise that every use and derived alias remain nonmutating.

The engineer chose to retire the Windows legacy unsafe accessor when the published sealed
family became available and then asked for Linux's types to match that unsafe profile. Linux
therefore gains a safe sealed accessor while keeping its existing unsafe borrow for compatible
custom integration. No mutable lower-layer signal API is removed, and no formal doorbell rule
changes: producer/consumer ordering and signal control stay owned by the protocol.

## Trace corpus

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "The trace corpus". TLC's own JSON
error trace repeats both states of every transition and names nothing but the action, so the
corpus has a format of its own: the configuration's constants and the trace's origin, which the
harness needs and TLC does not record, and each action with only the variables it changes. Full
states per action were the alternative; the delta keeps a `Coordination` behaviour, with its
several dozen variables, small enough to review in a diff.

Simulation was first exported through an invariant bounding the walk's length with
`TLCGet("level")`, so that TLC would dump the walk as an error trace. For `Coordination` TLC
reported the same seven-state behaviour whatever the bound, while its printed levels showed walks
reaching it. Rather than depend on that, the exporter reads the walks TLC's simulation mode writes
as modules and replays the chosen one state by state, a second pass that costs a second or two and
yields exactly the walk TLC took in the encoding the other traces use. Many walks end within a
handful of steps -- a process gives up or dies early -- so a seed keeps the longest of several.

## Trace replay

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Replaying a trace against the code".
Three questions arose before any replay ran, and the engineer decided each.

**Controls.** Of the timelines, all but three are read from configurations that break a rule.
Replaying only the configurations that match the code would have left those timelines checked
against the model alone. Requiring the code to agree up to the broken rule and depart there keeps
them as evidence: the prefix pins the code to the timeline's setting, and the departure shows the
code does not do what the control does.

**Scope.** The single-process spec can run each store operation whole, injecting a failure or a
crash at a chosen filesystem call and comparing every intermediate state. The other specs
interleave processes, and `Coordination`'s actions -- attachment, admission, the count, the fence
-- happen inside the coordination implementations, one of which keeps its state in mapped memory
rather than files. One item for all of them would have been an item of unbounded size, so the work
was split by bridge.

**The bite check.** The plan was to remove collection's namespace sync and watch the replay of the
`DANGLING-DURABLE` counterexample fail. That counterexample is read from a model with no record
guard, and model checking the configuration as built without the sync showed the record guard
alone prevents `DANGLING-DURABLE`. Three alternatives were considered: catch the removal as a
mismatch at the `CollSync` step of a trace of the protocol as built (chosen); additionally break a
record-guard rule and replay that control's counterexample; or add a test-only switch that
disables the record guard in production code. The third was rejected because it would put a fault
mode into the store to serve a test.

## Publication replay

The canonical decisions are [DESIGN-NOTES.md](DESIGN-NOTES.md) "The publication model describes
the store's commit log" and "The PointerPublication replay".

**Which side moves.** The first replay found the store emptying the commit log where the model did
not: a rolled-back transaction and every recovery. Changing the store to match would have made
recovery leave records for the next open to replay and a failed transaction leave its ingested
blob, both behaviours the store had chosen for good reasons. Annotating the traces as expected
departures would have left most simulated behaviours unreplayable past their first failed
transaction. The engineer chose to refine the model, which is meant to describe the protocol as
built; refining it then exposed the settle's collection and the begin record, which the same
reasoning covered.

**Why the begin is a step of its own.** Folding the begin record into the step that enters the
ingest would have kept the existing timelines replaying, but that step can also be the end of a
settle, whose clearing sync the store makes before the begin. Comparing runs of calls needs each
change the store makes in order to be a step the model takes in the same order, so the begin
became a step and the one timeline that passed through it was superseded.

**Placing faults by position.** Counting calls of a kind would have failed the wrong call whenever
the store makes a call the model does not name -- a settle that finds no record file makes no
sync at all, and a record append looks at the file first. Placing a fault after the changes both
sides have made, at the first call of the kind the model's next step begins with, follows the
store through the calls the model leaves out.

## Creation decisions

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "The creation model describes the
coordination's decisions".

**Validity before choice.** The first replay of `Creation`'s simulations found a creator that had
looked at an empty root ending `occupied` or `exists` under the exclusion where the model said it
lost the race, and an opener refused for another implementation where the model said the root was
not yet a store. Choosing between the model and the code was put second: the first question was
whether the model accepts what the code does. A copy of the spec that made the code's decisions
was checked in every configuration before anything was chosen. Every property held, every control
still broke the property it targets, and one witness -- a creator losing the race between two
creators of one implementation with the same inputs -- was no longer reached, which is the takeover
itself. With validity established, refining the model kept the code's behaviour, which finishes a
dead creator's work rather than leaving it for a later creator to resume.

**Separate actions.** The fresh creator's reading under the exclusion became three actions --
`LoseRace`, `TakeOver` and `CreatedUnderExclusion` -- rather than one action with branches, so a
trace names which conclusion was reached and a timeline can cite the takeover by name.

## Creation replay

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "The Creation replay".

**Which code.** The model describes the coordination contract, and canopy's `Store::init` does not
create through it: it reserves, then works under writer exclusion. Driving `Store::init` would
have checked a creation the model does not describe, and driving the contract leaves `Store::init`
unchecked. The engineer chose to drive the contract, through both implementations, and to queue
the question of `Store::init` as a decision rather than settle it inside the harness.

**Scheduling by the model's state.** The step a process takes next is a function of its state in
the model, so the replay stops each process before the first call of that step without consulting
the trace's next action. A trace that ends with a process mid-creation therefore still leaves that
process stopped where the model has it, and the comparison after the last step is as exact as the
others.

**Settling a crash's choices.** The model's crash chooses which files that are not durable
survive, while the filesystem's crash keeps exactly what is durable. Making the survivors durable
and removing the lost before the crash makes the filesystem's outcome the model's, which a random
crash could not, so a trace with a crash replays deterministically.

## Contract bridge order

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "The contract is bridged before the
documents above it, and the shared implementation announces its steps".

**Order.** The checklist first had `SharedPublication`'s bridge before `Coordination`'s, as if the
documents could be replayed with the step scheduler alone. Its behaviours are the contract's with
documents added, so that order would have bound most of the contract inside the documents' bridge,
where a mismatch in the contract shows only as a document gone wrong. Two orders were weighed:
widening the documents' item to bind the part of the contract it needs, or binding the contract's
writer actions first, on its own traces. The engineer chose the second, with the rest of the
contract -- the registry, the upgrade, clones, reads and discovery -- as an item after the
documents.

**Stepping the shared implementation.** Its contract state is shared words, not files, so the
scheduler, which holds a process at a filesystem call, cannot hold it at those steps. Three ways
were weighed: a test-support hook in acadia through which each step announces itself and its words
can be read (chosen); bridging the file-based implementation now and queuing the shared one, which
would narrow the platform to the implementation the harness happens to reach; and running each
coordination call whole, which would lose every interleaving inside a call, the very interleavings
the model checks.

## Writer action binding

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "The contract's writer actions in the
code".

**What the abstract write stands for.** `Coordination`'s write changes two cells in turn, and with
`Txn` on is a transaction over a commit log; none of that is the contract's. Three bindings were
weighed: the replay as the contract's own client, keeping the cells, the log and staging itself
(chosen); the same for an unlogged write but a `Store` transaction of two documents for a logged
one; and every write through `Store`, which has no unlogged write of two cells. Binding each model
to the layer it describes keeps a mismatch where it lives: a fault in acadia shows in
`Coordination`'s replay, and one in canopy's transactions or recovery in `SharedPublication`'s.

**Escapes later.** The model lets a waiting writer withdraw even while the lock is free, while the
code's nearest equivalent, an acquisition that does not wait, fails only while the lock is held;
and a refusal follows eligibility, which upgrades change. Binding them with the writer actions
would have mixed a likely mismatch into the first replay of the contract, so the engineer moved
them to the item that binds eligibility and upgrades.

**New configurations rather than the old.** Of the existing configurations only two are writers
alone, and neither has a host crash, unlogged recovery or mixed writes. Two generated
configurations cover those, and are checked by the sweep like any other.

**Choosing walks.** The longest walk of a writer configuration was a death, a crash and then a
recovery started and stopped until the depth ran out, which exercises almost nothing. Keeping the
walk with the most kinds of action, from many more walks, is a selection a corpus entry opts into,
so the existing entries keep the walks they had.


## Coordination writer replay

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "The Coordination writer replay".

**Two divergences, both settled in the model.** The first replay left five file-based traces where
a recoverer stopped between publishing the count and clearing the fence, which `complete_recovery`
does in one call, and two shared traces where the fence was set after a crash that the model had
clear. A scratch copy of the spec with `RecStop` not enabled at `recClear` and `Crash` free to set
the fence passed every item-tier configuration, so the model accepts both; the engineer chose to
refine the spec to describe the code rather than split `complete_recovery`, start a rebuilt shared
state unfenced, or mark the traces as departures.

**Fabricating a fence was rejected.** The replay's crash keeps or loses the file-based records as
the model's crash chooses. Making it create a fence the model's crash set would have replayed a
behaviour the file-based implementation cannot have, so each implementation replays only the
traces that are its behaviours instead.

**Walks of the shared implementation's own.** Most walks chosen for coverage take a failing
publication, which the shared implementation cannot, so it replayed few of them. Three ways were
weighed: two constants stating an implementation's narrower behaviour and a configuration that
sets them (chosen, since TLC then checks every property under that behaviour too); having the
exporter keep only walks that avoid the actions and crashes the shared implementation cannot take,
which leaves the model unchecked under it; and leaving it at the few walks there were.


## Failed unlogged write

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "An unlogged write that fails after
its record fails unresolved".

**How it was found.** Mapping the store's unlogged write to `SharedPublication`'s steps found that
a write failing after its record's append closed its modification settled, and the next
operation's settle finished it. A scratch copy of the model with that behaviour violated
`OutcomeIsFixedOnceRecovered`: a settled-looking store with a publication pending.

**Alternatives weighed.** Fixing the store to fail unresolved (chosen), which restores the
guarantee at the cost of refusing writes until recovery; and weakening the model to accept
closing settled, which keeps writes available but lets a document change after a "settled" read
with no write to it. Correctness first decided it.


## Recovery and rollback order

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "The model recovers and rolls back as
the store does". The analysis is
[DESIGN-SESSION-2026-11-15-store-recovery-order.md](design-sessions/DESIGN-SESSION-2026-11-15-store-recovery-order.md).

**Judged by what each does to the system.** The first framing asked only which side changes to
make the store and the model agree. The engineer asked instead what each behaviour does for the
system -- writes while recovery is finishing, whether the log can be reset, crashes part-way,
performance -- and on those terms every one of the store's choices was the sounder: keeping a
rolled-back transaction's `begin` keeps the list of what to collect, emptying the log before the
fence clears closes a window with stale records, and collecting on rollback and after a
roll-forward prevents a leak.

**Alternatives weighed.** Refining `Coordination`'s client-side recovery and abort and modelling
recovery in units in the composition (chosen), which checks every property against the recovery
the store runs and adds two checks the model lacked; giving the composition client steps of its
own, which would restate `Coordination`'s commit log and break the rule that the composition
restates nothing; and keeping the model's order with the affected traces marked as departures,
which would leave the store's recovery of transactions unchecked by the replay. Changing the store
to the model's order would make the system worse in each case.

## Recovery-unit verification cost

The decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "State-space reduction for recovery-unit
verification". On 2026-11-16, the unreduced two-writer transaction candidate had explored
more than 30 million distinct states after roughly an hour, with its queue still growing.
The acquisition budget allowed the separate holds that complete recovery; raising that
budget also retained many different acquisition histories. Focused witnesses established
completion, between-unit refusal and restart of stale progress. Removing the count-restart
guard produced the expected publication-safety violation, so the guard has discriminating
evidence, but those focused checks do not discharge the broader safety obligation.

The engineer chose to establish a behavior-preserving state-space reduction. Continuing the
unreduced exhaustive runs avoids a new abstraction argument but pays the enumeration cost
on every model revision. Restricting verification to selected schedules would be cheaper,
but cannot establish safety for interleavings excluded by those schedules. A justified
reduction costs model work and reference comparisons once, while preserving the behavior
the broad checks must cover. No particular discarded distinction is justified by this
choice alone; the argument and checks are part of implementing it.

### Recovery quotient argument

The candidate separates the finite budget for ordinary requests from recovery reacquisition.
A recovery request has the same eligibility, pending-request and exclusion steps as an
ordinary request; only the bookkeeping counter ceases to consume its budget. Every bounded
reference behavior therefore remains possible after erasing the recovery contributions to
that counter. Unlimited recovery retries add behaviors rather than remove them. Safety in
this over-approximation covers the bounded reference; liveness is not inferred from it.

The fingerprint retains ordinary-request budgets, process and admission state, ownership,
fence, trust, failures, log phase, staging, document versions, collection position and keep
set, and each recovery run's progress. It omits the cached log revision, which the composition
does not use to choose an action or check a result. Abstract transaction cells keep their
relative offsets and explicit staging/prepare presence, preserving both the equality tests
and next-version arithmetic without preserving irrelevant absolute revision numbers.

Document versions and the current generation are ranked together, preserving zero, equality,
order and fresh-generation allocation. No stored version may be ahead of the generation,
and active document work must use the current holder's generation; these are explicit checks.
Remembered generations retain whether they equal the current generation, because acquisition
uses that distinction to detect an intervening holder. An absent and an older generation are
equivalent for that test. The count
keeps its parity, and each recovery run keeps whether its saved count is absent, current or
stale. A saved count must never lead the current count: completed recovery and writes move
it forward, and a host crash discards every run. Therefore a stale saved count cannot become
current without the run refreshing it. The view changes state identity for checking, not the
variables or concrete traces that the replay consumes.

This argument is scoped to the composition without reads, upgrades, callbacks or notifications;
the view asserts those restrictions. Coordination's independent checks retain those features
and their original state. Extending the composition to one of them requires extending this
argument before reusing the view. Matching small unreduced runs and the collection, refusal,
completion and stale-progress controls are necessary checks, not a substitute for the argument.

Per-acquisition request intent and readiness are retained only while that acquisition can use
them. A new request replaces its intent, and recording the acquired hold replaces readiness
and the intervening-holder result. Values left behind after release therefore cannot choose a
future action. The checker omits those inactive values but retains the recovery cursor, saved
count and remembered-generation equality that a later acquisition actually reads.

## Directory-debt recovery boundaries

The decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Directory-debt units in the publication
composition". A real Store replay returned directory-debt work before publication recovery.
The fixture's record was empty live but retained durable text, so a crash exposed it again.
Repeated recovery was not a substitute for identifying which state survived the crash.

The engineer chose to add the recovery boundaries without pulling the entire namespace model
forward. Keeping each sync and the clear under its own exclusion hold preserves the acquisition
behavior the bridge checks. A fixed document set already supplies the shard directories its
future versions use; an explicit pending-debt fixture exercises the recovery boundaries without
pretending to verify directory creation or torn appends. Those broader rules remain separately
scheduled. Ignoring the returned phase would let a replay pass while the Store performs a
different sequence of exclusion holds, and was rejected.

## Writer request outcomes

The decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "A writer's request ends served, withdrawn or
dead". The model had carried a per-request eligibility flag, an external `Disqualify` step that
cleared it, and a `Refuse` step that ended a pending request when it was clear. They entered when
the model was drafted, and no decision of the engineer's defined what event they stood for. The
nearest stated reason was that eligibility follows upgrades, but the attachment tiers already keep
an upgrade from finishing while a writer stays attached. Binding the model to the code would have
meant inventing a policy to give the flag a meaning. The engineer chose the simpler model, with
the understanding that a real cause for refusing a waiting writer would be added back with its own
decision.

## Reductions keep their original

The decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "A reduction keeps the original it answers
to". A reduction is justified by an argument and by comparison with the unreduced form. Retiring
the unreduced form once the two matched would leave the argument as the only link to the original,
and a later revision of the model could break the argument with nothing left that could show it.
Keeping the original costs space and, when it runs, time. The engineer judged that cost acceptable
while the model is developed, and wants any compression or loss of information to keep a way back
to the original unless the original is superseded or excessively large.

## The queued client contract

The decision is [DESIGN-NOTES.md](DESIGN-NOTES.md#the-queued-client-contract-is-the-public-model-2026-11-16).
M14's primary product is a model another system can compose with, not an engine API whose TLA+
model happens to omit some fields. The opaque boundary is authored and checked first; the
asynchronous session and queue API realizes it rather than defining it.

**Why expose a standalone contract.** A dependent that imports `Ring.tla`, `Coordination.tla` or a
publication composition has imported mechanism choices. It would change when a lock, doorbell,
journal phase or scheduler representation changed even if its observable behavior did not. A
standalone contract gives the dependent only command, result, request, pair, logical-store and
recovery observations. The toy dependent in M14-1 is the acceptance test: it must compile and
model-check without naming an internal mechanism variable.

**Why the public lifecycle has no started or withdrawn state.** A caller can observe submission
acceptance or refusal and completion while a pair lives. When the pair is dropped or lost, the
client no longer receives per-request terminal outcomes. Whether an accepted command was withdrawn
before store work or completed invisibly is therefore not observable. Exposing one state to hide
that distinction would force a client to depend on an engine transition it cannot query. The
contract exposes `CompletionUnavailable`, while the ring mechanism separately checks that a
pre-start withdrawal has no effect and a post-start command finishes atomically. The same behavior
also covers process death and session loss without manufacturing a false negative result.

**Why a request identity is only local.** A durable status service for every accepted request would
need identity issuance, retention, uniqueness, garbage collection, crash recovery and a query API.
It would also be unable to distinguish every application operation without storing command intent.
That is a storage subsystem with a different durability and capacity design, not a consequence of
the ring. The chosen `RequestId` therefore matches one completion to one accepted submission only
for the lifetime of its pair. A caller that needs a durable distinction places a unique token or
other idempotency fact in the transaction itself.

**Why the fundamental transaction submit is one-shot.** Automatically retrying an accepted command
can apply an operation twice when the prior result is unknown. Retry after a definite precondition
rejection is different, but deciding how to rebuild arbitrary application input belongs to the
caller. The `transform` helper is restricted to a closure that may run repeatedly and has no
external side effect; it receives an explicit attempt bound and rebuilds outside exclusion. The
caller-facing `transact` operation submits once. This keeps uncertain effects visible instead of
hiding them behind an uncheckable exactly-once claim.

**Ring scheduling alternatives.** Unordered service has the smallest implementation but permits
one continuously ready pair to pass another indefinitely. An asynchronous boundary that gives
clients no way to distinguish starvation from a slow command would make that problem needlessly
hard to diagnose and use. Strong fairness on every engine or store step is broader and would expand
the model without solving the unordered coordination waits below the ring. The selected rule is
round-robin among pairs whose head is currently eligible: one service turn each, with blocked heads
parked and reinserted at the back. It gives bounded bypass at the ring with little state and leaves
per-pair FIFO intact. A later command in one pair cannot bypass a blocked head; its pair is parked,
so its pressure does not block the engine's other pairs.

**Coordination fairness stays unchanged.** The round-robin rule does not impose an order on writer
exclusion, attaching handles or upgrades, and it does not turn a contention wait into a wall-clock
promise. The existing client model remains correct to permit a wait to continue while coordination
progress is absent. Stronger fairness returns only with evidence that `ClientLiveSpec`'s explicit
progress assumptions fail important client workloads. At that point the smallest affected layer is
reconsidered; a fairness requirement is not rejected on principle and is not added preemptively.

**Versioning and compatibility.** The implementation may refine the contract by removing admitted
behaviors. A new permitted outcome, a weakened ordering or completion guarantee, a changed meaning
of effect certainty, or a stronger dependency in `ClientLiveSpec` is a breaking client contract.
A breaking model gets a new module name and the previous model remains usable by dependents. Rust
major-versioning carries the corresponding API boundary; adding only an optional command that old
dependents need not match is compatible.

**How the boundary is held.** `ClientSpec` and `ClientLiveSpec` are independently checkable. The
ring, coordination and publication models project into the same state, and negative controls break
effect certainty, completion correlation, pair order, loss ambiguity and ready-pair service. A toy
dependent remains unchanged as the projections refine the contract. That composition proof, not
API aesthetics or the current Rust shape, is the criterion for opaqueness.

## Byte append is the bounded format-free primitive

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Store-evaluated transaction primitives
stay byte-oriented". CQ-1 establishes that time added to every writer hold affects utilization and
that a short tail matters at moderate load, but it does not justify a caller-shaped callback. It
also leaves optimistic conflict retry as the alternative whose cost the store must bound. Append is
therefore evaluated inside the store so multiple append-only submissions do not invalidate one
another's read checks, while the caller still supplies only data.

Raw concatenation is selected because absence has an unambiguous empty-byte interpretation and the
result has no format decision. Numeric operations would silently choose an integer width, byte order,
signedness and overflow behavior, which would make canopy the owner of a body format under a method
name that does not admit that responsibility. A separate format-owning layer can define those
operations later without changing the generic byte store.

The suffix and resulting-body limits make the primitive's allocation and body-to-hash work bounded.
Materializing the append as an ordinary swap before the prepare record keeps recovery and observer
semantics from acquiring a second operation language. A separate public operation enum was rejected:
it would make every future primitive a dependent's exhaustive match obligation even when that
primitive is irrelevant to the dependent.

## Compose at the request history, not at implementation variables

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Ring and store outcomes refine the
opaque client contract". Ring and the durable store models own different facts. Ring knows that an
entry was accepted, whether a pair remains available and which request may be served next. The
publication and coordination composition knows whether a store operation applied, had no effect or
left that fact uncertain and requiring recovery. Neither mechanism's internal state is a useful
contract for a dependent.

The projection therefore joins them at request correlation. A pair drop first changes availability:
queued work projects to no public completion, and running work can settle only as an unavailable
request. The later store outcome may change logical contents, but it cannot manufacture a completion
or rewrite uncertainty. An explicit post-loss settlement transition is necessary because making
pair loss erase the operation would be stronger than the implementation promises, while making pair
loss wait for the operation would hold the dependent to mechanism timing.

A `started` field was rejected from the public projection because exposing it would let a dependent
distinguish pre-start withdrawal from a running operation whose result was later lost. The client
contract deliberately offers only the unavailable completion and an uncertainty rule. The toy
model remains an unchanged instance of that contract; it does not gain Ring or Store variables.

## Creation is idempotent, and a store's nonce is set by its first user

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Store creation is idempotent". It
came from an intermittent failure of the test that runs two identical creators at once
([UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md), 2026-11-17 update). The loser's
occupancy check, made before any exclusion, saw the winner's half-built trees and refused the root
as not empty, although the store being built was exactly the one the loser asked for. The outcome a
creator saw depended on when it looked.

Two simple models remove that dependence. In the first, the root must be empty and one atomic claim
picks the only creator; every other creator is refused, whenever it arrives. In the second, creation
is idempotent: every creator with one identity ends with the same store. The engineer preferred the
second. Every creator then waits for the outcome rather than guessing it from a listing, and a
retry after a crash, a race between processes and a repeated call are the same case.

Two refinements of the idempotent model were rejected. Returning `AlreadyInitialized` to an
identical creator that waited, which the earlier decision promised, keeps the error for a store the
caller asked for and can open, and forces every caller to treat the error as success. Comparing
every creation input, as the reservation did, makes two processes creating one store disagree on
their own creation time and agent version, so idempotency would hold only within one process. The
identity a creator compares is therefore what an opener must also match, and the rest is the
winner's provenance.

Reporting whether a call made the store or found it made was also rejected. No caller in the
repository branched on the difference, and the use it invites -- seed the store only when this call
made it -- is unsafe under idempotent creation: a creator that dies after the marker and before
seeding leaves a store every later call reports as found, so it is never seeded. Seeding must be
idempotent regardless, and then the report adds nothing. What collapsing gives up is "fail if it
already exists", and that is narrower than it looks: a store of another identity is still refused,
and a caller that needs a fresh store can make its identity unique, so that the only identical
store is its own retry's. The coordination contract keeps the distinction (acadia's
`Created::Existing`), where the model and the replay hold it; canopy does not pass it on.

`Store::init` creating through `Creation::create` settles how canopy's creation relates to the
contract, which had been left open: the contract already resumes a creation with matching inputs and
decides under its exclusion, which is what idempotent creation needs, and the existing replay of the
contract then covers `init`.

A nonce derived from the inputs was the reason creation could resume after a crash, and with
identity-only inputs it would tell nothing apart that the inputs do not. A zero nonce says so.
Distinguishing this store from another of the same identity -- one removed and created again at the
same path -- is left to the first operation that needs it, which holds writer exclusion and the
caller's nonce source, so setting the value cannot race and needs no new dependency. The value is
kept in a record canopy owns rather than written into the reservation, because the reservation is
what creators of every implementation read and compare, and is never rewritten once published.

## Lowercase is the schema-2 spelling, not a transparent rename

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Caller-derived names are lowercase in
Canopy schema 2". Replacing only the public name types was rejected: path hashes, shards, blob names,
staging paths and repair records derive from those spellings, so an uppercase emitter left in the
write path would split one logical document across two persisted conventions. Changing every
emitter together gives schema 2 one address for each accepted input.

A same-format migration was also rejected. Lowercasing an existing namespace is not a spelling fix on
case-sensitive filesystems: it changes every derived owner hash and every blob reference, and on a
case-insensitive filesystem a rename of a name that folds equal may not establish a distinct durable
entry. With no important deployed stores, treating schema 1 as inadmissible is simpler and avoids
silently interpreting uppercase-schema data under lowercase semantics. Schema admission records the
boundary without pretending to migrate it. Decoder acceptance of either base32 case remains useful
because case-folding can happen in transport or filesystem presentation without changing the encoded
bytes, and because accepting both cases costs no ambiguity.

## Cancelling a request without a cancellable blocking acquire

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Coordination cancellation is a client nonblocking
loop". A blocking acquire was rejected as the cancellation mechanism because unwinding an OS wait
would add a second publication/cancellation protocol beside the coordination contract. Keeping a
caller request local and retrying `Waiting::Never` requires no lock queue and preserves the same
`WouldWait` cleanup production callers already receive. A conformance death was also rejected for
withdrawal: process termination releases locks as a crash, not as a clean client decision.

The upgrade drain is intentionally different. It is already admitted and owns a cancellable lease
token, so cancelling it reopens operations while retaining the attachment. Conversion after detach
is incremental and nonblocking through `poll_gate` and `poll_tiers`; a cancelled conversion leaves
the handle detached. Keeping these two mechanisms separate matches where the caller still owns a
pending attempt and where it already owns admitted state.
