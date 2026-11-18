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

## Moved 2026-11-13 -- Waiting on a signal without a thread of the caller's own (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="ac-4"></a>AC-4 -- Add `Signal::watch`, which runs a callback on a thread pool each time the signal is found raised, on acadia's own registration of the object. *(completed 2026-11-13 12:57:30 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "A signal is watched on a thread pool, by
acadia, on its own object". Windows uses a `windows-threadpool-sys` `ThreadpoolWait` on a
wait-only duplicate of the event, re-armed after each callback; Linux a process-wide `epoll`
service with one-shot registrations; `CondvarSignal` a thread per watch. The tests run over
`CondvarSignal` and the platform's signal on Windows and on Linux under WSL: a raise runs the
callback, a lowered signal does not, a signal raised before the watch runs it, a callback leaving
the signal raised runs again, runs never overlap, many raises from many threads lead to a run that
finds them all, a drop waits for a running callback, a dropped watch is not run, a watch dropped by
its own callback returns and is not run again, and many watches of many signals each run. Removing
the Windows re-arm failed the run-again and no-overlap tests; skipping the condition-variable
watch's join failed the drop-waits test.

## Moved 2026-11-15 -- Stepping the shared implementation for conformance replay (M4 of [CHECKLIST.md](CHECKLIST.md))

### <a id="ac-5"></a>AC-5 -- A test-support step hook in the shared coordination implementation, with the step vocabulary and interface agreed with the engineer. *(completed 2026-11-15 18:07:11 -05:00)*

`SharedFactory::with_hook` gives participants that announce each step changing the shared words
before it acts, and `inspect` reads the words without a lock. Tests drive a hooked participant
through a new state's recovery, a clean release, settled, nested and unresolved operations, a
holder found dead and a host restart, and check each announcement's order and that the words a hook
reads at each are those from before the step. Writing them showed that a dead holder's record
outlives it only while another participant keeps the words' epoch. Decision:
[DESIGN-NOTES.md](DESIGN-NOTES.md#the-shared-implementation-announces-its-steps-to-a-test-support-hook-2026-11-15).

## Moved 2026-11-17 -- MemFs durability reference model

### <a id="m13-1"></a>M13-1 -- `MemFs` has an independent durability reference model, replay bridge, controls and probes. *(completed 2026-11-17 12:39:51 -05:00)*

[MemFsDurability.tla](spec/MemFsDurability.tla) models live and durable names, inode-tagged files,
separate live and durable bytes, directory-carried snapshots, rename, sync, removal and crash over a
bounded path topology. [MemFsScenarios.tla](spec/MemFsScenarios.tla) reaches the four historical
regressions named by the work item: one inode under two names after one parent sync, a directory
replacing a durable file, a moved directory carried over a replacement, and an unnamed nested
directory lost by crash. The controls break file-data sync, name sync, carried refresh and crash
rollback; each is caught by `ScenarioExpected`, and the probe corpus reports every mutant caught.

[mem_model.rs](src/tests/mem_model.rs) replays the corresponding VFS actions through `MemFs`, audits
reference counts after every step and checks the crash projection; a test-only inode hook proves the
two surviving names identify one inode. The same bridge covers ordinary create/write/fsync, lost
unsynced names, directory-name boundaries, removal and full durability cases. The design and its
rationale are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) "MemFs durability reference and
replay" and [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) "A finite path model carries the durability
facts MemFs must preserve".

## Moved 2026-11-17 -- Recursive removal in the MemFs durability reference

### <a id="m14-1"></a>M14-1 -- The MemFs durability reference models recursive directory removal, with scenarios, controls, probes and replay. *(completed 2026-11-17 15:01:39 -05:00)*

[MemFsDurability.tla](spec/MemFsDurability.tla) gains `RemoveTree` for `remove_dir_all`, and both
removals now drop carried snapshots at or beneath the removed path, as `MemFs` does.
[MemFsScenarios.tla](spec/MemFsScenarios.tla) witnesses an unsynced recursive removal undone by a
crash, a synced one surviving it, and a moved directory recreated empty after a recursive or an
empty-directory removal. Four controls and probes break the snapshot drop on each removal, leave
descendants behind, and make removal durable without a sync; each is caught.
[mem_model.rs](src/tests/mem_model.rs) replays the four scenarios through `MemFs`. A revert-probe
that skipped `drop_carried_under` in `remove_dir`, and then in `remove_dir_all`, failed the
matching recreate replay each time, so the replay pins the snapshot drop on both paths. Decision:
[DESIGN-NOTES.md](DESIGN-NOTES.md) "MemFs directory removal in the durability reference".

## Moved 2026-11-17 -- Fixes from the 2026-11-17 acadia review

### <a id="m15-1"></a>M15-1 -- `OsFs::publish_new` skips a staging name a crashed process left behind instead of failing. *(completed 2026-11-17 16:13:31 -05:00)*

`publish_new` stages through `publish_staged`, which tries up to `STAGING_ATTEMPTS` names, skips a
taken one without removing it, and fails as `Other` -- never `AlreadyExists` -- if every name is
taken. Unit tests beside `os.rs` pre-create the colliding sibling, exhaust every name, and check
that a taken target is still refused after a skipped staging name. Decision:
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Review fixes: stale staging names and abandonment during a
change".

### <a id="m15-2"></a>M15-2 -- An abandoned file-based participant changes no record once `abandon` returns. *(completed 2026-11-17 16:13:31 -05:00)*

`FileCoordination::publishing` holds the table of holds across each change to a record --
`replace_whole`, the fence, the ownership record, registry entries and the reservation -- and
refuses once the participant is abandoned; a release whose bookkeeping races an abandonment
reports nothing. Two tests pause a change in flight and abandon the participant from another
thread: one shows the abandonment waits and refuses every later change, the other races a real
acquisition and checks the state left is one a death could leave. A revert-probe that released the
table before the change failed both. canopy's shared-publication replay abandoned a store while
its call was parked inside a change, which now waits forever; its process end fails the parked call
first, as the coordination replay already did. Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md)
"Review fixes: stale staging names and abandonment during a change".

### <a id="m15-3"></a>M15-3 -- The coordination tests' atomics use acquire and release orderings rather than `SeqCst`. *(completed 2026-11-17 16:13:31 -05:00)*

## Moved 2026-11-17 -- Fixes from the second 2026-11-17 acadia review

Decision for every item below: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Second review fixes: lock-free
sampling, MemFs naming and lock checks".

### <a id="m16-1"></a>M16-1 -- A file-based sample takes no lock, so it never waits for its own participant's change in progress. *(completed 2026-11-17 17:12:24 -05:00)*

The abandoned flag is an atomic beside the table of holds, written only under the table. A test
parks a change and samples the same participant; a revert-probe taking the table in `alive()` made
the sample wait and the test fail.

### <a id="m16-2"></a>M16-2 -- File-based creation stops when its participant is abandoned while an artifact is made. *(completed 2026-11-17 17:12:24 -05:00)*

`create` checks liveness before and after each `make`. A test whose maker abandons the participant
checks that creation stops after that artifact and is resumed by the next participant; removing
the checks made it fail.

### <a id="m16-3"></a>M16-3 -- `MemFs` keeps a moved directory's durable descendants when the descendant and then the parent are synced. *(completed 2026-11-17 17:12:24 -05:00)*

A directory sync marks a path unnamed only when the durable tree lacks it after the snapshots above
it are materialized. Scenario `moved-descendant-sync` and replay
`mem_model_moved_directory_keeps_its_durable_descendant` pin it; the replay failed against the
previous `mem.rs`.

### <a id="m16-4"></a>M16-4 -- `MemFs` does not let a directory rename name a descendant the moved directory never named. *(completed 2026-11-17 17:12:24 -05:00)*

A rename carries the unnamed marks beneath the moved directory to its new path. Scenario
`moved-unnamed-child` and replay `mem_model_moved_directory_does_not_name_its_unnamed_child` pin
it; the replay failed against the previous `mem.rs`.

### <a id="m16-5"></a>M16-5 -- `MemFs` checks locks atomically with the operation they guard. *(completed 2026-11-17 17:12:24 -05:00)*

`Holds` keeps the lock table guard until the operation's change is made. A test-only hook pauses
a write just after its check while another process takes a shared lock; the test shows the lock
waits for the write and its bytes do not change. Releasing the table across the hook made it fail.

### <a id="m16-6"></a>M16-6 -- The shared implementation's in-process model is keyed as `MemFs` resolves a path. *(completed 2026-11-17 17:12:24 -05:00)*

`model_root` applies `..` and drops prefixes and `.`. Tests cover the spellings and two
participants opened through aliased paths sharing one state.

### <a id="m16-7"></a>M16-7 -- Linux `wait_native` reports a hung-up or failed descriptor as an error. *(completed 2026-11-17 17:12:24 -05:00)*

Readable data is reported first; otherwise `POLLNVAL`, `POLLERR` and `POLLHUP` are errors, as
`wait_native_any` reports them. A Linux test on a pipe whose writer closed checks both, run under
WSL through the `canopy-wsl` bridge.

## Moved 2026-11-17 -- Splitting `mem.rs`

### <a id="m17-1"></a>M17-1 -- [mem.rs](src/mem.rs) is split: its `Vfs` implementation moves to [vfs.rs](src/mem/vfs.rs). *(completed 2026-11-17 17:31:00 -05:00)*

`impl Vfs for MemFs`, with the `Resolved` enum only it uses, moved whole into the child module
`mem::vfs` in split commit `9086f92`. The fracture is the filesystem's operation surface against
the two-tree model it operates on: the operations only call the parent's private types, fields and
bookkeeping helpers, which a child module can see, so the cut needed no visibility change. The
durable-tree helpers named as the first candidate were rejected because the parent calls them,
which would have needed widening. `cargo fmt` was a no-op on the moved code, and `git blame -w
-C1 -C1` attributes only the provenance header of [vfs.rs](src/mem/vfs.rs) to the split.

## Moved 2026-11-17 -- Fixes from the third 2026-11-17 acadia review

Decision for every item below: [DESIGN-NOTES.md](DESIGN-NOTES.md) "MemFs marks travel with what
they describe; empty lock ranges; the poller retires outside its lock".

### <a id="m18-1"></a>M18-1 -- A `MemFs` directory sync drops stale unnamed marks, so a file synced where an unnamed directory stood survives a crash. *(completed 2026-11-17 19:33:25 -05:00)*

A sync keeps marks only beneath children that are live directories, and a sync that replaces a
durable directory drops the marks beneath it. Scenario `file-over-unnamed-dir` and replay
`mem_model_a_file_synced_where_an_unnamed_directory_stood_survives` pin it; the replay failed
against the committed `mem.rs`.

### <a id="m18-2"></a>M18-2 -- A moved `MemFs` directory's unnamed marks travel with its carried snapshot and apply when it is materialized. *(completed 2026-11-17 19:33:25 -05:00)*

Scenarios `rename-over-stale-mark` and `rename-keeps-old-dst`, with replays
`mem_model_a_renamed_directory_drops_its_stale_marks_when_named` and
`mem_model_a_renamed_directory_keeps_its_old_contents_until_named`, pin it. Both replays failed
against the committed code; the second shows that copying the marks to the destination at rename
time, as `M16-4` did, loses the destination's durable contents on a crash before the parent sync.

### <a id="m18-3"></a>M18-3 -- An empty byte range overlaps no `MemFs` lock. *(completed 2026-11-17 19:33:25 -05:00)*

Zero-length reads, reads at end of file, empty appends and same-length truncations under another
process's exclusive lock succeed; the test failed when `overlaps` admitted an empty range.

### <a id="m18-4"></a>M18-4 -- A `MemFs` rename of a path onto itself resolves the source first. *(completed 2026-11-17 19:33:25 -05:00)*

A missing source and a missing parent fail as on `OsFs`, and an existing path still renames onto
itself; the conformance test runs against both filesystems.

### <a id="m18-5"></a>M18-5 -- The file poller drops retired registrations after releasing its state. *(completed 2026-11-17 19:33:25 -05:00)*

A test registration whose drop withdraws another notice reaches the poller's retirement; with the
retirement inside the lock the poller deadlocked and the test hung.

## Moved 2026-11-17 -- An identical creator finds the store it asked for

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "An identical creator finds the store it asked for".

### <a id="m19-1"></a>M19-1 -- `Creation::create` returns `Created::Existing` when it finds the marker under its own implementation's reservation for its inputs. *(completed 2026-11-17 21:26:02 -05:00)*

Both implementations decide it at either reading and make the marker durable before returning.
Revert-probes pin both halves in both implementations: reporting the old refusal failed exactly the
tests of an identical creator after a finished, a racing, a waited-for and an unsynced creation, and
skipping the marker's sync failed only the tests whose crash after finding the store would lose an
unsynced marker. Canopy's `Creation` replay still holds the code to the corpus exported before
canopy's M15-1, so it fails on the traces in which an identical creator ends at a finished store
until canopy's M15-2 re-exports it.

## Moved 2026-11-17 -- A creation's own publication leftovers do not occupy its root

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "A creation's own publication leftovers are its
artifacts'".

### <a id="m20-1"></a>M20-1 -- `Creation::create` reads an artifact's `publish_new` staging sibling as that artifact. *(completed 2026-11-17 21:55:24 -05:00)*

`PUBLISHING_INFIX` and `is_publication_staging_of` name the convention in the `Vfs` contract, and
`OsFs` builds its staging names from them. A revert-probe that stopped both implementations
recognizing the sibling failed exactly the two tests of a creation resumed beside a leftover, while
a leftover with no reservation, or beside no artifact, still occupies the root.

## Moved 2026-11-17 -- A published file can be replaced at once on Windows

### <a id="m21-1"></a>M21-1 -- On Windows `publish_new` moves its staged file into place without replacing, so a rename can replace what it published at once. *(completed 2026-11-17 22:33:07 -05:00)*

The decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "A publication is a move on Windows". `OsFs`
gives the staged file its name with `MoveFileExW` and no replace flag; Unix keeps the hard link.
`publish_new`'s contract now promises the published file has one name. Two `os` tests were added:
publishing and at once replacing a file by a rename, and a directory at the name refusing the
publication without leaving a staging sibling. Revert-probe: with the hard link, the replace test
failed on this host. Canopy's cross-process creation tests, which had failed in about half of runs,
passed 20 looped runs, and the `os` tests pass under WSL.
