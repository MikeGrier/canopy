# Design session 2026-11-18: what a crash is

Resulting decision: [DESIGN-NOTES.md](../DESIGN-NOTES.md) "Three failures, two events: a panic, a
process ending, a host restart"; rationale in [DESIGN-RATIONALE.md](../DESIGN-RATIONALE.md) "Three
failures, two events". Work: milestones M44 and M45 in [CHECKLIST.md](../CHECKLIST.md).

## How it came up

The ninth review of the crate (2026-11-18) found two more defects -- a writer waiting across a
simulated restart taking exclusion on the old lock file, and a registry attachment from before a
restart overwriting its successor's entry -- of the same kind as the eighth review's first: state
from before a `MemFs` crash acting after it. Each had been, or would be, fixed by a boot check in
one coordination path. The question raised was whether the class should be closed at its source.

## What "crash" means

Three failures differ in what survives them:

- **A thread panic.** The process keeps running; its other threads continue; its locks stay held
  unless the unwinding drops their guards; unsynced writes survive in the page cache; the boot is
  unchanged.
- **A process ending** (killed, aborted, out of memory). Its threads are gone; the operating
  system releases its locks; its completed writes survive, synced or not, and other processes see
  them; shared mappings survive; the boot is unchanged.
- **An unexpected host restart** (power loss, OS crash). Every process is gone and every lock
  released; only synced data survives; the boot identity changes.

A clean reboot is a restart that synced everything; sleep and hibernation change nothing.

`MemFs` modelled a process ending with `abandon` (locks released; the view could still write) and a
restart with `simulate_crash` (storage lost and boot advanced, but every lock and view kept). Two
decisions a day apart had described the crash differently -- "a crash loses storage and keeps
locks", then "`simulate_crash` is its power loss and so its restart" -- and neither was a choice the
engineer had made. Together they describe a host that restarts while its processes keep running.

The formal models already say otherwise: `Crash` in the coordination model ends every process and
releases every lock, and `CrashLeavesNoLiveProcess` checks it; the creation, acquisition and
publication models agree. `MemFs` was the component out of line with them. Canopy's coordination
replay already ends every simulated process before it calls `simulate_crash`.

Options weighed for the restart: make `simulate_crash` end every view from before it; the same but
spare the handle that crashed; or keep `MemFs` and add boot checks per path. The first was chosen,
with `abandon` also refusing the ended view's operations.

## Panics

If a panic may unwind -- the default, and the library cannot choose otherwise -- then code running
during the unwind must be right for half-done work, poisoned mutexes need a policy, foreign-code
boundaries must contain panics, and a `Drop` must not panic while unwinding. acadia had the
boundaries; it ignored poison everywhere; and its holds treated an unwinding drop as a clean
release, which reports a holder that panicked between modifications as having finished.

The engineer's stance: a panic is a process ending. Enforced idiomatically, a library makes it so for
its own state: a guard dropped while `std::thread::panicking()` ends its participant as `abandon`
does; a poisoned participant mutex ends the participant; process-wide mutexes keep recovering from
poison; the workspace's own binaries use `panic = "abort"`. Rejected: aborting from a guard,
`set_hook`, and `compile_error!` under unwinding.

The engineer observed that this simplifies the formal models: three failures map onto the two
events they already have, `Terminate(p)` and `Crash`, and none gains one. The `Vfs` contract gains two
statements -- a restart ends every process, and an ended process issues nothing -- which `OsFs` has
from the operating system and `MemFs` must implement. `MemFsDurability.tla` models storage alone and
is unchanged.

## Open questions

None. The work is queued as M44 and M45.
