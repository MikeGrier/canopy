# Design session 2026-11-15 -- the store's recovery and rollback order

Resulting decisions: [DESIGN-NOTES.md](../DESIGN-NOTES.md) "An unlogged write that fails after its
record fails unresolved" and "The model recovers and rolls back as the store does". Work:
`M11-6` in [CHECKLIST.md](../CHECKLIST.md).

## Context

Binding [SharedPublication.tla](../spec/SharedPublication.tla) to `Store` needed every model step
mapped to the store's calls. The unlogged write lined up step for step. Three divergences were
put to the engineer first:

- D1: an unlogged write that fails after its record's append closed settled. A scratch model with
  that behaviour violated `OutcomeIsFixedOnceRecovered`; the engineer chose to fix the store.
- D2: `Store::recover` takes writer exclusion once per unit, where the model recovers in one hold;
  the engineer chose to refine the model to recover in units.
- D3: the store's settle collects what a roll-forward superseded; a scratch model doing so held
  every record-guard configuration, and the engineer chose to refine the model.

The first framing of D1 and D2 offered options without saying what each cost or bought; the
engineer asked for recommendations with reasons, and they were given as tables of benefit and cost.

## What reading the store's recovery found

Working out D2 found that the store's recovery differs from the client-side recovery
`Coordination.tla` models in more than its holds:

1. A rolled-back (begun) transaction's log records are emptied only at completion; the model
   resets the log right after sweeping the staging (`RecReset`).
2. Completion empties the log, then publishes the count and clears the fence; the model empties
   the log (`Quiesce`) only after the fence is cleared.
3. D5: the store collects what a transaction rolled back before its prepare ingested, in a live
   rollback (`rollback_unprepared`) and in recovery, and recovery collects what a rolled-forward
   transaction superseded in a phase of its own; the model collects in neither.

Items 1 and 2 live in `Coordination`'s abstract client, which the composition reuses, so they
cannot be refined in `SharedPublication` alone.

## The engineer's question: what do these do to the system?

The options were first weighed as model alignment. The engineer asked to weigh more than the
model: what the behaviours do for writes while recovery is finishing, whether they prevent a log
reset, and what they cost.

- Item 1. The `begin` record is the only durable list of where the transaction may have ingested
  blobs (`D329`). Kept until completion, it survives a crash before the collection unit; emptied
  early, it would be lost and what was ingested leaked. No ordinary write is affected, since the
  fence refuses writes throughout, and a begun or ended transaction never blocks a reset -- only a
  prepare without its end does. Cost: a non-empty log for a few more steps; the next open replays
  the log only if recovery itself dies.
- Item 2. The fence comes off only once the log is clean, so no writer can begin between the
  fence clearing and the reset with stale records ahead of its own; in the model's order one
  could, and its crash would make the next recovery replay the stale records. Both failure orders
  are safe: a failed reset is ignored and retried by the next write's quiescence, and a completion
  that fails after the reset leaves the fence for a recovery that finds an empty log.
- Item 3. Without the rollback's collection an aborted transaction's blob is orphaned once the log
  is reset; without recovery's, a rolled-forward transaction's superseded version stays until the
  next write to its document. A live rollback collects before it empties the log, so a failed
  collection fails the rollback and escalates to recovery while the log still names the targets.
  Collections delete data, and the model does not yet check that these never delete a blob a
  pointer names -- modelling them adds that check.
- D2 itself. Between units, another writer waits for the lock, acquires it, is told recovery is
  required and fails fast with `RecoveryRequired`; a second recoverer may interleave units. A run
  restarts when the modification count changed between its steps -- the guard against acting on
  stale progress after another recoverer completed and a writer began -- and the model with units
  checks that guard. Each unit is a fresh acquisition (the ownership record written and the
  generation advanced durably in the file-based implementation, the handle's per-holder state
  reset), about 2n + 4 acquisitions for n transactions in the log, which is emptied after every
  write. Recovery is rare, so the cost is small and bounded.

Conclusion: in every case the store's behaviour is the sounder design; changing the store to the
model's order would make the system worse.

## Options and choice

- P1, chosen: refine `Coordination`'s client-side recovery and abort to the store's order, add the
  collections (D5), and model recovery in units in the composition. Every property is checked
  against the recovery the store runs, with two new checks: no recovery or rollback collection
  deletes a named blob, and interleaved recoverers and writers between units are handled by the
  count guard. Cost: `Coordination`'s configurations, its timelines, its probes and its corpus --
  M11-5's writer traces included -- are checked again.
- P2: give the composition its own client steps; this restates `Coordination`'s commit log and
  breaks the rule that the composition restates nothing.
- P3: keep the model and mark the affected traces as departures; the store's recovery of
  transactions would go unchecked by the replay.

D5 proceeds as D3 did: validated in a scratch copy first, refined if every configuration holds,
and raised with the engineer only if the model rejects it.
