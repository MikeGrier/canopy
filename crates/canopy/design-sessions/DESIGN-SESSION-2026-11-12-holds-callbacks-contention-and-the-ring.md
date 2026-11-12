# Design session 2026-11-12 -- holds, callbacks, contention, and the ring

A record of one long working session on branch `mikegrier/first`, written at its end so that the work
can be resumed on another machine. The decisions themselves are in [DESIGN-NOTES.md](../DESIGN-NOTES.md)
and the work in [CHECKLIST.md](../CHECKLIST.md); this file records how the session went and where to
pick up. No D-numbers were assigned; the decisions are the DESIGN-NOTES sections named below.

## What was decided and done

- **The coordination model refines the acquisition model** (MP-4.10). `CoordinationAcquisition.tla`
  maps the coordination state onto `Acquisition.tla` and TLC checks `AcquisitionRefined` in every main
  configuration. The acquisition model gained escapes (give up, withdraw, cancel a drain), refusals,
  deaths and a crash, and the finding that an attaching handle can be passed over forever when
  upgrade starts are not counted, which is an input to ODI-2. DESIGN-NOTES: "The coordination model
  refines the acquisition model".
- **Every hold ends, checked in the coordination model** (MP-4.11), with fairness on a holder's own
  steps and on releases, and a control for each fairness condition removed. The cyclic workload and
  the `Live-*` configurations were retired. DESIGN-NOTES: "Every hold ends, checked in the
  coordination model".
- **No caller code runs under exclusion but the commit observer** (MP-4.12, ODI-7). Transactions are
  built without exclusion and submitted once (`transact` with an operation list and an options
  value; a builder records reads as checks; `transform` becomes a retry helper), so nothing nests
  under a hold and the nesting was removed from the model, which made the hold-ends checks several
  times faster. The callback rule -- waits on nothing, does not call into the store, returns, is
  bounded in time -- is a step (`Observe`) and three controls behind the `Callbacks` switch. The
  three workloads (a service, a cooperative database, a ubiquitous metadata store whose schema is
  upgraded at worst monthly) are recorded there.
- **How long the contract makes a caller wait** (CQ-1). `crates/contention-sim` is a seeded
  discrete-event simulator checked against the M/G/1 baseline. Findings: a service is bounded by
  utilization, not by the order among waiters; bursts about as large as the number of processors
  are where order matters; the snapshot's spinning retry fails whenever a write overlaps it (a
  defect, queued as RD-1); a monthly upgrade waits on the drain of long-lived attachments, not at
  the gate; a callback's time is paid by every writer.
- **A change to the coordination state is delivered as a notification.** Waiting work holds no
  thread: it registers on the coordination interface, and the notification rings the engine's
  doorbell. The default is one poller per process; on Windows an opportunistic lock on the
  published count, broken by the rename of a fresh count, can deliver it from an I/O completion
  (RD-2, which must first confirm that the rename breaks the lock).
- **A client reaches the store through a session and its queue pairs.** The engineer's model: all
  work is put on a queue and the client awaits the completion entry; sessions and queues are
  strictly in-process; a submission doorbell and a completion doorbell; one producer and one
  consumer per queue, exclusivity by construction (`Send`, not `Sync`, `&mut` push and pop);
  completion reserved at submission; per-pair order; an atomic operation is never cancelled, and
  dropping a pair or an enumeration's handle is dropping, not cancelling.
- **The ring: a portable protocol over one platform primitive, behind a contract spec.** Taken from
  the engineer's `durable-ioring` and `windows-waitable-queues` in MikeGrier/windows-threadpool-sys,
  built here as the module `canopy::ring` rather than a crate of its own. acadia gets `Signal`
  (event on Windows, `eventfd` on Linux, condvar elsewhere) by `cfg`; canopy gets
  `Doorbell<S: Signal = PlatformSignal>` and generic queues, monomorphized, with `AsHandle`/`AsFd`
  for native waiting. Each layer has a contract spec and an implementation spec that TLC checks
  refines it, and everything above composes with the contract alone; below the barrier a checker of
  the Rust memory model checks the fences. The engineer means this to be the pattern
  windows-waitable-queues and durable-ioring follow later, in their own repository.

## Where to resume

Nothing below has been run since this session's commit unless it says so.

1. **Finish M3** in [CHECKLIST.md](../CHECKLIST.md): ODI-2 (decide avoidance or fairness per point of
   conflict, from MP-4.7, MP-4.8, the acquisition model's uncounted-upgrade finding and CQ-1's
   findings), then ODI-4 (whether a timeline has an executable form), then SPEC-2.
2. **M3's milestone boundary**, which this session did not reach:
   - the full sweep with `tools/run-tlc-all.ps1 -Tier all` for `Coordination.tla`,
     `Acquisition.tla`, `SharedPublication.tla`, `PointerPublication.tla` and `Creation.tla`; the
     Coordination item tier, the callback and hold-ends configurations and the Acquisition
     configurations were run during the session and came out as their headers require, but the
     milestone tier and the other specs were not run after the nesting removal;
   - the probes for every spec the milestone changed (`tools/run-probes.ps1 -Check` first, to find
     stale mutants; the nesting probe was removed from `CountAndReads.psd1`);
   - `tools/check-audience-tags.ps1` on the changed specs, and `tools/check-run-tlc-all.ps1`;
   - the workspace build in debug and release with no warnings, `cargo test -p canopy` and
     `cargo test -p contention-sim`, and `cargo doc -p canopy --no-deps`;
   - then the milestone commit with one `Completed item:` line per item, and push.
3. **Then M4** (the `STRANDED-VOTE` fix), which hands off to acadia `AC-1` and then `AC-3` (the
   platform signal), which hands off to canopy M5 `RG-1` (the doorbell's specs) and `RG-2` (the
   doorbell), ahead of `NT-1`, `MP-5`, `RD-1` and `RD-2`. The ring's own specs and queues are `QS-1`
   and `API-3` in M10.

## Decisions left to the engineer, as recorded in the checklist

- Which checker of the Rust memory model runs the doorbell and ring (`loom` or another): RG-2.
- ODI-2, ODI-4, ODI-5 and ODI-6 as written.

## Practical notes for the next session

- Configurations are generated: change the tables under `tools/` and regenerate, never a `.cfg`.
  `Callbacks.ps1` is the newest part of the Coordination generator.
- `tools/lib/Tlc.ps1`'s `Get-DefinitionAt` maps a violated action property, which TLC names only by
  source location, to the definition that holds it.
- `.ps1` and `.psd1` files are CRLF; everything else is LF; planning and design documents are 7-bit
  ASCII; spec header boxes are 77 characters wide.
