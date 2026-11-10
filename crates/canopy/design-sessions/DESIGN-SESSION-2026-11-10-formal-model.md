# Design session 2026-11-10 -- a formal state model for canopy

Resulting decision: [DESIGN-NOTES.md](../DESIGN-NOTES.md) "Formal state model in TLA+ (`FM-1`)".
Work queued in [CHECKLIST.md](../CHECKLIST.md) (`FM-0.1` .. `FM-5.1`).

## Trigger

The eighth code-review round (`.scratch/CODE-REVIEW-2026-11-10-040625.md`, not committed)
reported `E1`: two consecutive non-transactional writes that each fail part-way through
publishing their pointer can leave three replicas holding three different pointers
(`[C, B, A]`). No write was journalled, so a reopen cannot resolve it, and pointer repair
requires a majority.

## What a read does without a majority (as of this session)

- `Store::read_pointer` returns `StoreError::PointerUnavailable { missing, rejected,
  disagreeing }`.
- `Store::read_document` turns that into: an error if any replica was rejected; `Read::Absent`
  if a majority hold nothing; otherwise `Read::Contended(Contention::Undetermined)`.
- Nothing is retried in the store. `read_settled` loops while the caller's closure agrees;
  collection skips the document; `repair_pointer` refuses; writer-side reads give up after
  `WRITER_READ_ATTEMPTS`.

The `Contended` classification assumes a commit in flight. In the `E1` state nothing is in
flight, so every read reports contention for ever and no path converges.

## Can a reader tell a write is in flight?

Yes, within one open store, which is the only scope a reader has: `Store::open` takes the
root's exclusive lock (`D116`), so all readers share the process and the `Store`'s
modification counter. The counter is odd while a modification is in progress and even
otherwise, including after a failed one; `Store::undisturbed(before, after)` holds when no
replica could have changed between two samples. A read that samples before and after its
vote can therefore tell a stable no-majority state from contention. Recorded as `FM-2.3`;
it detects `E1`, it does not decide the fix.

## Why a model

Most serious findings across the review rounds (`C2`, `C3`, `D1`, `D2`, `E1`) were
unenumerated interleavings of partial failure, crash, and replica disagreement. Each round
patched the reported case and the next found a neighbour. A model enumerates the
interleavings mechanically; the code is then held to it.

Two layers were identified: the canopy protocol over an idealised durable filesystem, and
the filesystem durability semantics `MemFs` implements (its own recent defects show it needs
a specification of its own).

## Tool

Options considered: Stateright (Rust model checker, model in the workspace, weaker
liveness); TLA+ / TLC or Quint (standard, strong liveness and fairness, a second language
and toolchain, conformance needs a trace bridge); a hand-rolled explorer (rejected:
re-implements a model checker). The engineer chose **TLA+**, because it is the tool in
favour with the team, so specs are reviewable by colleagues.

## Conformance

Model-based testing: TLC behaviours exported as traces, replayed step by step against the
real `Store` on `MemFs` + `FaultyFs`, comparing a projection of the store's live and durable
state with the model state after every step.

## Open questions

- `E1`'s fix: resolve a prior partial publication before admitting a new version, a durable
  per-write publication record, or fencing on a failure after the first rename. To be decided
  from the model -- `FM-2.1`.
- Whether `E2`-`E6` (local defects) are fixed before the model exists. Not yet answered.
