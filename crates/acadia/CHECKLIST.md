# CHECKLIST -- acadia

Decision: [canopy's DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Several processes may open one
store".

## M1 -- Non-blocking primitives for canopy's coordination layer

- [ ] <a id="ac-1"></a>**AC-1** -- Add the non-blocking primitives the file-based coordination implementation
  needs for its creation reservation, admission gate, attachment tiers and registry: an
  exclusive publish-if-absent that leaves a file either complete or absent, including when
  power is lost before it is durable ([DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Creating a
  store root, modelled in `Creation.tla`"; `write` today creates or truncates); and shared and
  exclusive locks on a file or a byte range of one,
  acquired without waiting and released when the holder dies, over `OsFs` on Windows
  (`LockFileEx`) and Linux (`flock` or
  open-file-description locks, never process-associated `fcntl` locks), over `MemFs`, and with
  faults injectable through `FaultyFs`. Give `MemFs` an abandonment hook that releases a
  simulated process's holds without clean-release bookkeeping. Locked byte ranges on Windows
  refuse reads by other handles, so the primitives must let data and its lock live in different
  ranges. Test each platform, including holds released by a real process's death.
  > **CROSS-COMPONENT PREREQUISITE:** `crates/canopy` -> M4 -> `FM-2.3` completes the work
  > before this; see [CHECKLIST.md](../canopy/CHECKLIST.md).
  >
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> M5 -> `MP-5`
  > (the coordination interface and its file-based implementation). See
  > [CHECKLIST.md](../canopy/CHECKLIST.md).

## M2 -- Blocking exclusion for canopy's coordination layer

- [ ] <a id="ac-2"></a>**AC-2** -- Add a blocking exclusive lock acquisition, with the same release on death,
  over `OsFs` on Windows and Linux, `MemFs` and `FaultyFs`, for the contract's writer
  exclusion. Test waiting, a holder's death waking a waiter, and abandonment.
  > **CROSS-COMPONENT PREREQUISITE:** `crates/canopy` -> M5 -> `MP-7` completes the work before
  > this; see [CHECKLIST.md](../canopy/CHECKLIST.md).
  >
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> M6 -> `MP-8`
  > (writer exclusion in every implementation). See [CHECKLIST.md](../canopy/CHECKLIST.md).
