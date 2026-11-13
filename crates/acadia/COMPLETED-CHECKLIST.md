# COMPLETED CHECKLIST -- acadia

Append-only archive of completed items from [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-11-10 -- MemFs directory sync cost

### <a id="mf-1"></a>MF-1 -- Make a `MemFs` directory sync cost what changed rather than the directory's size. *(completed 2026-11-10 19:36:32 -05:00)*

A directory sync reconciled every entry, so a store syncing a partition root after each new
shard was quadratic in the shard count; profiling the content-store scale tests put most of
their time in `MemFs::fsync`. Each directory now records the names changed since its last
sync and a sync reconciles only those, or everything after a wholesale replacement. The
design and its oracle are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) "MemFs directory
sync costs what changed". A revert-probe that dropped the record a rename makes in its
destination parent failed the differential test against the full-reconcile oracle.

### <a id="mf-2"></a>MF-2 -- Add `MemFs::fork`, an independent copy of a filesystem's whole state, so a fixture is built once. *(completed 2026-11-10)*

Tests that need the same expensive populated store each built their own; a fork copies the
live and durable trees and every piece of pending bookkeeping, so a crash of the fork behaves
exactly as a crash of the original would have, and the two never affect each other. Locks are
not copied, because they belong to their holders. Recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md)
"MemFs fork".

## Moved 2026-11-12 -- Non-blocking primitives for canopy's coordination layer (M1 of [CHECKLIST.md](CHECKLIST.md))

### <a id="ac-1"></a>AC-1 -- Add `Vfs::try_lock`, `Vfs::publish_new` and `MemFs` simulated processes with abandonment, over every backend. *(completed 2026-11-12 21:51:42 -05:00)*

The decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Locks on a file or a range of one, taken
without waiting" and "Publishing a file only where nothing is". `try_lock` takes `LockFileEx`
locks on Windows and open-file-description locks on Linux, one handle per guard, and refuses a
conflict at once; `MemFs` keeps a lock table per filesystem, refuses reads and writes inside a
locked span as Windows does, and gives each simulated process its own view, whose `abandon`
releases its locks without its guards. `publish_new` stages, syncs and hard-links on `OsFs`, and
publishes durable bytes under a not-yet-durable name in one step on `MemFs`. `FaultyFs` forwards
both and can fail either. The `vfs-lock-holder` fixture in `canopy-lock-holder` checks real
processes killed outright, on Windows and on Linux under WSL. Running that suite on Linux showed
that a lock released only by closing its handle outlives the guard while a concurrently forked
child holds the description, so `OsFs` releases its Unix locks explicitly, `lock_exclusive`'s
included.

### <a id="ac-3"></a>AC-3 -- Add `Signal`, a sealed level with raise, lower and a bounded wait: a manual-reset event on Windows, an `eventfd` on Linux, a condition variable elsewhere. *(completed 2026-11-12 22:07:20 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "A signal is a level, one per platform behind
a sealed trait". `EventSignal` implements `AsHandle` and `EventFdSignal` implements `AsFd`;
`CondvarSignal` exists on every target and is `PlatformSignal` where neither does. The same tests
run over `CondvarSignal` and the platform's signal on Windows and on Linux under WSL, and a test on
each platform waits on the native object with `WaitForSingleObject` or `poll`. A revert-probe that
made the event auto-reset failed exactly the tests that depend on it being a level.

## Moved 2026-11-12 -- Blocking exclusion for canopy's coordination layer (M2 of [CHECKLIST.md](CHECKLIST.md))

### <a id="ac-2"></a>AC-2 -- Add `Vfs::lock`, the waiting form of `try_lock`, over every backend, woken by a release or a holder's death. *(completed 2026-11-12 22:14:11 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "A lock may be waited for, without a bound".
It landed ahead of canopy's MP-7, which the checklist had placed before it but on which it does not
depend. `OsFs` waits in the kernel's lock call, `MemFs` on a condition variable that every release
and abandonment wakes, and `FaultyFs` can fail it before any wait. The unit tests cover a waiter let
in by a dropped guard, every shared waiter let in by one release, a waiter for free bytes never
waiting, and a simulated death letting a waiter in while an unrelated death does not; the
cross-process suite has waiters woken by a killed holder, for a file and for a range, and a waiting
process let in when this one lets go, on Windows and on Linux under WSL. Removing the wake from
`MemFs`'s abandonment failed the simulated-death test.
