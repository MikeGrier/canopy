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
