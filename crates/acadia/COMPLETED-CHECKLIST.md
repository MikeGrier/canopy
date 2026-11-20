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

## Moved 2026-11-17 -- Fixes from the fourth 2026-11-17 acadia review

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Fourth review fixes: markers, plan names, recovery
order, foreign roots and MemFs locks", which also records the findings that did not hold. Each fix
was shown by a failing test, or for the MemFs reference model by a control its probe catches,
before it was made, except where noted.

### <a id="m22-1"></a>M22-1 -- `When::OnCall(u32::MAX)` fires once, not on every later matching call. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m22-2"></a>M22-2 -- A `PathMatcher::Predicate` runs outside the rules mutex, so it may use the decorator it is installed in. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m22-3"></a>M22-3 -- `MemFs::truncate` to a length no buffer can hold is refused without a panic. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m22-4"></a>M22-4 -- `OsFs::metadata` reports a directory's length as zero, as `Metadata::len` specifies. *(completed 2026-11-17 23:47:13 -05:00)*

The failing assertion was seen under WSL, where a directory's native size is not zero.

### <a id="m22-5"></a>M22-5 -- On Unix, `publish_new` does not report success while the staging name survives. *(completed 2026-11-17 23:47:13 -05:00)*

No test: an unlink that fails after a successful link cannot be provoked portably. The contract now
states that a failure other than `AlreadyExists` may leave the file published whole.

### <a id="m23-1"></a>M23-1 -- A creation plan is refused when an artifact is a staging name of another, or names another artifact or a reserved name by a case variant or a non-UTF-8 spelling. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m23-2"></a>M23-2 -- A creator finds a marker only once it is complete: the marker is made atomically, which the creation contract requires of its maker. *(completed 2026-11-17 23:47:13 -05:00)*

`ArtifactMaker::marker` returns the marker's bytes and coordination publishes them; canopy's
genesis maker and its conformance maker moved to it in the same change.

### <a id="m24-1"></a>M24-1 -- Ending a modification when none is open changes nothing, fence included. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m24-2"></a>M24-2 -- A recovery that fails part way never leaves a settled sample beside untrusted state. *(completed 2026-11-17 23:47:13 -05:00)*

A first fix that published the count last, after setting the fence and vouching the boot record,
broke canopy's `Coordination` and `SharedPublication` replays, because the model publishes count and
trust in one step and sets no fence in recovery. The kept fix makes the file implementation's
vouched count record vouch for the state. A revert-probe that read trust from the boot record alone
failed the cut-short test on a root never vouched for; the same test on a root already vouched for
did not, so the test starts from a never-vouched root whose holder died.

### <a id="m24-3"></a>M24-3 -- Discovery never reports a writer that appeared while it looked as dead. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m24-4"></a>M24-4 -- An abandoned upgrade conversion stops reporting that it holds the gate. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m24-5"></a>M24-5 -- File registry tokens are never reused within a process. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m25-1"></a>M25-1 -- The shared state directory's name is durable before any identity is issued from it. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m25-2"></a>M25-2 -- The shared-state epoch survives a write cut short. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m25-3"></a>M25-3 -- An abandoned shared participant publishes no reservation once `abandon` returns. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m25-4"></a>M25-4 -- No participant takes writer exclusion on a root reserved for another implementation. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m25-5"></a>M25-5 -- The sampling contract states that a participant's first use of the state initializes it, which can wait for another participant's initialization and never for a writer. *(completed 2026-11-17 23:47:13 -05:00)*

A documentation change to the contract; no behavior changed.

### <a id="m26-1"></a>M26-1 -- `MemFs` range locks follow the file, not the name. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m26-2"></a>M26-2 -- A `MemFs` rename between two names of one file changes nothing. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m26-3"></a>M26-3 -- `MemFsDurability.tla` does not restore the children of a removed and recreated directory. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m27-1"></a>M27-1 -- A burst of due notifications costs linear work under the process-wide mutex. *(completed 2026-11-17 23:47:13 -05:00)*

No test: the change is to cost, not behavior; the notification tests cover what it must preserve.

### <a id="m27-2"></a>M27-2 -- Source comments cite design decisions, not work-item ids. *(completed 2026-11-17 23:47:13 -05:00)*

### <a id="m27-3"></a>M27-3 -- The findings that did not hold are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). *(completed 2026-11-17 23:47:13 -05:00)*

## Moved 2026-11-18 -- Fixes from the fifth acadia review

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Fifth review fixes: recovery's commit point, restart
repair, foreign roots and the marker under abandonment", which also records what did not hold. Each
fix was shown by a failing test before it was made, except where noted.

### <a id="m28-1"></a>M28-1 -- A shared participant whose state was opened before a foreign reservation appeared takes no writer exclusion on that root. *(completed 2026-11-18 00:51:19 -05:00)*

### <a id="m28-2"></a>M28-2 -- Shared recovery never shows a settled sample beside untrusted state, to a concurrent reader or after a death between its stores. *(completed 2026-11-18 00:51:19 -05:00)*

No test: nothing can pause between two atomic stores without a new step in the hook vocabulary.

### <a id="m28-3"></a>M28-3 -- A first recovery cut short after its count leaves recovery required for the next writer. *(completed 2026-11-18 00:51:19 -05:00)*

The cut-short test now runs on an unfenced fresh root too, where it failed at the boot-record write
after the count. A first fix that skipped removing an absent fence was refused by canopy's
`Coordination` replay, which needs `RecClear` on every recovery; the kept fix fences the hold when
the removal fails.

### <a id="m28-4"></a>M28-4 -- An acquisition cut short while it repairs state from another boot leaves state the next acquisition repairs. *(completed 2026-11-18 00:51:19 -05:00)*

### <a id="m29-1"></a>M29-1 -- A creator abandoned while it publishes the marker publishes none once `abandon` returns. *(completed 2026-11-18 00:51:19 -05:00)*

The test failed for the file implementation; it asserts the paused publication was reached, which
a first version of it did not.

### <a id="m29-2"></a>M29-2 -- Plan names fold by the Unicode simple uppercase mapping, so every simple-case alias is a duplicate. *(completed 2026-11-18 00:51:19 -05:00)*

### <a id="m30-1"></a>M30-1 -- The `Vfs` contract states that `metadata` and `exists` follow symbolic links, as every path operation does, while `read_dir` names a link as `Other`. *(completed 2026-11-18 00:51:19 -05:00)*

The finding asked for link metadata; the contract was clarified instead, and a Unix test pins it.

### <a id="m30-2"></a>M30-2 -- `MemFs` keeps no replacement mark for a path once its absence is durable and nothing carried needs it. *(completed 2026-11-18 00:51:19 -05:00)*

### <a id="m30-3"></a>M30-3 -- A `MemFs` read refused by a lock copies nothing. *(completed 2026-11-18 00:51:19 -05:00)*

No test: a cost-only change.

### <a id="m30-4"></a>M30-4 -- `OsFs::read` and `read_at` on a directory on Windows fail as `IsADirectory`. *(completed 2026-11-18 00:51:19 -05:00)*

### <a id="m30-5"></a>M30-5 -- A Linux range lock past what `off_t` holds is refused rather than truncated. *(completed 2026-11-18 00:51:19 -05:00)*

No test: no supported target has an `off_t` narrower than `LockSpan`'s bound.

### <a id="m31-1"></a>M31-1 -- The notification tests' counters use acquire and release orderings. *(completed 2026-11-18 00:51:19 -05:00)*

### <a id="m31-2"></a>M31-2 -- The fifth review's findings that did not hold, and the decisions it changed, are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). *(completed 2026-11-18 00:51:19 -05:00)*

## Moved 2026-11-18 -- Fixes from the sixth acadia review

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Sixth review fixes: an unidentified boot, inspection
under abandonment, the reported fence, joining an ending epoch", which also records what did not
hold. Each fix was shown by a failing test before it was made.

### <a id="m32-1"></a>M32-1 -- File recovery on a filesystem that cannot identify its boot fails and changes nothing. *(completed 2026-11-18 03:34:53 -05:00)*

### <a id="m32-2"></a>M32-2 -- Abandonment never releases writer exclusion while a discovery inspection runs. *(completed 2026-11-18 03:34:53 -05:00)*

The test failed for the file implementation; a first version of it never reached the inspection,
because discovery on a root with no state directory has no probe to lend.

### <a id="m32-3"></a>M32-3 -- A recovery whose fence removal fails is seen as requiring recovery by every participant at once: does not hold, recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). *(completed 2026-11-18 03:34:53 -05:00)*

### <a id="m32-4"></a>M32-4 -- Shared discovery reports the fence as set only when it is set. *(completed 2026-11-18 03:34:53 -05:00)*

### <a id="m33-1"></a>M33-1 -- A participant joining a live shared epoch keeps the epoch it observed, or decides again. *(completed 2026-11-18 03:34:53 -05:00)*

Reproduced over `MemFs` only; the native case was judged not to hold.

### <a id="m33-2"></a>M33-2 -- An abandoned shared participant records no incomplete membership once `abandon` returns. *(completed 2026-11-18 03:34:53 -05:00)*

### <a id="m34-1"></a>M34-1 -- `MemFs` drops replacement marks for durably absent children of a carried directory. *(completed 2026-11-18 03:34:53 -05:00)*

### <a id="m34-2"></a>M34-2 -- `OsFs::read_at` returns an error, never panics, for a range no buffer can hold. *(completed 2026-11-18 03:34:53 -05:00)*

Reproduced on a 64-bit target under WSL with a sparse 8 TiB file, which aborted the process.

### <a id="m34-3"></a>M34-3 -- `OsFs::remove_file` on a directory fails as `IsADirectory` on Windows. *(completed 2026-11-18 03:34:53 -05:00)*

### <a id="m35-1"></a>M35-1 -- The sixth review's findings that did not hold, and the decisions it changed, are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). *(completed 2026-11-18 03:34:53 -05:00)*

## Moved 2026-11-18 -- Fixes from the seventh acadia review

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Seventh review fixes: shared discovery probes, the
fence as reported, opening under abandonment, the notification poller". Each fix was shown by a
failing test before it was made, except the oplock acknowledgement's.

### <a id="m36-1"></a>M36-1 -- One observer's discovery never hides a dead writer from another's. *(completed 2026-11-18 06:30:09 -05:00)*

### <a id="m36-2"></a>M36-2 -- A shared participant that cannot identify its boot reports the fence as it is and never samples a settled count. *(completed 2026-11-18 06:30:09 -05:00)*

### <a id="m37-1"></a>M37-1 -- A shared joiner that cannot find a live epoch's words fails rather than retrying for ever. *(completed 2026-11-18 06:30:09 -05:00)*

### <a id="m37-2"></a>M37-2 -- A shared participant abandoned during its first use changes no durable state once `abandon` returns. *(completed 2026-11-18 06:30:09 -05:00)*

### <a id="m37-3"></a>M37-3 -- A shared notification's registration makes no filesystem call under the poller's mutex. *(completed 2026-11-18 06:30:09 -05:00)*

### <a id="m38-1"></a>M38-1 -- A `MemFs` directory rename never carries a file as the moved directory's snapshot. *(completed 2026-11-18 06:30:09 -05:00)*

### <a id="m38-2"></a>M38-2 -- A live file-registry entry whose record cannot be read is listed as unknown, not an error. *(completed 2026-11-18 06:30:09 -05:00)*

### <a id="m38-3"></a>M38-3 -- A Windows oplock break whose acknowledgement cannot be issued releases the lock rather than holding the writer. *(completed 2026-11-18 06:30:09 -05:00)*

No test provokes the failure; the fix rings the notice and closes the handle, which answers the
break.

### <a id="m39-1"></a>M39-1 -- The seventh review's findings that did not hold, and the decisions it changed, are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). *(completed 2026-11-18 06:30:09 -05:00)*

## Moved 2026-11-18 -- Fixes from the eighth acadia review

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Eighth review fixes: a hold from an ended boot, the
recovered count, a missing generation, mutexes free of filesystem calls". Each fix was shown by a
failing test before it was made, except the oplock acknowledgement's.

### <a id="m40-1"></a>M40-1 -- A file hold from before a restart never changes its successor's records. *(completed 2026-11-18 06:56:00 -05:00)*

### <a id="m40-2"></a>M40-2 -- File recovery publishes the visible count rounded up to even. *(completed 2026-11-18 06:56:00 -05:00)*

The shared implementation already did: its epochs start at an odd count.

### <a id="m40-3"></a>M40-3 -- A missing generation record beside a current boot record is refused, not read as zero. *(completed 2026-11-18 06:56:00 -05:00)*

### <a id="m41-1"></a>M41-1 -- No filesystem call is made under a shared participant's mutex. *(completed 2026-11-18 06:56:00 -05:00)*

Scoped to reading the boot; publications made under the hold table stay under it, so that `abandon`
waits for them.

### <a id="m41-2"></a>M41-2 -- A shared first use changes no durable state once `abandon` returns, including its bootstrap. *(completed 2026-11-18 06:56:00 -05:00)*

### <a id="m41-3"></a>M41-3 -- A panic while a poller samples a participant never stops the poller. *(completed 2026-11-18 06:56:00 -05:00)*

### <a id="m42-1"></a>M42-1 -- A Windows oplock acknowledgement that completes with an error releases the lock. *(completed 2026-11-18 06:56:00 -05:00)*

No test provokes the failure; a probe showed a successful acknowledgement completes with status
zero.

### <a id="m42-2"></a>M42-2 -- The safe Windows native wait cannot take ownership of a mutex. *(completed 2026-11-18 06:56:00 -05:00)*

### <a id="m42-3"></a>M42-3 -- The reservation-abandonment test fails when publication is not exercised. *(completed 2026-11-18 06:56:00 -05:00)*

### <a id="m43-1"></a>M43-1 -- The eighth review's findings that did not hold, and the decisions it changed, are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). *(completed 2026-11-18 06:56:00 -05:00)*

## Moved 2026-11-18 -- A crash is a host restart in every filesystem

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Three failures, two events: a panic, a process
ending, a host restart".

### <a id="m44-1"></a>M44-1 -- The `Vfs` contract states that a host restart ends every process and that an ended process issues nothing, and `MemFs::abandon` refuses the abandoned view's later operations and fails its lock waits in progress. *(completed 2026-11-18 13:36:02 -05:00)*

### <a id="m44-2"></a>M44-2 -- `MemFs::simulate_crash` ends every process view made before it -- locks released, waits failed, later operations refused -- and canopy's callers take a post-crash view. *(completed 2026-11-18 13:36:02 -05:00)*

The controller that invokes `simulate_crash` receives a fresh process identity before returning;
every other process value keeps its ended identity. A waiter on a lock file lost to the crash and an
old file-registry attachment were both shown unable to act on their successors.

### <a id="m44-3"></a>M44-3 -- The coordination boot checks that defended against a process acting across a simulated restart are removed. *(completed 2026-11-18 13:36:02 -05:00)*

The shared implementation still reads the boot identity as a backend-liveness operation, but no
longer compares it to decide whether a process is alive.

## Moved 2026-11-18 -- A panic ends the participant it unwinds through

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Three failures, two events: a panic, a process
ending, a host restart".

### <a id="m45-1"></a>M45-1 -- Every coordination guard that keeps bookkeeping ends its participant as `abandon` does when dropped while a panic unwinds. *(completed 2026-11-18 14:29:13 -05:00)*

Writer holds, attachments, upgrades, discovery probes and creation holds are exercised through
both file and shared implementations. An old handle is refused, and a fresh participant can
recover or take the root's next work.

### <a id="m45-2"></a>M45-2 -- A participant mutex poisoned by a panic ends the participant and refuses with `Abandoned`; process-wide mutexes recover from poison with their reason at the lock site. *(completed 2026-11-18 14:29:13 -05:00)*

File holds, shared holds and shared initialization each have a poison test. Pollers, the weak model
cache and registration cleanup are process-wide and recover.

### <a id="m45-3"></a>M45-3 -- The workspace's own binaries build with `panic = "abort"` in release. *(completed 2026-11-18 14:29:13 -05:00)*

## Moved 2026-11-18 -- Ninth review: shared opening, generations, MemFs sync and Windows

### <a id="m46-1"></a>M46-1 -- A shared `abandon` never returns while an opening it raced still holds the epoch's liveness lock. *(completed 2026-11-18 14:53:52 -05:00)*

### <a id="m46-2"></a>M46-2 -- A shared generation reservation missing or behind the mapped limit at renewal is refused, not restarted at zero. *(completed 2026-11-18 14:53:52 -05:00)*

### <a id="m46-3"></a>M46-3 -- A `MemFs` directory sync across a carried snapshot never fails after changing durable state. *(completed 2026-11-18 14:53:52 -05:00)*

A revert-probe restored the previous anchor rule and made the exact rename/retype sync fail with
`NotADirectory`; restoring the carried-ancestor anchor made it pass.

### <a id="m46-4"></a>M46-4 -- `wait_native` on Windows accepts only events a wait does not reset. *(completed 2026-11-18 14:53:52 -05:00)*

### <a id="m46-5"></a>M46-5 -- A failed oplock acknowledgement rings only after the handle is closed, even when no thread can be started at teardown. *(completed 2026-11-18 14:53:52 -05:00)*

One process-wide cleanup worker is established before native registration. If it cannot start,
registration falls back to the poller instead of granting an oplock it cannot release safely.

## Moved 2026-11-18 -- Ninth review: creation, fault injection and tests

### <a id="m47-1"></a>M47-1 -- An artifact name with an embedded NUL is refused before the root is reserved. *(completed 2026-11-18 15:21:00 -05:00)*

The invalid-plan matrix reaches the NUL case through both coordination implementations and over
both MemFs and OsFs; the root remains empty. A revert-probe without the guard failed the case.

### <a id="m47-2"></a>M47-2 -- `FaultyFs::read_at` never returns more than the range asked for. *(completed 2026-11-18 15:21:00 -05:00)*

Replacement corruption is bounded to the length the undecorated range read returned, including
zero-length, short, truncated and past-EOF reads. A revert-probe returned the full replacement for a
zero-length read and failed the test.

### <a id="m47-3"></a>M47-3 -- The marker-abandonment test fails when publication is not exercised. *(completed 2026-11-18 15:21:00 -05:00)*

The test synchronizes the abandonment thread, joins it, and unconditionally asserts the guarded
ordering, a permitted creation outcome and the marker state. A revert-probe that bypassed the file
publication guard made the assertion fail because `abandon` returned in the publication callback.

## Moved 2026-11-18 -- Ninth review records

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Ninth review fixes: restart as process end,
handoff, reservations, carried sync, event and oplock capabilities" and
"Three failures, two events: a panic, a process ending, a host restart". Rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) "The ninth review, 2026-11-18".

### <a id="m48-1"></a>M48-1 -- The ninth review's findings that did not hold, and the decisions it changed, are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md). *(completed 2026-11-18 15:28:47 -05:00)*

All ten findings held. Findings 1 and 2 were fixed at the Vfs process boundary in M44, findings 3 to
7 in M46, and findings 8 to 10 in M47; the record states which ninth-review fixes supersede parts of
the eighth review.

## Moved 2026-11-18 -- Tenth review: recovery, participant boundaries and platform capabilities

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Tenth review fixes: interrupted renewal,
participant boundaries and platform capabilities". Rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) "The tenth review, 2026-11-18".

### <a id="m49-1"></a>M49-1 -- Interrupted generation renewal resumes an exact one-block-ahead durable reservation. *(completed 2026-11-18 17:25:01 -05:00)*

Missing, behind and arbitrary mismatched reservations remain malformed.

### <a id="m49-2"></a>M49-2 -- Poisoning shared liveness ends acquisition without a recursive hold-table lock. *(completed 2026-11-18 17:25:01 -05:00)*

### <a id="m49-3"></a>M49-3 -- A panic through a shared-state guard ends creation on the public participant too. *(completed 2026-11-18 17:25:01 -05:00)*

### <a id="m49-4"></a>M49-4 -- Windows oplock teardown transfers one registration to one cleanup owner. *(completed 2026-11-18 17:25:01 -05:00)*

Concurrent notice withdrawal and channel failure cannot split the timer, I/O, ring and cleanup
sender between callers.

### <a id="m49-5"></a>M49-5 -- Sealed Windows waits accept only acadia's manual-reset event capability. *(completed 2026-11-18 17:25:01 -05:00)*

### <a id="m49-6"></a>M49-6 -- The callback panic contract states the behavior of unwind and abort strategies. *(completed 2026-11-18 17:25:01 -05:00)*

### <a id="m49-7"></a>M49-7 -- MemFs crash identity storage uses an ended prefix and a bounded current-boot abandon set. *(completed 2026-11-18 17:25:01 -05:00)*

## Moved 2026-11-18 -- Eleventh review: immutable process views and unwind-safe native boundaries

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Eleventh review fixes: immutable process views and
unwind-safe native boundaries". Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md)
"The eleventh review, 2026-11-18".

### <a id="m50-1"></a>M50-1 -- MemFs crash simulation leaves every existing process view ended and returns a distinct successor. *(completed 2026-11-18 20:06:49 -05:00)*

### <a id="m50-2"></a>M50-2 -- Shared-state panic cleanup releases the public participant's in-progress creation lock. *(completed 2026-11-18 20:06:49 -05:00)*

### <a id="m50-3"></a>M50-3 -- File-registry listing claims are unwind guards. *(completed 2026-11-18 20:06:49 -05:00)*

### <a id="m50-4"></a>M50-4 -- Windows oplock ring withdrawal drops user code outside the registration mutex. *(completed 2026-11-18 20:06:49 -05:00)*

### <a id="m50-5"></a>M50-5 -- Native oplock callbacks contain sample panics and restore callback scope. *(completed 2026-11-18 20:06:49 -05:00)*

### <a id="m50-6"></a>M50-6 -- Acadia's public sealed Windows event makes a sealed thread-pool wait. *(completed 2026-11-18 20:06:49 -05:00)*

### <a id="m50-7"></a>M50-7 -- FaultyFs read_at copies replacement bytes only through the undecorated result bound. *(completed 2026-11-18 20:06:49 -05:00)*

### <a id="m50-8"></a>M50-8 -- Crash and abandonment lock tests prove the waiter entered the conflicting wait. *(completed 2026-11-18 20:06:49 -05:00)*

### <a id="m50-9"></a>M50-9 -- Interrupted generation renewal adopts the saturated final limit. *(completed 2026-11-18 20:06:49 -05:00)*

## Moved 2026-11-18 -- Eleventh review follow-up: teardown ownership and unwind containment

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Eleventh review follow-up: teardown ownership and
unwind containment". Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) "The eleventh
review follow-up, 2026-11-18".

### <a id="m51-1"></a>M51-1 -- Oplock teardown transfers native resources before discarded ring destruction. *(completed 2026-11-18 20:48:32 -05:00)*

### <a id="m51-2"></a>M51-2 -- File registry claim guards cover registration, update and unregistration. *(completed 2026-11-18 20:48:32 -05:00)*

### <a id="m51-3"></a>M51-3 -- Both process-wide notification pollers contain destructor panic. *(completed 2026-11-18 20:48:32 -05:00)*

### <a id="m51-4"></a>M51-4 -- Shared model liveness is released before its discoverable words disappear. *(completed 2026-11-18 20:48:32 -05:00)*

### <a id="m51-5"></a>M51-5 -- Native registration-time sample panic preserves a poller ring. *(completed 2026-11-18 20:48:32 -05:00)*

### <a id="m51-6"></a>M51-6 -- Windows notification tests prove native callback and withdrawal paths. *(completed 2026-11-18 20:48:32 -05:00)*

### <a id="m51-7"></a>M51-7 -- Wait-entry assertion failure releases its blocker before scope join. *(completed 2026-11-18 20:48:32 -05:00)*

### <a id="m51-8"></a>M51-8 -- The re-entrant predicate test retains no strong fixture cycle. *(completed 2026-11-18 20:48:32 -05:00)*

## Moved 2026-11-18 -- Final eleventh review: temporary ownership and layer precedence

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Final eleventh review fixes: temporary ownership and
layer precedence". Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) "The final
eleventh review follow-up, 2026-11-18".

### <a id="m52-1"></a>M52-1 -- Temporary participant owners in both notification pollers are destroyed with per-value panic containment. *(completed 2026-11-18 21:23:34 -05:00)*

### <a id="m52-2"></a>M52-2 -- Shared coordination lock values are removed from participant mutexes before their destructors run. *(completed 2026-11-18 21:23:34 -05:00)*

### <a id="m52-3"></a>M52-3 -- FaultyFs checks ended-process precedence before fault selection and observer execution. *(completed 2026-11-18 21:23:34 -05:00)*

### <a id="m52-4"></a>M52-4 -- Every non-callback oplock withdrawal releases native I/O before destroying the withdrawn ring. *(completed 2026-11-18 21:23:34 -05:00)*

### <a id="m52-5"></a>M52-5 -- A one-shot native registration sample failure rings exactly once instead of waiting for its deadline. *(completed 2026-11-18 21:23:34 -05:00)*

## Moved 2026-11-18 -- Final review: asynchronous delivery, cleanup and restart ordering

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Final review fixes: asynchronous delivery,
complete cleanup and restart ordering". Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md)
"The final review follow-up, 2026-11-18".

### <a id="m53-1"></a>M53-1 -- A failed native registration sample rings only on an implementation thread, after native I/O is released. *(completed 2026-11-18 22:06:27 -05:00)*

### <a id="m53-2"></a>M53-2 -- Shared abandonment removes the liveness lock before its destructor can call back. *(completed 2026-11-18 22:06:27 -05:00)*

### <a id="m53-3"></a>M53-3 -- Shared abandonment attempts every cleanup category and contains lock-destructor panics. *(completed 2026-11-18 22:06:27 -05:00)*

### <a id="m53-4"></a>M53-4 -- Poller containment includes destruction of a panic payload, so one ring cannot stop a worker. *(completed 2026-11-18 22:06:27 -05:00)*

### <a id="m53-5"></a>M53-5 -- Raw MemFs operations apply ended-process precedence before invalid-input or unsupported results. *(completed 2026-11-18 22:06:27 -05:00)*

### <a id="m53-6"></a>M53-6 -- Fault restart state cannot duplicate a one-shot rule or run fault effects after the old process ends. *(completed 2026-11-18 22:06:27 -05:00)*

### <a id="m53-7"></a>M53-7 -- Lock wait timeout tests release their blocker before an assertion can join the scoped waiter. *(completed 2026-11-18 22:06:27 -05:00)*

## Moved 2026-11-18 -- Teardown availability and restart commit ordering

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Teardown availability and restart commit
ordering". Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) "Teardown availability and
restart commit ordering, 2026-11-18".

### <a id="m54-1"></a>M54-1 -- File-participant unwind cleanup contains each detached lock destructor before notifying watchers. *(completed 2026-11-18 22:24:36 -05:00)*

### <a id="m54-2"></a>M54-2 -- Unwind destruction never depends on spawning a cleanup thread after panic. *(completed 2026-11-18 22:24:36 -05:00)*

### <a id="m54-3"></a>M54-3 -- Fault counter commitment and process-end transition use one ordering with restart. *(completed 2026-11-18 22:24:36 -05:00)*

### <a id="m54-4"></a>M54-4 -- The shared-abandonment regression observes every lock destructor, so a no-op cleanup fails. *(completed 2026-11-18 22:24:36 -05:00)*

## Moved 2026-11-19 -- Merge review: ordinary release and native hints

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Merge review fixes: ordinary release and native
notification hints". Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) "The merge review
follow-up, 2026-11-19".

### <a id="m55-1"></a>M55-1 -- Every ordinary participant lock release contains a destructor panic and abandons the participant without stranding sibling locks. *(completed 2026-11-19 07:23:17 -05:00)*

### <a id="m55-2"></a>M55-2 -- Native Windows early hints do not make the deadline-only conformance test flaky. *(completed 2026-11-19 07:23:17 -05:00)*

## Moved 2026-11-19 -- Full review: admission liveness and worker teardown

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Full review fixes: admission liveness and
worker-owned capture destruction". Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md)
"The full review follow-up, 2026-11-19".

### <a id="m56-1"></a>M56-1 -- Admission and upgrade transitions refuse success after any released lock destructor abandons the participant. *(completed 2026-11-19 07:53:37 -05:00)*

### <a id="m56-2"></a>M56-2 -- Linux self-withdrawal cannot strand the epoll service when callback-capture destruction panics. *(completed 2026-11-19 07:53:37 -05:00)*

## Moved 2026-11-19 -- Corrected full review: implementation and contract alignment

Decision: [DESIGN-NOTES.md](DESIGN-NOTES.md) "Corrected full review fixes: durability, cleanup
and contract alignment". Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) "The corrected
full-review follow-up, 2026-11-19".

### <a id="m57-1"></a>M57-1 -- MemFs fsync of a direct implicit-root file makes its name and bytes durable. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-2"></a>M57-2 -- Shared sampling uses the current boot-liveness result and samples odd when identity becomes unavailable. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-3"></a>M57-3 -- File registration never returns a token after claim release abandons the participant. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-4"></a>M57-4 -- Unregister cleanup contains acquisition and destructor panics and releases all participant-owned locks. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-5"></a>M57-5 -- OsFs read_at rejects ranges above a bounded allocation ceiling before reserving or filling memory. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-6"></a>M57-6 -- File hold and upgrade operations return Abandoned before unrelated local-state errors. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-7"></a>M57-7 -- The Canopy recorder forwards process-liveness and commit hooks. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-8"></a>M57-8 -- Admission tests exercise the public transition whose own gate release abandons. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-9"></a>M57-9 -- CountNotification distinguishes an exact trigger from a permitted early platform hint. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-10"></a>M57-10 -- Rename docs require syncing each parent changed by a cross-directory move. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-11"></a>M57-11 -- Unix lock docs state the inherited-descriptor fork boundary. *(completed 2026-11-19 09:08:23 -05:00)*

### <a id="m57-12"></a>M57-12 -- Signal::lower's public ordering contract matches Windows ResetEvent semantics. *(completed 2026-11-19 09:08:23 -05:00)*

## Moved 2026-11-19 -- M58: Coordination and VFS review

- [x] <a id="ar-1"></a>**AR-1** -- Concurrent abandonment waits for guard destruction on both coordination backends.
- [x] <a id="ar-2"></a>**AR-2** -- Dead file participants reject attach and acquire without reading the reservation.
- [x] <a id="ar-3"></a>**AR-3** -- Memory process commits reject invalid paths before invoking the callback.
- [x] <a id="ar-4"></a>**AR-4** -- An exact fault matcher requires both paths to normalize successfully.
- [x] <a id="ar-5"></a>**AR-5** -- The first-use abandonment test observes the setup lock attempt while another process holds the lock.

## Moved 2026-11-19 -- M59: Coordination, publication and durability review

- [x] <a id="mr-1"></a>**MR-1** -- Native notice withdrawal detaches resources without waiting for a running deadline callback.
- [x] <a id="mr-2"></a>**MR-2** -- Abandonment waits for ordinary guard retirement through destruction on both coordination backends.
- [x] <a id="mr-3"></a>**MR-3** -- Creation checks the portable marker staging budget before reserving the root.
- [x] <a id="mr-4"></a>**MR-4** -- The rename model and MemFs bridge check source inode survival after replacing a durable file, syncing both parents and crashing.

## Moved 2026-11-20 -- Extended component storage

### <a id="name-1"></a>NAME-1 -- Specify exact logical identity and the v1 component storage format. *(completed 2026-11-20 00:09:28 UTC-05:00)*

The [storage format](DESIGN-NOTES.md#extended-component-storage-format-v1) defines direct and
hashed spellings, per-parent ownership records and the ordering between a binding and its child.

### <a id="name-2"></a>NAME-2 -- Implement durable, per-parent component name resolution above the VFS. *(completed 2026-11-20 00:19:51 UTC-05:00)*

[ComponentDirectory](src/names.rs) holds the parent's lock through resolution and child access.
It records exact UTF-8 bytes before child publication, rejects unowned paths and inconsistent
bindings, and flushes child removal before forgetting its owner. [Resolver tests](src/names/tests.rs)
exercise a second native process, interrupted sidecar publication and restart on either side
of durable child removal.

### <a id="name-3"></a>NAME-3 -- Use exact logical names for managed creation, resumption and confirmation. *(completed 2026-11-20 01:08:18 UTC-05:00)*

Both coordinators select the [managed protocol](src/coord/creation/managed.rs) explicitly.
The [contract tests](src/tests/coord/creation.rs) interrupt publication before a long Unicode
marker, resume through a new participant, and list and read distinct case and normalization
variants by their original names. Competing creators find one published layout; a managed
probe of a raw store refuses without adding resolver metadata.

### <a id="name-r1"></a>NAME-R1 -- Claim CSV ownership before replacing any stage. *(completed 2026-11-20 01:25:20 UTC-05:00)*

[ComponentDirectory](src/names.rs) publishes and flushes a header-only CSV before it may
write or replace `.names.stage`. An existing stage in an unowned directory is refused
unchanged.

### <a id="name-r2"></a>NAME-R2 -- Release managed resolver locks on abandonment. *(completed 2026-11-20 01:35:33 UTC-05:00)*

Both coordinators own the resolver lock through their participant hold table and run metadata
operations through the same abandonment fence. The creator guard is released while a paused
maker is still on the stack, and subsequent resolver work refuses the abandoned participant.

### <a id="name-r3"></a>NAME-R3 -- Refuse mismatched managed reservations before resolver admission. *(completed 2026-11-20 01:38:39 UTC-05:00)*

Managed preflight compares reservation identity and input bytes before acquiring the resolver
lock. A rejected managed attempt leaves a reservation-only raw directory byte-for-byte as it
was, so the original raw creator can resume.

### <a id="name-r4"></a>NAME-R4 -- Flush repeated removal before reporting absence. *(completed 2026-11-20 01:43:08 UTC-05:00)*

An unbound retry flushes its parent before returning `false`. The fault test fails the CSV's
final parent sync, fails the retry flush in a second process view, then proves the deletion and
empty binding table survive a crash only after a durable acknowledgment.

### <a id="name-r5"></a>NAME-R5 -- Require raw artifact names to round-trip on Win32. *(completed 2026-11-20 01:46:50 UTC-05:00)*

Raw creation and confirmation reject Win32-normalized endings, DOS device basenames, stream
syntax, control bytes and reserved punctuation before a reservation is persisted. Managed
logical names accept the same text and round-trip through their resolved physical spelling.

### <a id="name-r6"></a>NAME-R6 -- Recover CSV publication staging siblings. *(completed 2026-11-20 10:16:33 UTC-05:00)*

A compatible reservation permits only regular files that exactly match the CSV target's
publication-staging grammar. Recovery succeeds whether the CSV is absent or present; the
same sibling remains occupied without a compatible reservation.

### <a id="name-r7"></a>NAME-R7 -- Fence abandoned makers off final artifact paths. *(completed 2026-11-20 10:22:38 UTC-05:00)*

Managed makers receive unique attempt-private siblings. The participant renames a completed
artifact into its binding only while live, and existing published children are reused on
resumption. A stale callback may finish its private sibling but cannot overwrite its replacement.

### <a id="name-r8"></a>NAME-R8 -- Prevalidate managed control entries without writes. *(completed 2026-11-20 10:25:56 UTC-05:00)*

Managed creation validates the existing CSV, fixed stage and lock entry before acquiring a
resolver lock. A same-input raw layout with ordinary CSV payload is rejected without adding
metadata, and the raw creator resumes.

### <a id="name-r9"></a>NAME-R9 -- Acquire resolver locks without following links. *(completed 2026-11-20 10:32:25 UTC-05:00)*

Native lock opens use no-follow semantics for their final component and verify the acquired
handle is a regular file. Unix does not follow symlinks; Windows opens reparse points as the
entry itself and refuses non-files without touching their targets.

### <a id="name-r10"></a>NAME-R10 -- Flush visible CSV ownership before fixed-stage reuse. *(completed 2026-11-20 10:36:00 UTC-05:00)*

An existing CSV entry is parent-fsynced before any fixed-stage removal or write. A retry fault
therefore leaves only the header visible, and a crash still cannot preserve unowned stage data.

### <a id="name-r11"></a>NAME-R11 -- Complete raw Win32 device and length validation. *(completed 2026-11-20 10:39:26 UTC-05:00)*

Raw components reject the six superscript Win32 device basenames and exceed no more than 255
UTF-8 bytes; markers retain their tighter publication-staging budget. Managed logical names
remain unrestricted and round-trip those spellings.

### <a id="name-r12"></a>NAME-R12 -- Atomically claim private managed-attempt directories. *(completed 2026-11-20 11:23:31 UTC-05:00)*

Managed artifacts are made in a short directory atomically created by the claimant. Equal
sequence values across PID namespaces compete on directory creation, so one claimant receives
a private `artifact` leaf and the other advances without observing a shared writable path.

### <a id="name-r13"></a>NAME-R13 -- Contain maker publication staging in the attempt directory. *(completed 2026-11-20 11:25:12 UTC-05:00)*

A maker receives the short `artifact` leaf inside its claim. Nested `publish_new` staging stays
inside that directory, abandoned attempts do not occupy the root, and direct 212-byte logical
names resume successfully without exceeding the native component limit.

### <a id="name-r14"></a>NAME-R14 -- Refuse non-file/non-directory staged entries. *(completed 2026-11-20 11:29:59 UTC-05:00)*

Managed placement reads the exact attempt-directory entry without following it. Only a regular
file or directory can be renamed to the final binding; a symlink or other entry fails before the
marker is published.

### <a id="name-r15"></a>NAME-R15 -- Validate all visible root entries before creating the resolver lock. *(completed 2026-11-20 11:34:28 UTC-05:00)*

Managed preflight decodes the CSV and validates every root entry against registered physical
names, owned attempt directories and permitted control publications. Unregistered raw children
are refused before `.names.lock` can be added, and validation repeats under the held lock.

### <a id="name-r16"></a>NAME-R16 -- Reject space-padded raw DOS device stems. *(completed 2026-11-20 11:37:48 UTC-05:00)*

Raw DOS device comparison trims trailing ASCII spaces only from the pre-extension stem. Mixed-
case ASCII and superscript device names remain refused, while the original spelling is preserved
for managed logical-name hashing and lookup.

### <a id="name-r17"></a>NAME-R17 -- Require durable managed ownership before control reuse. *(completed 2026-11-20 12:46:55 UTC-05:00)*

A durable `.names.owner` record with versioned bytes distinguishes managed control metadata
from raw files. A map or fixed stage without it is refused unchanged.

### <a id="name-r18"></a>NAME-R18 -- Pin OS-backed resolver roots before use. *(completed 2026-11-20 12:46:55 UTC-05:00)*

Participant factories and [ComponentDirectory](src/names.rs) resolve relative OS roots once.
Locks, reservations, CSV operations and resolved children retain the same absolute root.

### <a id="name-r19"></a>NAME-R19 -- Make managed confirmation lock read-only. *(completed 2026-11-20 12:46:55 UTC-05:00)*

Confirmation validates ownership and the CSV without mutation, then waits on an existing
`.names.lock`; an absent lock remains absent and raw confirmation remains resumable.

### <a id="name-r20"></a>NAME-R20 -- Check completed markers before plan whitelists. *(completed 2026-11-20 12:46:55 UTC-05:00)*

An owned completed marker returns `Created::Existing` before registered-name comparison.
Later logical children therefore do not invalidate an identical creation retry.

### <a id="name-r21"></a>NAME-R21 -- Remove attempts after returned maker errors. *(completed 2026-11-20 12:46:55 UTC-05:00)*

Once a maker returns an error, its claimed attempt directory is removed and the parent synced.
Crash-left attempts are not reclaimed while a writer may still be alive.
