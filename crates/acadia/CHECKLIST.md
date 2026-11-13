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

- [ ] <a id="ac-4"></a>**AC-4** -- Add `Signal::watch`, a registration that runs a callback on the
  platform's thread pool each time the signal is found raised, made by acadia on its own object so
  that the object never reaches the caller's code ([canopy's DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md)
  "The doorbell's signal is sealed"). On Windows a `windows-threadpool-sys` `ThreadpoolWait` on the
  event, armed again after each callback returns and never handing the callback its activation;
  on Linux an `epoll` registration serviced by a thread acadia owns; for `CondvarSignal` a thread
  waiting on the condition variable. The callback sees the level: one that returns without the
  signal lowered is run again at once. Dropping the registration stops it and waits for a running
  callback to return. Tests: a raise runs the callback, a lowered signal does not run it, a
  callback that leaves the signal raised runs again, many raises from many threads each lead to a
  run that finds them, and a drop during a callback waits for it; on Windows and on Linux (WSL),
  with the condition-variable signal everywhere.
  > **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> M5+ -> `RG-3`
  > (`Doorbell::watch`). See [CHECKLIST.md](../canopy/CHECKLIST.md).
