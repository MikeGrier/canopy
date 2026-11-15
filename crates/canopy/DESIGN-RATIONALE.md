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