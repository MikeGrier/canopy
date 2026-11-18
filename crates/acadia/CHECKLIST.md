# CHECKLIST -- acadia

Decision: [canopy's DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Several processes may open one
store".

## M1 -- Non-blocking primitives for canopy's coordination layer

- [x] <a id="ac-1"></a>**AC-1** -- Add `Vfs::try_lock`, `Vfs::publish_new` and `MemFs` simulated processes with abandonment, over every backend. -> [completed 2026-11-12](COMPLETED-CHECKLIST.md#ac-1)

- [x] <a id="ac-3"></a>**AC-3** -- Add `Signal`, a sealed level with raise, lower and a bounded wait: a manual-reset event on Windows, an `eventfd` on Linux, a condition variable elsewhere. -> [completed 2026-11-12](COMPLETED-CHECKLIST.md#ac-3)
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> M5 -> `RG-1`
  > (the doorbell's contract and implementation specs). See
  > [CHECKLIST.md](../canopy/CHECKLIST.md).

## M2 -- Blocking exclusion for canopy's coordination layer

- [x] <a id="ac-2"></a>**AC-2** -- Add `Vfs::lock`, the waiting form of `try_lock`, over every backend, woken by a release or a holder's death. -> [completed 2026-11-12](COMPLETED-CHECKLIST.md#ac-2)

## M3 -- Waiting on a signal without a thread of the caller's own

- [x] <a id="ac-4"></a>**AC-4** -- Add `Signal::watch`, which runs a callback on a thread pool each time the signal is found raised, on acadia's own registration of the object. -> [completed 2026-11-13](COMPLETED-CHECKLIST.md#ac-4)
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> M5+ -> `RG-3`
  > (`Doorbell::watch`). See [CHECKLIST.md](../canopy/CHECKLIST.md).

## M4 -- Stepping the shared implementation for conformance replay

- [x] <a id="ac-5"></a>**AC-5** -- A test-support step hook in the shared coordination implementation, with the step vocabulary and interface agreed with the engineer. -> [completed 2026-11-15](COMPLETED-CHECKLIST.md#ac-5)
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> M11 -> `M11-5`
  > (`Coordination`'s writer actions bound to the code). See
  > [CHECKLIST.md](../canopy/CHECKLIST.md).
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> M11 -> `M11-5`
  > (`Coordination`'s writer actions bound to the code). See
  > [CHECKLIST.md](../canopy/CHECKLIST.md).

## M13 -- MemFs durability reference model

- [x] **M13-1** (from canopy `M13-1`) -- `MemFs` has an independent durability reference model, replay bridge, controls and probes. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m13-1)

## M14 -- Recursive removal in the MemFs durability reference

- [x] **M14-1** -- The MemFs durability reference models recursive directory removal, with scenarios, controls, probes and replay. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m14-1)

## M15 -- Fixes from the 2026-11-17 acadia review

- [x] **M15-1** -- `OsFs::publish_new` skips a staging name a crashed process left behind instead of failing. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m15-1)

- [x] **M15-2** -- An abandoned file-based participant changes no record once `abandon` returns. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m15-2)

- [x] **M15-3** -- The coordination tests' atomics use acquire and release orderings rather than
  `SeqCst`.

## M16 -- Fixes from the second 2026-11-17 acadia review

- [x] **M16-1** -- A file-based sample takes no lock, so it never waits for its own participant's change in progress. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m16-1)
- [x] **M16-2** -- File-based creation stops when its participant is abandoned while an artifact is made. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m16-2)
- [x] **M16-3** -- `MemFs` keeps a moved directory's durable descendants when the descendant and then the parent are synced. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m16-3)
- [x] **M16-4** -- `MemFs` does not let a directory rename name a descendant the moved directory never named. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m16-4)
- [x] **M16-5** -- `MemFs` checks locks atomically with the operation they guard. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m16-5)
- [x] **M16-6** -- The shared implementation's in-process model is keyed as `MemFs` resolves a path. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m16-6)
- [x] **M16-7** -- Linux `wait_native` reports a hung-up or failed descriptor as an error. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m16-7)

## M17 -- Splitting `mem.rs`

- [x] **M17-1** -- [mem.rs](src/mem.rs) is split: its `Vfs` implementation moves to [vfs.rs](src/mem/vfs.rs). -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m17-1)

## M18 -- Fixes from the third 2026-11-17 acadia review

- [x] **M18-1** -- A `MemFs` directory sync drops stale unnamed marks, so a file synced where an unnamed directory stood survives a crash. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m18-1)
- [x] **M18-2** -- A moved `MemFs` directory's unnamed marks travel with its carried snapshot and apply when it is materialized. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m18-2)
- [x] **M18-3** -- An empty byte range overlaps no `MemFs` lock. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m18-3)
- [x] **M18-4** -- A `MemFs` rename of a path onto itself resolves the source first. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m18-4)
- [x] **M18-5** -- The file poller drops retired registrations after releasing its state. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m18-5)

## M19 -- An identical creator finds the store it asked for

Decision: canopy's [DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Store creation is idempotent".

- [x] **M19-1** -- `Creation::create` returns `Created::Existing` when it finds the marker under its own implementation's reservation for its inputs. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m19-1)

  > **CROSS-COMPONENT PREREQUISITE:** `crates/canopy` -> `M15` -> `M15-1` ([Creation.tla](../canopy/spec/Creation.tla)
  > decides it first). See [CHECKLIST.md](../canopy/CHECKLIST.md).
  >
  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> `M15` -> `M15-2`
  > (the `Creation` replay). See [CHECKLIST.md](../canopy/CHECKLIST.md).

## M20 -- A creation's own publication leftovers do not occupy its root

Decision: canopy's [DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Store creation is idempotent"; the
root holds only creation's own entries before creation, chosen with the engineer on 2026-11-17.

> **CROSS-COMPONENT PREREQUISITE for:** `crates/canopy` -> `M15` -> `M15-4`, whose marker is
> published with `Vfs::publish_new`. See [CHECKLIST.md](../canopy/CHECKLIST.md).

- [x] **M20-1** -- `Creation::create` reads an artifact's `publish_new` staging sibling as that artifact. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m20-1)

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> `M15` -> `M15-4`
  > (`Store::init` creates through the contract). See [CHECKLIST.md](../canopy/CHECKLIST.md).

## M21 -- A published file can be replaced at once on Windows

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "A publication is a move on Windows". Found by canopy's
cross-process creation tests (canopy's [CHECKLIST.md](../canopy/CHECKLIST.md) -> `M15` -> `M15-6`).

- [x] **M21-1** -- On Windows `publish_new` moves its staged file into place without replacing, so a rename can replace what it published at once. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m21-1)

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> `M15` -> `M15-6`
  > (creation's tests at scale and across processes). See [CHECKLIST.md](../canopy/CHECKLIST.md).

## M22 -- Fourth 2026-11-17 review: fault injection and the OS backend

Review report: findings of the fourth acadia review (GPT-6.2 Astra), each confirmed by a failing test
before it is fixed; a finding that does not hold is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md)
instead of being changed.

- [x] **M22-1** -- `When::OnCall(u32::MAX)` fires once, not on every later matching call. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m22-1)
- [x] **M22-2** -- A `PathMatcher::Predicate` runs outside the rules mutex, so it may use the decorator it is installed in. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m22-2)
- [x] **M22-3** -- `MemFs::truncate` to a length no buffer can hold is refused without a panic. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m22-3)
- [x] **M22-4** -- `OsFs::metadata` reports a directory's length as zero, as `Metadata::len` specifies. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m22-4)
- [x] **M22-5** -- On Unix, `publish_new` does not report success while the staging name survives. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m22-5)

## M23 -- Fourth review: creation plans and markers

- [x] **M23-1** -- A creation plan is refused when an artifact is a staging name of another, or names another artifact or a reserved name by a case variant or a non-UTF-8 spelling. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m23-1)
- [x] **M23-2** -- A creator finds a marker only once it is complete: the marker is made atomically, which the creation contract requires of its maker. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m23-2)

## M24 -- Fourth review: file-based coordination

- [x] **M24-1** -- Ending a modification when none is open changes nothing, fence included. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m24-1)
- [x] **M24-2** -- A recovery that fails part way never leaves a settled sample beside untrusted state. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m24-2)
- [x] **M24-3** -- Discovery never reports a writer that appeared while it looked as dead. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m24-3)
- [x] **M24-4** -- An abandoned upgrade conversion stops reporting that it holds the gate. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m24-4)
- [x] **M24-5** -- File registry tokens are never reused within a process. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m24-5)

## M25 -- Fourth review: shared-memory coordination

- [x] **M25-1** -- The shared state directory's name is durable before any identity is issued from it. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m25-1)
- [x] **M25-2** -- The shared-state epoch survives a write cut short. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m25-2)
- [x] **M25-3** -- An abandoned shared participant publishes no reservation once `abandon` returns. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m25-3)
- [x] **M25-4** -- No participant takes writer exclusion on a root reserved for another implementation. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m25-4)
- [x] **M25-5** -- The sampling contract states that a participant's first use of the state initializes it, which can wait for another participant's initialization and never for a writer. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m25-5)

## M26 -- Fourth review: MemFs as a reference

- [x] **M26-1** -- `MemFs` range locks follow the file, not the name. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m26-1)
- [x] **M26-2** -- A `MemFs` rename between two names of one file changes nothing. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m26-2)
- [x] **M26-3** -- `MemFsDurability.tla` does not restore the children of a removed and recreated directory. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m26-3)

## M27 -- Fourth review: cost and conventions

- [x] **M27-1** -- A burst of due notifications costs linear work under the process-wide mutex. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m27-1)
- [x] **M27-2** -- Source comments cite design decisions, not work-item ids. -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m27-2)
- [x] **M27-3** -- The findings that did not hold are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). -> [completed 2026-11-17](COMPLETED-CHECKLIST.md#m27-3)

## M28 -- Fifth review: coordination trust, recovery and exclusion

Review report: findings of the fifth acadia review (GPT-6.2 Astra, 2026-11-18), each confirmed by a
failing test before it is fixed; a finding that does not hold is recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md) instead of being changed.

- [x] **M28-1** -- A shared participant whose state was opened before a foreign reservation appeared takes no writer exclusion on that root. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m28-1)
- [x] **M28-2** -- Shared recovery never shows a settled sample beside untrusted state, to a concurrent reader or after a death between its stores. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m28-2)
- [x] **M28-3** -- A first recovery cut short after its count leaves recovery required for the next writer. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m28-3)
- [x] **M28-4** -- An acquisition cut short while it repairs state from another boot leaves state the next acquisition repairs. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m28-4)

## M29 -- Fifth review: creation

- [x] **M29-1** -- A creator abandoned while it publishes the marker publishes none once `abandon` returns. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m29-1)
- [x] **M29-2** -- Plan names fold by the Unicode simple uppercase mapping, so every simple-case alias is a duplicate. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m29-2)

## M30 -- Fifth review: the backends

- [x] **M30-1** -- The `Vfs` contract states that `metadata` and `exists` follow symbolic links, as every path operation does, while `read_dir` names a link as `Other`. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m30-1)
- [x] **M30-2** -- `MemFs` keeps no replacement mark for a path once its absence is durable and nothing carried needs it. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m30-2)
- [x] **M30-3** -- A `MemFs` read refused by a lock copies nothing. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m30-3)
- [x] **M30-4** -- `OsFs::read` and `read_at` on a directory on Windows fail as `IsADirectory`. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m30-4)
- [x] **M30-5** -- A Linux range lock past what `off_t` holds is refused rather than truncated. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m30-5)

## M31 -- Fifth review: conventions and records

- [x] **M31-1** -- The notification tests' counters use acquire and release orderings. -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m31-1)
- [x] **M31-2** -- The fifth review's findings that did not hold, and the decisions it changed, are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). -> [completed 2026-11-18](COMPLETED-CHECKLIST.md#m31-2)
