# DESIGN NOTES -- acadia

Crate-local design notes for the filesystem seam (D55/D56). The shared decision register lives
in `DESIGN-NOTES.md` at the root of the originating repository (not imported here); this file
elaborates the decisions that land in this crate rather than re-deciding them. Names such
as `canopy` and `canopy-core` are crates in that repository.

## D300 -- Anchored Artifact Reads

[read_root](src/read_root.rs) adds `ReadRoot` and its OS implementation `OsReadRoot`
alongside the existing path-oriented `Vfs`. This separate capability is necessary:
whole-file `Vfs::read` cannot bound allocation or bind a series of descendant opens to
one root handle. Existing storage operations retain their specified semantics.
The contract is `DESIGN-NOTES-INSTALLED-BODIES.md`; rationale is `DESIGN-RATIONALE.md`
(`bov-2-verification-mechanisms`). Both are in the originating repository, not imported here.

Every component is opened relative to a held directory handle with no-follow semantics.
Metadata checks use that opened handle, never a preceding path lookup. All symlinks and
Windows reparse points are refused, including those in the absolute root's ancestry.
Only regular files are returned; Unix uses nonblocking open to reject FIFOs without
waiting for a writer. `cap-std` and `cap-fs-ext` implement the handle-relative/no-follow
primitives selected to satisfy this contract on Windows and Unix. Windows may reject
directory renames while handles are held; Unix may permit them, but reads stay anchored.
File content may still change during reading: consumers must hash and retain exact bytes.

The artifact verifier depends only on readers, not on this crate. Its integration test
uses this capability through a closure adapter. Real Windows junction and Linux symlink,
FIFO, hard-link, and replacement tests cover the OS behavior; no metadata-then-open
fallback or platform-specific loss of protection is permitted.

## D107 elaboration -- the fault-injection rule model

> **SUPERSEDED in part, 2026-11-17, by "Fourth review fixes: markers, plan names, recovery order,
> foreign roots and MemFs locks" below.** Selecting a rule runs every matcher, predicates
> included, with nothing locked; only the chosen rule's call count is taken under the lock. The
> rest of this section stands.

[D107](../../DESIGN-NOTES.md) fixes what `acadia` guarantees every consumer for injecting and
observing storage faults. This section is the mechanism: how a rule is written and how a call is
evaluated against it. The implementation is [src/fault.rs](src/fault.rs); the conformance suite
that holds it to this model is [src/tests.rs](src/tests.rs).

### A rule has four independent parts

A rule is a product of four choices, each of which can vary without disturbing the others:

| Part | Question it answers |
|---|---|
| `FaultOp` | which `Vfs` operation is intercepted |
| matcher | which **paths** that operation must be acting on |
| gate + `When` | **whether this call** is elected |
| effect | what happens to the elected call |

Keeping these orthogonal is the whole design. The three decorators D107 retires each fused two
or more of them into hand-written control flow -- a matching rule welded to an armed flag welded
to the failure -- which is why none of them could be reused by the next consumer that needed a
variation on one part.

### Evaluation order, and why it is this order

A rule **applies** to a call when three things hold together: its `FaultOp` is the operation
being performed, its **gate is open**, and its matcher accepts the path. Rules are considered
**in insertion order** and the **first applicable** rule decides the call; later rules are not
consulted for it, so a single rule with a `When` schedule expresses a per-path sequence rather
than needing several rules that would race for the same call.

The schedule is then counted over the calls the rule **applied to** -- not over all filesystem
traffic, and not over calls the op and matcher accepted while the gate was shut. That last part
is the load-bearing one, and it is why the gate is evaluated *with* the match rather than after
it: a closed gate that still counted matches would let setup traffic silently consume a
`When::Once` budget, and the rule would then never fire after the gate opened. Because a closed
rule consumes nothing, "open the gate, then fail exactly once" fails the first matching call
*after* opening, which is the composition the gate exists to provide.

A closed rule likewise **claims** nothing: it does not decide the call, so a later rule matching
the same path still gets its turn. A gate therefore reads as "this rule is not live yet" rather
than as "this rule swallows the call and does nothing", which is the reading that would make a
policy's behaviour depend on the order in which unrelated rules were added.

### Path matching

Paths are compared after **normalization** to their named components under the single implicit
root: a prefix, a root, and `.` are dropped, and `..` pops. Matching therefore never depends on
how a caller spelled an equivalent path.

The matcher vocabulary is exact-equality (the default), file name, file-name prefix,
contains-a-named-component, and a caller-supplied predicate. The first four are declarative and
readable as data, which is the property that makes a policy inspectable; the predicate is the
deliberate escape hatch, and its existence is an admission that the vocabulary will not be
complete. A predicate that could be spelled with one of the first four should be, because a
predicate is opaque to anything that wants to reason about the policy.

**The prefix variant marks a layer boundary, and it is worth knowing why it is spelled this
way.** It exists because a store writes a document through a staging temp, `<document>.tmp.<nonce>`,
so a rule naming the bare document never matches the write. The tempting fix -- a matcher that
takes a document and knows how to find its staged path -- would drag the temp-name convention
down into this crate, where it does not belong: `acadia` sees paths, not documents. "Final
component starts with this string" needs no knowledge of any consumer's layout, so it is the
spelling this layer can own honestly. A consumer that wants a document-aware matcher builds one
on top, in the crate that owns the convention.

A `FaultOp::Rename` rule matches on the operation's **source** path. Renames are how the store
publishes an atomic result, so the interesting fault is "the publish failed", which is named by
where the bytes are coming from.

### Effects, and the one that does not fail

`Fail`, `ShortWrite`, and `Corrupt` change the outcome of the operation. Faults apply only where
they are meaningful -- a `Corrupt` on a write, or a `ShortWrite` on a read, passes through
unchanged -- because the alternative is a rule that silently does nothing for a reason the
author cannot see.

An **observing** rule is different in kind: it leaves the operation's outcome alone and instead
records the call and/or runs a caller-supplied action *while the operation is in flight*. The
store is mid-operation when that action runs, so the action may re-enter the store. This is the
point (it is how a test provokes an interleaving that is otherwise unreachable) and it is also
the hazard, so it is stated in both places rather than left to be discovered.

Re-entrancy is what forces the shape of the interception: selecting a rule takes the policy's
lock, and an action that re-enters this same decorator selects again, so running the action
under that lock would self-deadlock on the first re-entrant call -- the exact case the facility
exists to support. Selection and observation are therefore separate steps, and the action runs
only after the rule state is unlocked. An observing rule also reports **no fault** to the
operation, which is how "observe and proceed unchanged" avoids needing a distinct arm in each
of the `Vfs` methods.

### Determinism is a property of the whole model

There is no runtime randomness anywhere in this crate's fault injection (D56, and the repository
rule that unit tests are reproducible). Every rule is declared up front, insertion order is
stable, the schedule is a counter, and a gate is opened by the test itself -- control the test
already has. A fault-injecting test that fails is reporting a real defect, never a sampled one.

## Standing rule -- a bespoke `Vfs` decorator means this crate is missing something

**If you need a storage fault this surface cannot express, file it against `acadia`. Do not
write your own `Vfs` decorator in your crate.**

This is not a style preference; it is what [D107](../../DESIGN-NOTES.md) was written to settle.
Three consumer crates each independently hand-wrote a decorator -- `StagedRenameFaultVfs` in
`canopy`, `EngineStateWriteFaultVfs` and `ReadHookVfs` in `canopy-core` -- and every one of them spent
most of its length delegating thirteen `Vfs` methods verbatim in order to change the behaviour of
exactly one. Each also re-invented a path-matching rule, and each matched paths in a slightly
different way. That cost was paid three times for one missing capability, and none of the three
could be reused by the next consumer that needed a variation.

When you hit the wall, the response is a request against this crate, because a gap here is a gap
for *every* consumer:

- the fault you need is not in the vocabulary -> extend [`Fault`](src/fault.rs);
- you cannot name the paths you mean -> extend `PathMatcher` (and reach for
  `PathMatcher::predicate` in the meantime -- it is a first-class part of the surface, not a
  workaround);
- you need the fault to start at a moment you choose -> that is `FaultGate`;
- you need to watch, or to act inside, an operation -> that is `Fault::Observe`.

**The one legitimate exception** is a fault that is genuinely about *your own crate's types*
rather than about storage -- a failing `Engine`, a codec that returns malformed bytes, a clock
that jumps. Those are not `Vfs` concerns and no `Vfs`-level facility can reach them; they belong
in the consumer, beside the type they are faulting. The test is simple: if the thing you want to
break is a *filesystem operation*, it belongs here.

A decorator that reappears in a consumer crate should be read as a bug report against this file.

## Review fixes, 2026-11-09 (`RF-8`..`RF-10`)

> **SUPERSEDED in part, 2026-11-09, by "Third review fixes, 2026-11-09" below.** How a
> renamed directory carries its durable contents is restated there: the contents are captured
> at the rename, so they no longer depend on the order the old and new parents are synced in.

- **`flock` contention is `EWOULDBLOCK` and nothing else.** `EACCES` comes from the open
  that precedes the lock, so it is reported as `PermissionDenied` rather than `Locked`: a
  misconfigured root must not look like a busy one.
- **Windows directory fsync is attempted, not skipped.** The directory is opened with
  `FILE_FLAG_BACKUP_SEMANTICS` and write access and flushed with `FlushFileBuffers`, and any
  failure is reported. Win32 does not document what that flush persists, so this is the
  strongest request available rather than a proven guarantee. `OsFs` supports Unix and
  Windows only; any other target fails to compile.
- **`MemFs` models a directory's own name separately from its entries.** Fsyncing a
  directory makes its entries durable, not its name in its parent; a directory whose name
  was never synced is lost on `simulate_crash` with everything in it. The implicit root
  cannot be fsynced, so its direct entries count as named. A renamed directory carries its
  durable contents to its new name when that name is synced, as its inode would.

## Third review fixes, 2026-11-09 (`RV3-4`)

- **A renamed directory's durable contents are captured at the rename.** `MemFs` tracks
  directories by path, not identity, so the rename records the durable subtree the directory
  is leaving behind (or the one it already carried, if it was renamed before). That snapshot
  becomes durable at the new name when the new parent is synced -- replacing an empty
  directory the rename overwrote -- or earlier, as the base a sync at or beneath the new name
  builds on. Syncing the old parent first therefore cannot lose it.
- **A known leniency.** If something at or beneath the new name is synced before the new
  parent is, the carried contents are placed in the durable tree at that moment; if the
  rename replaced an existing empty directory, a crash before the parent sync then shows the
  carried contents at the destination rather than the empty directory. A test that depends on
  that ordering cannot rely on `MemFs` to catch it.
- **What a renamed directory carries is dropped only by an operation that succeeded.** A
  refused `remove_dir` or `remove_dir_all` keeps it, and a sync whose destination cannot be
  created leaves it carried rather than losing it.

## Fourth review fixes, 2026-11-09 (`RV4-6`)

- **A path whose directory was replaced starts from empty.** `MemFs` records a path as
  replaced when its directory is removed or renamed away (and a rename's destination too);
  the next sync naming it begins the durable directory there from empty instead of inheriting
  its predecessor's entries, before any carried snapshot is placed over it.
- **A directory moved out of a renamed one takes its contents with it.** Its durable contents
  exist only inside the renamed ancestor's carried snapshot, so the rename extracts them from
  there, and the ancestor's snapshot no longer holds them.

## Fifth review fixes, 2026-11-09 (`RV5-5`, `RV5-7`)

> **SUPERSEDED in part, 2026-11-09, by "Sixth review fixes" (`RV6-5`, `RV6-6`).** Reclamation
> no longer runs as a whole-filesystem walk after a sync that drops an entry; data is released
> by reference count, and the trigger described in the second bullet missed entries discarded
> by `refresh` and `materialize`.

- **A carried snapshot does not reach past a replacement.** Extracting a directory from a
  renamed ancestor's snapshot is refused if any path between the ancestor and the directory
  was replaced since: the directory there now is new, and the snapshot's copy is its
  predecessor's.
- **Unreachable file data is reclaimed.** After a directory sync that drops or replaces an
  entry, and after `simulate_crash`, live bytes not named by the live tree and durable bytes
  named by neither tree nor any carried snapshot are dropped, so memory follows the current
  filesystem rather than its history.

## Sixth review fixes, 2026-11-09 (`RV6-4`..`RV6-6`)

- **A rename destination's history goes with it.** Renaming a directory over an (empty)
  directory discards every carried snapshot at or beneath the destination, and every
  `replaced` mark strictly beneath it, before the source's own state is installed -- even when
  the source carries nothing. Otherwise a sync would materialize the replaced directory's
  snapshot and a crash would resurrect its files inside the replacement.
- **File data is reference counted.** `Inner::refs` counts, per inode, its entries in the live
  tree, the durable tree and the carried snapshots. Every operation that discards an entry
  releases it -- `reconcile_children`, `refresh` and `materialize` during a sync, a rename over
  a file, removal, and dropping carried snapshots -- and a file's durable bytes go with its
  last reference. Moving a snapshot between `carried` and the durable tree changes no count.
  Reclamation therefore costs an operation only what it discarded; only `simulate_crash`,
  which replaces the whole live tree, recounts from scratch. Live bytes are dropped whenever
  the live tree loses a file, including a file displaced by a rename.
- **The counts are audited in tests.** `MemFs::audit_refs` (test-only) recounts from scratch
  and asserts the incremental counts and both data maps agree with it; a test-only walk
  counter bounds the cost of an overwrite.

## Seventh review fixes, 2026-11-10 (`RV7-3`, `RV7-4`)

> **SUPERSEDED in part, 2026-11-10, by "Eighth review fixes" (`RV8-2`..`RV8-4`).** The second
> bullet anchors the carried snapshot at the first durable file on the path, resets a moved
> directory's snapshot, and records no unnamed directories inside it. Read the eighth-round
> section before acting on it.

- **Live bytes go with the last live name.** A crash can leave two names for one inode (a
  rename made durable in its destination directory but not its source), so `Inner::live_refs`
  counts live-tree entries per inode and a removal, a rename over a file, or a recursive
  removal drops the live bytes only when the count reaches zero.
- **A directory standing where the durable tree has a file syncs into a carried snapshot.**
  Until its parent's sync names it, the new directory's contents are held in `carried` under
  the path where the file is, exactly like a renamed directory's; the parent's sync
  materializes them over the file, and a crash before then drops them and keeps the file.
  Directories synced beneath it, replaced paths, and carried snapshots below it join that
  snapshot.

## Eighth review fixes, 2026-11-10 (`RV8-2`..`RV8-5`)

- **A carried sync anchors at the deepest file on the path.** `carried_anchor` walks the
  durable tree and, below the first durable file, the snapshots carried over it; the snapshot
  a sync updates is the one at the deepest prefix holding a file in either. A directory that
  replaced a file inside a carried snapshot therefore gets its own snapshot, and is placed over
  that file when its parent syncs, instead of failing on it.
- **A carried sync never drops state.** `sync_into_carried` puts the anchor's snapshot back
  whatever the outcome, and a nested snapshot leaves `carried` only once its place in the
  anchor's snapshot exists.
- **The anchor's replacement mark does not reset its snapshot.** A rename sets the snapshot
  and the mark together and a removal drops both, so a snapshot carried at the anchor is
  always newer than the mark; the mark is cleared without emptying it.
- **Directories created inside a snapshot are unnamed until their parent syncs**, exactly as
  in the durable tree, so a crash drops a directory whose own parent was never synced even
  when an ancestor's sync materialized the snapshot holding it.
- **`OsFs::read_at` refuses a directory before looking at the range.** Unix opens a directory
  for reading, so an empty or past-the-end read would otherwise succeed on one; it now reports
  `IsADirectory`, as `MemFs` does.

## MemFs directory sync costs what changed, 2026-11-10

A `MemFs` directory sync reconciles only the entries whose live state may differ from the
durable one, so its cost follows what changed since that directory's last sync, not the
directory's size. Reconciling every entry made a store that syncs a partition root after
creating each shard quadratic in the number of shards: the content-store scale tests spent
most of their time inside `MemFs::fsync`, not in the store.

- **The invariant.** For every live directory, its durable directory differs from it only in
  the names recorded for it in `Inner::dirty`, or the record is "all" and nothing is assumed.
  A directory with no record matches its durable directory exactly. Recording a name that did
  not change costs a no-op reconcile; missing one loses that change on a crash. So the rule is
  to record conservatively: every operation that changes a live directory's entries (create,
  write or append of a new file, remove, rename on both sides) records the name in its parent.
- **Wholesale replacement records "all".** A durable directory that is created, reset by a
  replacement mark, or replaced by a materialized carried snapshot no longer differs from its
  live directory by any recorded set, so it and (for a snapshot) every directory beneath it are
  marked "all". A directory a sync gives a new, empty durable copy is marked "all" too.
- **Records are dropped only by the sync that satisfies them**, after it has succeeded, and by
  a crash, after which live equals durable. Records under a removed or renamed directory are
  discarded; both paths of a directory rename are replacement-marked, so their next sync
  starts from empty and reconciles every entry regardless.
- **Carried syncs do not consume records.** A sync into a carried snapshot leaves the durable
  tree untouched, and the snapshot's materialization marks its subtree "all".
- **The full reconcile stays as the oracle.** `MemFs::with_full_directory_sync` (test only)
  ignores the records and reconciles every entry. A seeded differential test replays identical
  random operation sequences, crashes included, against both and requires identical results,
  identical crash survivors, and consistent reference counts; a test-only counter of examined
  entries pins the cost bound itself. Removing the record a rename makes in its destination
  parent fails the differential test, so it detects under-recording rather than only
  agreeing with itself.

## MemFs fork, 2026-11-10

`MemFs::fork` returns an independent filesystem holding a copy of the original's whole state:
both trees, both data maps, and the bookkeeping that decides what a crash keeps (unnamed
directories, carried snapshots, replacement marks, change records, reference counts). A fork
therefore survives a crash exactly as its original would have at the moment of forking, and
neither ever observes the other's later changes. Locks are deliberately not copied: a lock
belongs to its holder rather than to the stored tree, the same reason a crash does not
release one. Its purpose is fixtures: a store populated once can be handed to every test as a
private copy instead of being rebuilt per test.

## Locks on a file or a range of one, taken without waiting, 2026-11-12

> **SUPERSEDED in part, 2026-11-17, by "Fourth review fixes: markers, plan names, recovery order,
> foreign roots and MemFs locks" below.** A `MemFs` lock belongs to the file it was taken on, not
> to the name: it follows the file through a rename, and a lock on a file that a crash erased does
> not block a file made later at that name. The rest of this decision stands.

`Vfs::try_lock` takes a shared or exclusive lock on a whole file or a byte range of one, fails at
once with `Locked` on a conflict, and is released when its guard drops or its process dies. It
is the primitive canopy's file-based coordination builds its admission gate, attachment tiers
and registry on[^ac-1] ([canopy's DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Several processes
may open one store").

- **Every guard is its own holder.** `OsFs` opens a handle per lock and locks through it:
  `LockFileEx` on Windows, open-file-description locks on Linux. Both conflict between two
  handles of one process exactly as between two processes, which is what lets a test contend
  for a lock on one thread, and what lets one process hold several roles. Process-associated
  `fcntl` locks are never used: any close of the file by the process releases all of them.
  Another Unix gets `flock` for a whole file and `Unsupported` for a range, since it has no
  per-description range lock.
- **A range is bounded by what Linux can name.** It is at least one byte and ends at or before
  `LockSpan::MAX_END` (`i64::MAX`), so one range means the same on every platform. A whole-file
  lock is `[0, u64::MAX)` and overlaps every range.
- **A locked span holds no data.** Windows locks are mandatory: an exclusive lock refuses every
  other handle's read and every lock refuses another handle's write, and every seam operation
  opens its own handle, so the holder is refused too. Linux locks are advisory. The contract
  therefore keeps data out of every locked span, and the primitive allows a range anywhere,
  including past the end of the file, so a lock can sit beside the data it guards. `OsFs`
  reports a refused read or write as `Locked`.
- **`MemFs` refuses as Windows does**, so a caller that reads or writes a locked span fails on
  every platform's test run, not only on Windows. A read is checked against exclusive holds over
  the bytes it returns; a write, an append or a truncation against every hold over the bytes it
  changes.
- **A file is locked through `try_lock` or `lock_exclusive`, never both.** On Windows
  `lock_exclusive`'s share mode refuses every other handle, on Linux its `flock` is independent
  of open-file-description locks, so the two would exclude each other on one platform and not
  the other. `MemFs` treats `lock_exclusive` as an exclusive whole-file hold, the stricter
  reading, and creates its file without truncating it, as `OsFs` does.
- **A Unix lock is released explicitly before its handle closes.** A child forked by another
  thread holds a duplicate of the description until it execs, and `flock` and
  open-file-description locks last as long as the description, so a lock released only by the
  close outlives its guard for that window. A Linux run of the cross-process root-lock suite
  once saw a holder refused a root whose only other holder had already dropped its guard, which
  this window explains. On Windows, `UnlockFileEx` before the close makes the release prompt,
  where a closed handle's locks go only when the system gets to them.
- **A simulated process dies by abandonment.** A `MemFs` value is one process's view:
  `MemFs::another_process` gives a second over the same tree and lock table, and
  `MemFs::abandon` releases every lock that process holds at once, without its guards running,
  as the OS does for a dead process. Guards still in hand stay inert, since every hold has an
  id that is never reused. A crash (`simulate_crash`) loses storage and keeps locks; abandonment
  loses locks and keeps storage. Real deaths are checked by `canopy-lock-holder`'s
  `vfs-lock-holder` fixture, killed outright on both platforms.

## Publishing a file only where nothing is, 2026-11-12

> **SUPERSEDED in part, 2026-11-17, by "A publication is a move on Windows".** On Windows `OsFs`
> no longer hard-links the staged file: it moves it into place without replacing, and the sibling
> is removed only when the move fails. The Unix bullet below still holds.

`Vfs::publish_new` creates a file holding exactly the given bytes if nothing is at the path, and
fails with `AlreadyExists` otherwise. The file is never visible incomplete, and a power loss
leaves it complete or absent; its name is durable once the parent is synced. canopy's creation
reservation needs exactly this ([canopy's DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Creating
a store root, modelled in `Creation.tla`")[^ac-1].

- **`OsFs` stages, syncs and hard-links.** The bytes are written to a fresh sibling named
  `<name>.publishing.<pid>-<n>`, synced, and linked to the path; the sibling is removed whether
  the link succeeds or not. A link fails where anything has the name, which is the exclusivity
  a replacing rename lacks, and it names an inode whose bytes are already durable, which a file
  created at the path and then written cannot promise. A crash can leave a staging sibling; it
  means nothing and may be removed. The filesystem must support hard links, which NTFS, ReFS
  and the Linux filesystems a store root lives on do.
- **`MemFs` publishes in one step** to the state `OsFs` reaches before its sibling is removed:
  the bytes durable, the name not, so a crash before the parent's sync loses the file and a
  crash after keeps all of it.

[^ac-1]: `AC-1` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ac-1) [completed 2026-11-12; `f63b76f`]

## A signal is a level, one per platform behind a sealed trait, 2026-11-12

`Signal` is the one platform primitive of canopy's ring ([canopy's
DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "The ring: a portable protocol over one platform
primitive, behind a contract spec")[^ac-3]: raise, lower and a bounded wait, all on a level.

- **A level, so redundant calls are free of meaning.** Raising a raised signal and lowering a
  lowered one change nothing and report nothing, and a wait returns at once on a raised signal
  without lowering it. The doorbell protocol above relies on exactly that and on nothing else,
  so it is argued once for every implementation.
- **One implementation per platform.** `EventSignal` is a Windows manual-reset event, held as
  a `win-sync-sys` `Event`, the safe event of
  [MikeGrier/windows-threadpool-sys](https://github.com/MikeGrier/windows-threadpool-sys), so
  acadia makes no event call of its own. An
  auto-reset event would be an edge: it lowers itself for the one waiter it releases, so a
  second waiter, or a second look, misses the raise. `EventFdSignal` is a Linux `eventfd`,
  non-blocking and not in semaphore mode: readable while its counter is not zero, raised by a
  write of one, lowered by a read, which takes the whole counter, so two raises and one lower
  leave it lowered. A counter too full to take another write is raised already, and an empty
  one has nothing to read, so both report nothing. `CondvarSignal`, a mutex and condition
  variable, is `PlatformSignal` everywhere else and exists on every target, for tests and for
  a checker of the Rust memory model, which cannot run a kernel object.
- **Sealed, and fallible.** The trait is sealed so the protocol is argued against these three
  and no other. Creating, raising, lowering and waiting return `io::Result`, since the kernel
  calls can fail, though a valid object's calls do not in practice.
- **A wait never reports `false` early.** The platform waits take whole milliseconds, so the
  remaining time is rounded up and the wait repeated until the deadline has passed, which also
  covers a timeout too long for the platform to name and an interrupted `poll`.
- **The native object is the standard library's trait.** `EventSignal` implements `AsHandle`
  and `EventFdSignal` implements `AsFd`, so a caller waits on one with the platform's own wait
  and portable code never names a platform type. `EventSignal::event` also lends the `Event`
  itself, which a `windows-threadpool-sys` thread-pool wait takes without `unsafe`, because an
  `Event` is known to be an event and never a mutex. Tests wait with `win-sync-sys`'s
  `wait_for_single_object` and with `poll` and see the level `Signal::wait` sees. A
  revert-probe that made the event auto-reset
  failed the tests that a raise stays raised through waits, wakes every waiter, and is seen by
  the platform's wait without being consumed.

[^ac-3]: `AC-3` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ac-3) [completed 2026-11-12; `f63b76f`]

## `EventSignal` lends a handle that can only wait, 2026-11-13

`EventSignal::wait_only` returns a new handle to the event carrying `SYNCHRONIZE` access alone --
`win-sync-sys`'s `WaitOnlyEvent`, made by `Event::duplicate_wait_only` -- so the kernel refuses a
set or a reset through it. That access is not what protects the level: code holding a handle to
the event can reach the event whatever that handle's access, and `WaitOnlyEvent` lends its handle
through `AsHandle`. The protection is the line between safe and `unsafe`, so a crate that must keep
a signal's level its own, as canopy's doorbell does, lends the wait-only handle only through an
`unsafe` accessor whose contract forbids changing the level ([canopy's
DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "The doorbell's signal is sealed"). `windows-threadpool-sys`
takes a `WaitOnlyEvent` without further `unsafe` from 0.4.1, for the reason it takes an `Event`: a
`WaitOnlyEvent` can only be made from an `Event`, so it is known to be an event and never a mutex.
acadia re-exports the type on Windows, so a crate that lends it on names it without depending on
`win-sync-sys`. The handle is its own, so it keeps the event open after the signal is dropped, and
a wait through it sees the level `Signal::wait` sees.

## A lock may be waited for, without a bound, 2026-11-12

`Vfs::lock` takes the same locks as `Vfs::try_lock` and waits while another holder's lock
conflicts, for canopy's writer exclusion, which waits rather than refusing ([canopy's
DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Writer exclusion and its ownership record, modelled in
`Coordination.tla`")[^ac-2].

- **Both modes and every span.** The writer exclusion needs an exclusive lock, but the waiting
  form of `try_lock` costs nothing more in any mode, so it takes the same arguments and the two
  never differ in what they lock. A waiter for bytes nobody holds is granted at once.
- **Woken by any release that lets it in, in no order.** `OsFs` waits in the kernel --
  `LockFileEx` without `LOCKFILE_FAIL_IMMEDIATELY` on Windows, `F_OFD_SETLKW` on Linux, `flock`
  without `LOCK_NB` elsewhere -- so a dropped guard or a dead holder wakes it, and an interrupted
  wait on Unix resumes. `MemFs` waits on a condition variable that every release and every
  abandonment wakes. Neither promises an order among waiters, which the coordination contract
  does not ask for ([canopy's DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "The contract promises
  no order among waiters at any point of conflict").
- **No bound and no cancellation.** A kernel lock wait can be bounded or cancelled only with
  machinery the platforms do not share (an overlapped handle and `CancelIoEx` on Windows, a
  signal on Linux), and the writer exclusion's contract waits without a bound. A thread holding a
  conflicting guard waits for ever, as two processes would. A bounded or cancellable wait is not
  scheduled; it is added when a consumer's contract needs one.
- **No backend has an inherent method named `lock`.** An inherent method shadows a trait method
  of the same name at every call site, so one would stop a caller holding that backend from
  calling `Vfs::lock` on it by method syntax; the backends' own state locks are `MemFs::tree` and
  `FaultyFs::rule_state`.
- **Tested across processes.** `vfs-lock-holder` takes the waiting form on request; the suite has
  a waiter woken by a killed holder, for a whole file and for a range, and a waiting process let in
  when this one lets go, on Windows and on Linux under WSL. Removing the wake from `MemFs`'s
  abandonment fails the test of a waiter let in by a simulated death.

[^ac-2]: `AC-2` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ac-2) [completed 2026-11-12; `f63b76f`]

## The platform's own wait on a borrowed signal, 2026-11-12

`wait_native` waits on a borrowed event (`BorrowedHandle`, Windows, through `win-sync-sys`'s
`wait_for_single_object`) or descriptor (`BorrowedFd`, Linux, through `poll`) with the platform's
own wait, for at most a bound, and reports whether it was raised; it lowers nothing. It is what
`EventSignal::wait` and `EventFdSignal::wait` are, taken out so that a
caller holding only the native object a type lends through `AsHandle` or `AsFd` -- canopy's
doorbell, over the platform's signal -- can wait on it without a system call or `unsafe` of its own
([canopy's DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "The doorbell in code,
`canopy::ring::Doorbell`")[^rg-2]. It keeps the signal's rule that a wait never reports `false`
before its bound has passed.

[^rg-2]: `RG-2` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#rg-2)
    [completed 2026-11-12; `f63b76f`]

## A signal is watched on a thread pool, by acadia, on its own object, 2026-11-13

`Signal::watch` runs a callback on a thread pool each time the signal is found raised, until the
`Watch` it returns is dropped[^ac-4]. It is how a crate that keeps a signal sealed -- canopy's
doorbell ([canopy's DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "The doorbell's signal is
sealed") -- waits without a thread of its own and without lending the platform object: acadia
registers the object, and the callback is handed nothing.

- **A level, as the signal is.** A callback that returns with the signal still raised is run
  again at once. So a consumer lowers the signal first and then looks for the work, and a raise
  that lands after the look runs it again; a raise is never lost between two runs.
- **One run at a time, per watch.** Each platform re-arms only after the callback returns: the
  Windows thread-pool wait from inside its callback, after the caller's code; the Linux `epoll`
  registration is one-shot and modified back in after the run; the condition-variable watch is a
  single thread. The callback never receives the pool's activation, so it cannot re-arm early and
  overlap itself.
- **Where the threads come from.** On Windows, the process's default thread pool, through a
  `windows-threadpool-sys` `ThreadpoolWait` on a wait-only duplicate of the event. On Linux, a
  process-wide service acadia starts on the first watch: one `epoll` set and up to four threads,
  each registration holding a duplicate of the `eventfd` under a token that is never reused. For
  `CondvarSignal`, a thread per watch, since a condition variable has no pool to join; it shares
  the signal's state through an `Arc`.
- **Dropping waits for a run in progress, except its own.** A drop marks the watch stopped, so it
  is not run again, and then waits for a running callback to return. Dropped from inside its own
  callback it cannot wait for itself: the condition-variable watch skips the join, the `epoll`
  watch waits only for another thread's run, and the Windows wait is closed on a thread started
  for it once the run returns, since closing it from its own callback would wait for itself.
- **A panicking callback aborts the process.** The Windows pool's callback is entered through an
  `extern "system"` trampoline, where a panic aborts; every platform does the same, so a callback
  cannot behave differently by platform. A failed `epoll` wait or re-arm aborts too: acadia owns
  both descriptors, and the failure would otherwise leave a watch silently stopped, which no caller
  could detect.

Revert-probes: not re-arming the Windows wait failed the tests that a callback leaving the signal
raised runs again and that runs never overlap, and a condition-variable watch that did not join on
drop failed the test that a drop waits for a running callback.

[^ac-4]: `AC-4` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ac-4) [completed 2026-11-13; `191fbf2`]

## A signal's operations are ordered as acquire-release operations, 2026-11-13

`Signal` promises that `raise`, `lower` and `wait` are each ordered with the calling thread's own
memory accesses as an acquire-release read-modify-write of the signal's level would be. A protocol
over a signal -- canopy's doorbell is the one there is -- may then treat the level as one more
atomic location, and order it with its own atomics by acquire and release alone ([canopy's
DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "The doorbell's orderings, checked by loom")[^rg-2-2].
The contract is the engineer's: it is the obvious one, and it follows from what each platform
documents of the operations behind it.

- **Windows, `EventSignal`.** Microsoft's documentation of the Win32 synchronization functions
  ([Synchronization and Multiprocessor
  Issues](https://learn.microsoft.com/en-us/windows/win32/sync/synchronization-and-multiprocessor-issues),
  "Memory Ordering") lists the functions that signal synchronization objects, and the wait
  functions, among those that use the barriers needed to order memory around them: `SetEvent`,
  behind `raise`, and `WaitForSingleObject`, behind `wait`. `ResetEvent`, behind `lower`, is not
  named. The doorbell needs no barrier of `lower`'s own, only that its effect on the event is done
  before the call returns, which `ResetEvent` states; the release of the caller's next atomic then
  orders it, since the release orders everything the thread did before it, the call included.
- **Linux, `EventFdSignal`.** A write to or read from an `eventfd` changes its counter under the
  `eventfd`'s own lock, before the system call returns, and a `poll` reads it under the same lock;
  the kernel's lock release and acquire order those accesses with one another, and the caller's
  atomics before and after the call order them with the caller's program as above.
- **`CondvarSignal` and `LoomSignal`.** The level is a `bool` under a mutex, so every operation is
  a lock acquire and release, inside the Rust memory model.

What this does not cover is the thread's program order across the call itself, which the contract
takes as given: a system call is part of the calling thread's execution, and a thread that is moved
between processors mid-call is moved through the scheduler's own barriers. A probe of the platform
would be a data point for an ambiguity here, not a proof of the contract, so none is scheduled.

[^rg-2-2]: `RG-2.2` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#rg-22)
    [completed 2026-11-13; `d01bc42`]

## The coordination interface is acadia's, as a layer of its own above `Vfs`, 2026-11-13

The coordination interface of canopy's several-process design -- the contract for writer exclusion,
the fence and generation, the admission gate, the registry of attached handles, the modification
count and its notification, and creation exclusion ([canopy's
DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Several processes may open one store", "The
coordination contract") -- lives in this crate, with its implementations and its conformance
suite[^m6-1].

- **A layer, not part of the seam.** The interface is the module `acadia::coord` and the modules
  beneath it, apart from `Vfs`, its backends and `Signal`. It depends on them; none of them depends
  on it, which review holds rather than the compiler. A consumer of the seam
  alone -- a storage engine's data path -- names nothing of coordination, which is the relation the
  contract has to `Vfs`: it decides who may act on a root and when, and the data operations stay
  `Vfs` calls.
- **It names no type of canopy's.** This crate is a dependency of canopy and cannot name what
  canopy defines, so the interface speaks in its own types -- an implementation identity, a nonce, a
  generation, a count -- and carries what only canopy can interpret as bytes the caller supplies.
  The creation reservation records every creation input (the genesis configuration, the agent, the
  schemas) as one such byte string, and an interrupted creation is resumed only when a resumer's
  bytes are identical. Canopy encodes them canonically, so identity of inputs is identity of bytes.
- **What canopy decides stays canopy's.** Admission by schema is canopy's check, made after the
  interface grants the shared tiers, as the contract orders it; the interface provides the tiers
  and the gate and knows no schema. A registry entry's contents -- the agent, the schemas spoken,
  how to find the process -- are a payload canopy supplies and a listing returns; the interface
  owns the entry's slot, its token and its liveness. A notification runs a callback the caller
  registers, so canopy rings its own doorbell and lends the interface nothing of it. The agent's
  version stamp and canopy's other durable records are written through `Vfs`, not through the
  interface, which provides only the generation that tells a handle its caches of them are stale.
- **Every implementation lives here.** The file-based one is built from `Vfs` primitives and so runs
  over `OsFs`, `MemFs` and `FaultyFs`; the shared-memory one is built from OS facilities `Vfs` does
  not model, as `Signal` already is, and carries the `unsafe` that needs, kept in modules of its own.
- **The conformance suite lives here,** since it is what states the contract, and every
  implementation, including the second one tests use, runs it. Its cross-process helpers are this
  crate's own test binary run again, so they depend on nothing but this crate (see "The
  coordination interface, defined").
- **The crate's description and documentation change with it:** the seam is one layer of the crate
  and coordination is the other.

[^m6-1]: `M6-1` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m6-1)
    [completed 2026-11-13; `d7daade`]

## The coordination interface, defined, 2026-11-13

`acadia::coord` states every part of the contract as a trait of its own, with each operation's
contract on it[^m6-1]:

- **One trait per part, joined into `Coordination` when implemented.** `Creation`,
  `RestartRecognition`, `ModificationCount`, `WriterExclusion`, `Admission` with `Attachment` and
  `Upgrade`, `Registry`, and `CountNotification`. `Coordination` itself names a participant's
  implementation and abandons it; a part becomes a supertrait of `Coordination` in the change that
  gives every implementation that part, so the interface a caller holds as `dyn Coordination` never
  carries an operation that is not built, and no implementation carries a stub.
- **What only the holder may do is reachable only through its hold.** Raising the count,
  completing recovery and releasing exclusion are methods of the `ExclusionHold` that acquiring
  returns; converting for an upgrade is a method of the `Attachment` it consumes; a registration
  lives inside the attachment or upgrade it describes, so dropping either removes the entry before
  releasing the tiers, in the order the registry's contract requires, without the caller arranging
  it. Holds are `'static` boxed trait objects, so a participant can be held as `dyn Coordination`.
- **Every acquisition says whether it may wait.** `Waiting::Wait` waits as the contract states,
  without bound or order; `Waiting::Never` refuses with `WouldWait` and holds nothing. Work that must
  not hold a thread tries, and is woken to try again by a notification or its deadline. Giving up a
  wait is not offered, because `Vfs::lock` cannot be cancelled; the non-waiting form is what an
  engine that holds no thread uses instead.
- **A participant is the unit of death.** Each participant keeps every lock it takes in a table of
  its own. `Coordination::abandon` empties the table without any bookkeeping -- a record stays, an
  entry is left uncleared -- which is what the operating system does for a dead process, and makes
  every later operation of the participant fail with `Abandoned`. Two participants in one process
  are as separate as two processes, since every `Vfs` lock guard is its own holder.
- **The file-based layout.** Beneath the root, the reservation (`.coord-reservation`), the
  creation exclusion's lock file, and the state directory `.coord`; in it, data files for the boot
  identity, the count, the ownership record, the fence, the generation, one file per registry
  entry and the registry's overflow record, and lock files for exclusion, the gate, the tiers (a
  byte each) and the registry (a claim byte and a liveness byte per entry). Every name beneath the
  root begins with `RESERVED_PREFIX`, `.coord`, in every implementation, so a creator's reading of
  the root knows them for coordination's own; no artifact of creation's may begin with it. A lock
  file holds no data and a data file is never locked, because what a read or write does to a byte
  another holder has locked is the platform's (`Vfs::try_lock`).
- **The second implementation** is the file-based one with another identity and another creation
  lock file. Two implementations need only those two differences to exercise the cases between
  them: their creation exclusions do not exclude each other, and the reservation is what decides.
- **Cross-process helpers are the test binary itself.** A cross-process case runs the same test
  binary again on one ignored test, telling it through its environment what to hold, and reads
  the line it writes once it holds it. The helpers live beside the cases and depend on nothing but
  this crate, which `canopy-lock-holder` could not offer, since it depends on canopy. The cases run
  on Windows and, under WSL, on Linux.

## Creating a root through the file-based implementation, 2026-11-13

> **SUPERSEDED in part, 2026-11-17, by "Fourth review fixes: markers, plan names, recovery order,
> foreign roots and MemFs locks" below.** The maker does not make the marker: it returns the
> marker's bytes, and coordination publishes them with `Vfs::publish_new` and syncs the root. The
> rest of this decision stands.

The file-based implementation builds `Creation` as canopy's creation design states it ([canopy's
DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Creating a store root, modelled in
`Creation.tla`")[^m6-2], with these choices of its own:

- **The reading under the exclusion decides.** A creator reads the root, waits for its creation
  exclusion, and reads it again; what the second reading finds is what it does, and a resumer whose
  reservation's nonce differs between the two readings is refused. The first reading only spares a
  root that is plainly not to be created in from taking the lock.
- **An artifact with no reservation occupies the root.** The reservation is durable before any
  artifact exists, so no creation leaves an artifact without one, and something bearing an
  artifact's name with no reservation is someone else's.
- **The reservation is published with `Vfs::publish_new`**, which leaves it complete or absent
  across a power loss, and its directory entry is made durable before the first artifact. A
  publication that finds a reservation already there refuses the creation: only another
  implementation, whose creation exclusion does not exclude this one's, can have put it there.
- **The reservation is kept** after creation, as the record of which implementation coordinates the
  root. Its encoding -- magic, version, identity, nonce, inputs -- is versioned by its own byte, and
  anything that is not exactly what the encoder writes reads as malformed.
- **An opener takes the creation exclusion shared**, as the contract states, so openers do not
  exclude each other and an open that meets a creation in progress waits for it. Canopy's notes
  record that the model finds this wait unnecessary for safety; it is kept because the contract
  states it, at the cost of an open waiting for a creation that is slow.
- **An open syncs the marker and the root before relying on it** (`UNSYNCED-MARKER`): the maker
  syncs each artifact, the implementation syncs the root after each, and an open repeats both for
  the marker. A crash after an open has done so keeps the marker; the conformance case that omits
  the open shows the same crash losing it.

Revert-probes: reading the root once instead of again under the exclusion, publishing the
reservation with a plain write, and opening without syncing the marker each failed the cases built
for them -- the resumer that waited, the intruder that appeared during the wait, the creator whose
reservation was published second, the simultaneous creators, and the marker made durable by an
open.

[^m6-2]: `M6-2` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m6-2)
    [completed 2026-11-13; `d7daade`]

## A host restart is recognised from the boot identity the filesystem reports, 2026-11-13

> **SUPERSEDED in part, 2026-11-18, by "Fifth review fixes: recovery's commit point, restart
> repair, foreign roots and the marker under abandonment" below.** A completed recovery vouches
> for the state through the count record alone; the file implementation never records the boot
> vouched. Read the banner below as well.

> **SUPERSEDED in part, 2026-11-17, by "Fourth review fixes: markers, plan names, recovery order,
> foreign roots and MemFs locks" below.** The file implementation's state is also trusted when
> the count record is vouched and from this boot, whatever the boot record says: the vouched count
> is recovery's commit point. The rest of this decision stands.

The coordination state is trusted only within the boot it was made in, and only once a completed
recovery has vouched for it (canopy's DESIGN-NOTES.md, "Several processes may open one store",
Survival)[^m6-3].

- **The boot identity is a `Vfs` operation.** `Vfs::boot_identity` names the current boot of the
  host the filesystem lives on, so a restart follows the backend: `OsFs` reports the operating
  system's, and `MemFs` counts its simulated crashes, since `simulate_crash` is its power loss and
  so its restart. The coordination is built from `Vfs` primitives, and this is the one it needed
  that the seam did not have. `FaultyFs` forwards it and can fail it (`FaultOp::BootIdentity`,
  matched against the empty path). The default, and `OsFs` on a platform with no identity, is
  `Unsupported`, which the coordination reads as state it cannot vouch for, never as trusted.
- **Linux: `/proc/sys/kernel/random/boot_id`**, which the kernel generates at each boot.
  **Windows: the `BootId` field of `KUSER_SHARED_DATA`**, the boot sequence number the kernel
  advances at each boot and shares, read-only, with every user-mode process at a fixed address;
  `windows-sys` gives the layout, and the read is one volatile load. Both stay the same across a
  sleep or hibernation, which preserves the state, and change across a boot, which loses it. On
  Windows the number is a counter kept in the system's registry, so restoring an older copy of the
  registry could repeat one: that is the one way the identity could fail to change across a boot,
  and it is accepted.
- **The boot record**, in the state directory, holds the boot identity the state was made in and
  whether a recovery has vouched for it since. State is trusted only when the record names this
  boot and is vouched; otherwise it is untrusted because the host restarted, because the state was
  made and no recovery has completed on it (or was never made), or because the filesystem cannot
  identify a boot or the record cannot be read.
- **The record is replaced whole and never synced.** It is written to a fresh staged file and
  renamed over itself, so a reader sees one record or the other. A restart is the only thing that
  could lose an unsynced write, and a restart makes the record untrusted whatever it says.
- **Reading the standing changes nothing.** The record is written only under writer exclusion --
  remade untrusted by the acquisition that finds it from another boot, and vouched by a completed
  recovery -- so the two writers are serialized and a slow remaker cannot undo a vouching. A
  record that belongs to a boot is never deleted: a later record carries its boot identity instead,
  so nothing written in this boot needs clearing after the next.

A revert-probe that ignored the boot identity in the record failed the case of a vouched record
surviving a crash.

[^m6-3]: `M6-3` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m6-3)
    [completed 2026-11-13; `d7daade`]

## The modification count is one record, carrying its boot and its standing, 2026-11-13

The file-based implementation publishes the modification count as canopy's design states it (canopy's
DESIGN-NOTES.md, "The modification count, the generation and reads, modelled in
`Coordination.tla`")[^m6-4], with these choices of its own:

- **The record carries what a sample needs, so a sample is one read.** It holds the value, the boot
  it was published in and whether a recovery has vouched for the state. A sample reads it once and
  reports the value, made odd unless the record is vouched and from this boot; a missing or
  unreadable record samples odd. Reading the boot record beside it would assemble a sample from two
  separately published parts, which the contract forbids.
- **Published whole, never synced.** A fresh file staged beside the record is renamed over it, so a
  sampler sees the previous record or the new one, and a publisher killed before its rename leaves
  the previous one. A staged file a failed publication leaves is removed when it can be. A restart is
  the only thing an unsynced publication could lose, and a restart makes the record untrusted, so
  odd, whatever it holds. Windows and Linux both let the record be replaced while a reader has it
  open, and that reader reads the record it opened.
- **Nothing caches a sample**, so none is stale; the cost is a read per sample, which the poller of
  the notification (`CountNotification`) bounds.
- **Raising belongs to the hold.** The outermost modification raises an even count to odd before the
  caller's first effect and remembers the value, closing it one higher on a settled end; nested
  modifications publish nothing. A count already odd -- a dead holder's, or untrusted -- is left
  odd through the modification, so only a completed recovery lowers it. A failed opening
  publication opens no modification and changes nothing; a failed closing publication, and an
  unresolved end, set the fence and leave the count odd. The holder's own operations are built here
  and reached only through `ExclusionHold`, which writer exclusion supplies.
- **The orderings the model assumes are the operating system's.** A raise is a rename that has
  returned before the holder's next effect is made, and a sample is a read made in the sampler's own
  order; both rest on one host's filesystem showing every process a completed rename at once, which
  the local filesystems the design is limited to provide.

Revert-probes: a sample that ignored the standing failed the cases of a count never published, one
no recovery vouched for, a record that is not one, and a sample after a restart; a failed closing
publication that set no fence failed its case.

[^m6-4]: `M6-4` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m6-4)
    [completed 2026-11-13; `d7daade`]

## Writer exclusion is one lock file, held whole, in the participant's table of holds, 2026-11-13

The file-based implementation's writer exclusion is an exclusive `Vfs::lock` on the whole of the
state's exclusion lock file[^m6-5]:

- **Every guard is its own holder**, so it excludes another participant in this process exactly as
  it excludes one in another process, and the contract's "every thread of every process" is the
  lock's own property rather than an in-process gate laid over it. It waits without promising an
  order among waiters, and the operating system releases it when its holder's process dies.
- **Not re-entrant.** A participant that holds it and asks again waits for itself, or is refused
  with `WouldWait`; canopy acquires it once, in its outermost write.
- **A hold outlives the borrow it was taken through.** A participant's table of holds is shared by
  every handle on it, so a hold carries a handle of its own and can be held as `'static`, as
  `Box<dyn ExclusionHold>` must be; abandoning the participant releases the lock even while the
  hold is still in hand, and dropping that hold afterwards releases nothing another holder has.
- **The hold's bookkeeping is the next layer.** The ownership record, the fence, the generation and
  raising the count all happen under this lock, in the `ExclusionHold` built over it, which is what
  joins `WriterExclusion` to `Coordination`; the lock and its release are what this layer
  guarantees, and its cases are run on the lock directly.

A revert-probe that kept the lock after its hold was dropped left the waiting cases waiting until
the harness's own timeout ended the run.

[^m6-5]: `M6-5` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m6-5)
    [completed 2026-11-13; `d7daade`]

## The hold of writer exclusion keeps its bookkeeping in files replaced whole, 2026-11-13

> **SUPERSEDED in part, 2026-11-18, by "Fifth review fixes: recovery's commit point, restart
> repair, foreign roots and the marker under abandonment" below.** Acquiring remakes the boot
> record after it writes the advanced generation, not before reading it. `complete_recovery`
> publishes the vouched even count and removes the fence; it does not record the boot vouched.
> The rest of this decision stands.

The file-based implementation's `ExclusionHold` keeps the ownership record, the fence, the
generation and the boot record under the exclusion lock, as canopy's design states them (canopy's
DESIGN-NOTES.md, "Writer exclusion and its ownership record, modelled in `Coordination.tla`")[^m6-6],
and joins `WriterExclusion` to `Coordination`:

- **Acquiring, in order:** take the lock; if an ownership record is there, its holder died, so set
  the fence; read the trust, and remake the boot record untrusted if it is missing, unreadable or
  from another boot; read the generation and write it advanced; write this holder's record last. A
  death or failure at any step before the last leaves the previous record for the next acquirer,
  and the lock is released. The fence is a file whose existence is the fence; the record, the
  generation and the boot record are replaced whole, and none is synced.
- **Releasing:** remove the record while the lock is held, then drop the lock, whether by `release`
  or by dropping the hold. A removal that fails still drops the lock and is reported by `release`;
  the record left behind costs the next acquirer a recovery, never correctness. An abandoned
  participant's hold does no bookkeeping: the record stays, as a death leaves it.
- **An unreadable generation is refused within a boot and read as zero across one.** Within a boot
  the generation is replaced whole, so an unreadable one means something else wrote it, and
  acquisition is refused rather than issue a generation a participant may remember. A power loss
  can leave an unsynced file empty, and no participant from before the restart remembers anything,
  so the acquisition that finds the state from another boot reads an unreadable generation as zero.
- **What recovery being required refuses.** An ordinary modification is refused while what the
  acquisition found requires recovery, or once this hold has set the fence itself by an unresolved
  end or a failed closing publication. A recovery modification is not refused.
  `complete_recovery`, refused while a modification is open, publishes a vouched even count, records
  the boot vouched and removes the fence, after which the hold's ordinary modifications are allowed.
  A death part-way through it leaves this holder's record, so the next acquirer sets the fence
  again.

Revert-probes: dropping the lock before removing the record made a live holder reported dead in the
case of two holders releasing and acquiring in turn, and an acquirer that did not set the fence
failed the case of the fence outliving the finder's clean release, with the other death cases.

[^m6-6]: `M6-6` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m6-6)
    [completed 2026-11-13; `d7daade`]

## The admission gate and the tiers are locks in two files, held in the participant's table of holds, 2026-11-13

The file-based implementation's admission gate is a lock on the whole of the state's gate lock
file, and its reader and writer tiers are a byte each of its tiers lock file, all taken with
`Vfs::lock` and kept in the participant's table of holds, as writer exclusion's lock is[^m6-7]:

- **Ordinary admission** takes the gate shared, then the reader tier shared, releases the gate,
  then takes the writer tier shared for a writer. A step refused or failed releases what the
  attachment had taken, so nothing is held. An attachment holds the gate only while it attaches.
- **An upgrade** releases its shared tiers first, so it never waits for the gate while holding a
  tier, then takes the gate exclusive, the reader tier exclusive when the upgrade is unsafe for
  readers, and the writer tier exclusive, in that order. A reader-safe upgrade releases the gate
  once it holds the writer tier, so readers attach and new writers wait on the writer tier; an
  unsafe one keeps the gate until it is dropped, so arrivals wait at the gate, holding nothing,
  rather than inside it holding the gate shared. A refused or failed upgrade leaves the participant
  detached.
- **The reader tier before the writer tier is what makes the order deadlock-free.** A writer that
  passed the gate before the upgrader holds the reader tier shared and may be waiting for the
  writer tier; an upgrader that took the writer tier first would wait for that writer's reader tier
  while the writer waited for its writer tier.
- **A death releases every lock**, the gate included, since the operating system releases a dead
  process's locks; abandoning the participant does the same in this process.
- **The registry entry goes first.** An attachment's or an upgrade's registry entry is released
  before its tiers, at a drop and at the upgrade's conversion alike.

Revert-probes: taking the writer tier before the reader tier left the case of a writer waiting
inside the gate and an unsafe upgrader deadlocked until the case's own wait ran out; keeping the
gate after the reader tier was taken failed the case of the gate being free after attachment and
the safe upgrade's case; keeping the gate after a safe upgrade, or releasing it after an unsafe
one, failed the case that only the unsafe upgrade keeps it; and an upgrade that kept its shared
tiers waited on itself.

[^m6-7]: `M6-7` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m6-7)
    [completed 2026-11-13; `d7daade`]

## The registry is a pair of lock bytes and a record file per entry, every lock taken as a try, 2026-11-13

The file-based implementation's registry, as canopy's design states it (canopy's DESIGN-NOTES.md,
"The registry of attached handles, modelled in `Coordination.tla`")[^m6-8], joining `Registry` to
`Coordination` and `Registrant` to `Attachment` and `Upgrade`:

- **An entry is two bytes of the registry lock file and one record file.** The first byte is the
  claim lock, the second the liveness lock; the record holds the token, the tiers and the caller's
  payload, and is read and written only under the claim lock. There are 64 entries.
- **Registering** tries each entry's claim lock, then its liveness lock exclusively, moving on from
  an entry where either is held; writes the record; and lets go of the claim lock, keeping the
  liveness lock for as long as the registration lasts. Registering again rewrites the record under
  the claim lock with the same token, and is refused with `WouldWait` while a listing has it, the
  record keeping its previous payload.
- **Leaving** claims the entry if it can and removes the record, then releases the liveness lock.
  An owner that finds the claim lock held releases only the liveness lock, which is what makes the
  entry dead. It happens before the tiers are released, at a drop and at an upgrade's conversion;
  an upgrade registers afresh once it holds its tiers, with a new token.
- **Listing** claims each entry, probes its liveness lock shared, and reads the record only when
  the probe is refused. An entry it cannot claim, or a held entry whose record cannot be read, is
  counted unknown. It takes no tier and needs no attachment.
- **The overflow record names the boot it was written in.** A participant that finds every entry
  held writes it, replacing it whole, and a listing reports that it may be incomplete while the
  record names this boot -- or cannot be read, or the boot cannot be identified. A host restart
  therefore discards it without anyone removing it.
- **A token is this process's identifier, the time its first token was made, and a count**, so two
  registrations in one process differ by the count and two processes by their identifier or, when
  an identifier is reused, by the time.

Revert-probes: a listing that did not claim the entry it inspected reported a dead token live
beside a reuser's liveness lock, in the paused reuser's case, the case of a listing run while a
reuser writes its token, and the churning case; a registrant that let go of the claim lock before
writing its token failed the case of the listing run during that write; leaving the registry after
the tiers failed the case that an entry is cleared while its tiers are held; an upgrade that kept
its entry through the conversion failed the conversion's case; and a listing that ignored the
overflow record failed both overflow cases.

[^m6-8]: `M6-8` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m6-8)
    [completed 2026-11-13; `d7daade`]

## One poller per process samples the count for every registration, each with its own backoff, 2026-11-13

The file-based implementation's `CountNotification`, as canopy's design states it (canopy's
DESIGN-NOTES.md, "A change to the coordination state is delivered as a notification")[^m6-9],
joining `CountNotification` to `Coordination`:

- **One poller thread per process**, started by the first registration and kept for the life of
  the process, serves every registration on every root. It waits without a timeout, and samples
  nothing, while no registration is outstanding.
- **Each registration has a backoff of its own**: sampled at once when it arrives, then after 1 ms,
  doubling to 32 ms. A shared backoff would let every new registration make all the others sample
  faster, which in a busy process means never backing off. The ceiling is set against a write
  holding the count odd for tens of milliseconds.
- **Registrations through one participant that are due together are sampled once.**
- **A registration rings once and is gone**, when its count differs from what it sampled, when its
  deadline passes, or when its count cannot be sampled -- so the caller, sampling again, meets the
  failure itself. A dropped notice withdraws a registration that has not yet been taken to ring.
- **Rings run on the poller's thread, outside its lock, each under `catch_unwind`**, so a ring that
  panics costs its own registration and not the poller every other registration in the process
  depends on.
- **The poller's waits are condition-variable waits with a timeout.** They are not among the timer
  entry points the workspace's lint governs, and they are the poller's purpose rather than a delay.

Revert-probes: a fixed interval with no backoff exceeded the sample bound; a ring decided without
the count failed every case that rings on a change; a withdrawal that did nothing rang a dropped
notice; a registration kept after it rang went on sampling, which the case of no sampling after a
ring caught once it held its notice past the ring; and a ring run without `catch_unwind` killed the
poller and failed the panicking ring's case.

[^m6-9]: `M6-9` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m6-9)
    [completed 2026-11-13; `d7daade`]

## A root that already holds artifacts is reserved without creating any, 2026-11-13

`Creation::reserve` records an implementation's reservation for a root that already exists and holds
what the caller's own format put there[^m7-2]. A store migrated from before the coordination layer is
the case: it has a marker and trees, and the root has to name the implementation that coordinates it
from then on.

- **It is the reservation step of `create`, and no more.** It reads the root, takes the creation
  exclusion, reads again, and publishes the reservation or resumes the one that is there, and it makes
  no artifact and does not look at what the root holds. The code is shared with `create`, so the two
  cannot disagree about what a reservation is or when one is refused.
- **What it answers.** `Created::Created` when it made the reservation, `Created::Resumed` when this
  implementation's reservation for the same inputs is already there, `CoordError::Reserved` for any
  other implementation or other inputs. A retry with the same inputs is therefore idempotent, which is
  what lets a caller whose migration was cut short finish it.
- **The model is not extended.** `reserve` is the reservation publish step of `Creation.tla` and
  nothing else of it, so the properties the model checks for that step cover it. What a caller must
  establish for the artifacts it adds around the reservation is the caller's design, not the model's.

Revert-probes: reserving without the second reading under the exclusion, without the sync of the
root, without comparing the inputs, and with an overwriting publish each failed the cases built for
them; the first needed the case where a reserver that waited resumes the reservation the holder made.

[^m7-2]: `M7-2` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m7-2) [completed 2026-11-14; `87a52e7`]

## Whether recovery is required is observed with the writer lock tried, 2026-11-14

`RecoveryDiscovery::discover_recovery` reports whether recovery is required and why, for any
participant, as canopy's recovery design states it ([canopy's
DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Recovery and its discovery, modelled in
`Coordination.tla`")[^m7-4].

- **The observation is made consistent by the lock.** The participant tries writer exclusion without
  waiting. A lock it gets means no holder is alive, so the fence, the ownership record and the boot
  record are read while it holds the lock, with no acquirer able to come between the readings, and a
  record found is a dead holder's. A lock that is held means a holder is alive, whose record is its
  transaction, so only the fence and the boot record are read; a holder that finishes a recovery
  while they are read can make the answer stale in the direction of reporting work that has just been
  finished, never of missing work that is there.
- **It is a probe, not a hold.** It drops the lock without the bookkeeping of a release: it sets no
  fence, replaces no record and advances no generation, so the next acquirer is told exactly what it
  would have been told had nobody asked. Asking leaves the lock file as the one thing it may create,
  and a root with no state directory has had no holder, so it is not probed at all.
- **A participant that holds exclusion is a live holder to itself.** Its own try fails as another's
  would, so its record is not reported and its fence and restart state are.
- **What is reported.** The fence, why the state cannot be vouched for, and whether a holder died; the
  fence is set by an acquirer that finds a record or by an unresolved ending, never by a discovery.

Revert-probes: reporting a record whatever the lock says, not reading the fence, clearing the fence it
finds and not reading the restart state each failed the cases built for them.

[^m7-4]: `M7-4` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m7-4) [completed 2026-11-14; `87a52e7`]

## A count notification on Windows is a lock the system breaks, 2026-11-14

On Windows over the operating system's own filesystem, the file-based implementation's
`CountNotification` is a lock the system breaks when a writer renames a fresh count over the one
published, as canopy's design states it (canopy's DESIGN-NOTES.md, "A change to the coordination state
is delivered as a notification")[^m7-7]. A registration holds no thread and samples nothing while it
waits: the break is an I/O completion and the deadline a timer, both on the thread pool.

- **The lock is a read-and-handle-caching oplock**, requested with `FSCTL_REQUEST_OPLOCK` on a handle
  opened for overlapped I/O that shares read, write and delete, and bound to the pool's I/O. A read
  oplock alone is not broken by a rename over the file, which was measured; the handle-caching one is,
  from read-and-handle to read.
- **The break holds the renaming writer until it is answered.** The holder answers it in the
  completion, with the same control acknowledging at level none, which completes at once and lets the
  rename go on. A holder that does not answer -- a process whose pool cannot run callbacks, such as one
  stopped in a debugger -- delays the writer's publication until it closes the handle or the system's
  break timeout passes. This is the price of being notified and not polling, and it contradicts the
  property that a reader never stands between a writer and its publication, so
  `FileFactory::polled` makes a factory's participants take the poller alone.
- **The break comes before the rename finishes**, so a registration whose lock broke samples again on
  pool timers, from 250 microseconds and doubling, up to six samples and no later than its deadline,
  and rings once the count differs or the samples are spent.
- **The lock must be on the published file.** After it is granted the file it is held on is compared
  with the one now at the count's path, and the count is sampled again; if either differs, the
  registration goes to the poller, because a rename between the open and the grant would leave a lock
  nothing breaks.
- **The poller takes the registration, unchanged, wherever this form cannot be made**: the factory
  polls, the filesystem is not the system's, the count has no file yet (the first publication renames
  into a name nothing holds, which breaks nothing), the lock is not granted, the deadline has passed,
  or the count has moved.
- **An abandonment rings the registrations it leaves**, since nothing breaks for a participant whose
  own operations are refused and the caller must meet the refusal; the participant keeps a weak list of
  them for it.
- **A notice dropped from inside its own ring releases its lock and timer on a thread of its own**,
  because releasing them waits for their callbacks, and the ring is one.
- **The lock is outside the seam.** It is made on the system's own file, as `Vfs::open_output` is, so a
  `FaultyFs` rule does not reach it; the seam says whether it applies with `Vfs::is_os_filesystem`.

Which form a store uses is the caller's, by the factory it gives canopy's `StoreOptions`; nothing
further is scheduled.

[^m7-7]: `M7-7` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m7-7) [completed 2026-11-14; `87a52e7`]

## A failed first registration records possible incompleteness, 2026-11-14

Registration is best-effort, so a failure before an entry is obtained can leave an attached
participant absent from the registry. The file implementation attempts to record overflow for
that failure as it does for a full registry, and returns the original registration error. A
failed update of an existing entry leaves that entry's payload and membership intact and does not
mark overflow merely because the update failed. The marker remains until the host restarts.

Recording overflow is itself fallible: a filesystem that refuses both writes cannot communicate
the missing membership to an independent listing. The attachment remains valid and the original
error remains available to its caller. Tests inject failures during directory creation, claim
and liveness locking, and payload writing while checking both retained tiers and incompleteness.
This behavior supports store attachment[^m8-2]; it changes neither admission nor registry lock order.

[^m8-2]: `M8-2` in [canopy's COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m8-2) [completed 2026-11-14; `87a52e7`]

## Recovery discovery lends a read-only inspection, 2026-11-14

`RecoveryDiscovery::discover_recovery_with` invokes its inspection only when its probe holds
free writer exclusion, before releasing that probe. The inspection may read caller-owned
recovery records but must not mutate or acquire exclusion. Held exclusion skips inspection;
without a state directory there is no probe to lend, so it skips inspection there too.
Ordinary discovery uses an empty inspection and retains its existing result and no-mutation
contract. An inspection that unwinds releases the probe through its guard.

This lets canopy report its root work list in the same observation that establishes no live
writer owns it. No owner record is written and no generation is advanced by the probe.
The turn-taking implementation is tracked in canopy's [CHECKLIST.md](../canopy/CHECKLIST.md).
Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#discovery-inspection).

## Test support supplies a distinct file coordination implementation, 2026-11-14

The opt-in `test-support` feature makes `FileFactory::alternate` available to process fixtures.
That factory has its own implementation identity and creation exclusion, while preserving the
file coordination contract and layout of its other state. `FileFactory::new` retains its canonical
identity and layout. A root reserved through one factory cannot be attached through the other.

This is a test capability, not an identity override on the canonical factory. The process-level
refusal is exercised by the writer fixture; the work is tracked in canopy's
[CHECKLIST.md](../canopy/CHECKLIST.md). Rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#alternate-process-factory).

## Incremental upgrade conversion retains the admission gate, 2026-11-14

`Attachment::begin_conversion` unregisters and releases shared tiers, then returns a conversion
whose gate and tier polls never wait for a lock. Granted holds remain owned until the conversion
finishes or is dropped; a failed poll releases them. The selected safety judgment cannot change
after tier acquisition begins. A completed conversion transfers those holds to an upgrade while
retaining its gate, and `Upgrade::admit_readers` releases it only for reader-safe upgrades.

The blocking `Attachment::upgrade` and the incremental path remain separate specified primitives:
the blocking baseline uses native waits and reopens safe readers before returning; the incremental
path lets its consumer publish admission before explicitly reopening them. Both are retained for
those distinct contracts, not consolidated into polling or retuned for one consumer. This supports
canopy's schema conversion[^m9-4]; rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#incremental-tier-conversion).

[^m9-4]: `M9-4` in canopy's [COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m9-4) [completed 2026-11-14; `bb46904`]

## Shared coordination state belongs to a live epoch, 2026-11-14

The shared-memory path is separate from the file-based coordination backend. Its state is a fixed
layout of aligned atomic words in an unnamed file-backed mapping, with `memmap2` providing the
native mapping because that facility matches the owned shared-access and lifetime contract. The
mapping's file has a name reserved to coordination; callers never truncate or remove it while an
epoch has participants. Separate setup and liveness lock files keep data outside locked spans.

An initializer holds setup exclusively, probes liveness exclusively and retains that probe while
it creates or recreates state. It clears trust and ownership, sets the recovery fence and publishes
the ready format only after the shared words are initialized. It records the host boot durably,
then exchanges the exclusive liveness probe for its own shared liveness while retaining setup.
Concurrent openers therefore cannot see an initialization gap as a live trusted epoch. Native
file-backed state deliberately survives mapping closure, but absence of any liveness holder makes
its prior trust unusable. A damaged inactive instance or mapping is recreated untrusted; a damaged
live instance or mapping is refused without destructive repair.

The boot record, a missing last participant, incomplete initialization and an unknown state format
are independent reasons to require recovery. An unknown boot is never vouched for. Abandonment
releases only that participant's liveness and refuses its later operations; ownership and shared
records remain for recovery. An in-flight diagnostic keeps liveness serialized with abandonment,
so a recreated mapping cannot undercut a read through its previous participant.

MemFs uses shared atomic words keyed by the instance identity and modeled boot. Its word storage
is retained only by participants, and a recreated epoch is fenced just as a native one is. Tests
separate last-participant exit, host restart, injected initialization failure and loss of the last
real OS process. The ordinary file backend remains independently selectable.
This state foundation is recorded in canopy's
[COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m10-1). Rationale
is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#shared-state-epochs).

## Shared writer holds preserve recovery evidence, 2026-11-14

A shared writer holds the specified VFS exclusion lock and retains its participant's mapping and
liveness. Its resource table releases every owned lock on abandonment without clearing ownership;
dropping the original participant does not release resources an outstanding hold still owns. The
owner is a nonzero acquisition-generation token. Acquisition and release meet on that one word
with acquire/release RMWs, so a new acquirer observes the predecessor's publication without
depending on incidental OS-lock or code-generator ordering. Abandoned ownership fences the root
before the record is replaced. Generation and hold identities advance without wraparound.

The opening count raise is an acquire/release RMW before any effect, and nested work raises only
once. **UNRESOLVED-MODIFICATION-EVIDENCE** means that an unresolved inner ending fences every
participant and preserves odd parity for the whole logical modification; a settled outer ending
cannot erase it. An open modification's dropped hold preserves recovery evidence. Ordinary work
is refused behind the fence, and only completed recovery publishes a trusted even count and
clears the fence. Count or nesting exhaustion is an explicit `CoordError::Exhausted`, not a reused
generation or count; an opening is refused if its closing raise cannot be represented.

The same evidence rule belongs to the established file path. If its fence publication fails, a
release leaves ownership in place rather than erasing the only record that requires recovery.
Its native-wait and file-publication mechanisms remain separate from the shared implementation;
these are correctness repairs to the specified contract, not a new discipline or consolidation.

Discovery's free-exclusion probe is owned by RAII and advances no root generation or record,
including when the inspection panics. A nonchanging owner RMW supplies the same observation
ordering as acquisition. A live holder is not reported dead. The virtual filesystem's changed
boot rejects old holds before any later effect; explicit abandonment remains the separate modeled
process-death action. Native mapped words use a mutable pointer captured before sharing, and
that mapping's fixed lifetime bounds every atomic reference. Invalid shared headers cannot be
vouched for merely by completing a count transition.

This work is the writer/fence/generation part of the shared-memory milestone[^m10-2]. Rationale is
in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#shared-writer-evidence).

[^m10-2]: `M10-2` in canopy's [COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m10-2) [completed 2026-11-14; `e0500f8`]

## Shared admission and registry retain one resource lifetime, 2026-11-15

Admission holds the gate shared, then the reader tier shared, releases the gate and takes the
writer tier shared for write access. Each tier is a distinct OS-lock file, owned by the same
participant resource table as writer exclusion, so death releases every held tier independently
of mapped bookkeeping. A conversion unregisters and releases its shared tiers before acquiring
the gate exclusively. The native blocking conversion remains distinct from incremental polling;
polling retains partial acquisitions and cancellation drops them without reacquiring attachment.
Safe polling keeps reader admission closed until its consumer explicitly admits readers; unsafe
conversion retains the gate and reader exclusion through completion. A changed safety choice is
refused and releases the partial holds.

Registration belongs to those same attachment/upgrade handles. Mapped slots carry the token,
declared tiers and immutable payload reference; arbitrary opaque payload bytes live in fresh
files, so no fixed buffer silently narrows the contract. Every registration and inspection holds
an entry's nonblocking claim lock, and its registrant retains exclusive liveness from before
publishing the slot until unregistering. A listing proves liveness under that claim, counts
claimed/unreadable slots unknown and takes no attachment tier. Updates publish a new payload
before changing its reference, retain their token, and leave the previous payload intact on
failure. Unregistration removes the entry before releasing tiers; an abandoned registrant neither
updates nor erases a reuser's slot. Stale mapped tokens refuse updates.

The persistent epoch ordinal and checked registry sequence prevent reuse of a token on that
root across last-participant exit. The model's in-process instance is minted again only when
exclusive liveness proves a fresh epoch, so copied files in an independent filesystem cannot
alias live word storage. Exhausted or malformed epoch identity is refused rather than reusing
an earlier registration identity. Registry capacity exhaustion leaves an attachment unregistered
and marks membership incomplete; a failed first registration also attempts that mark without
replacing its own error. The incomplete mark survives epoch recreation until a recognized host
restart. Only an initializer with exclusive liveness removes dead-epoch payload artifacts.

Admission and registry are one work item because `Attachment` and `Upgrade` both require real
`Registrant` behavior. Neither interface is implemented by a placeholder while its other half
waits. Native process tests retain a registered reader while an unsafe conversion waits, then
kill it and require both tier release and refusal to report its old record as live. The coupled
work is tracked by canopy's shared-memory milestone[^m10-3], with rationale in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#shared-admission-membership).

[^m10-3]: `M10-3` in canopy's [COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m10-3) [completed 2026-11-15; `e0500f8`]

## Shared count sampling and notifications expose the whole contract, 2026-11-15

`SharedFactory` makes a lazy participant whose construction changes no root data or coordination
state. First use checks any creation reservation's implementation identity before initializing
the shared epoch. Its separate creation exclusion and stable `acadia-shared-v1` identity use the
owned reservation wire format: opaque canonical inputs, creation nonce and implementation. Root
creation reads before and after exclusion, resumes only identical inputs and publishes the marker
last; confirmation refuses foreign reservations before relying on shared state. Creation locks
belong to the participant's abandonment table, and callbacks cannot finish a later artifact after
abandonment. Initialization never makes abandonment wait behind a blocked initializer.

Sampling is one acquire/release RMW on the mapped count, with no writer lock, syscall or composite
record on the native fast path. Its release ordering retains the read-before-sample bracket and
its acquire ordering retains the sample-before-read bracket; only an exclusion holder changes the
count's value. Untrusted epochs begin odd and only completed recovery admits a trusted even count.
The virtual backend additionally rejects a modeled old boot, because its old process objects can
still exist in the test after the model's power loss.

One shared-backend poller per process samples outstanding registrations, with independent bounded
backoff and a wait while none are outstanding. It rings once for changed count, deadline or failed
sampling, withdraws registrations on notice drop and contains callback panics. Callbacks and retired
registrations are dropped outside the poller mutex. No registrant dedicates a thread; default file
notifications and their Windows oplock path remain independent implementations of the same contract.
Real process tests require a child publication to change the parent's mapped sample and ring its
notice, rather than treating a timer ring as evidence of publication.

Generation ranges are reserved durably at epoch creation and renewed only when a live writer
exhausts its current reservation. The ordinary acquisition remains atomic mapped bookkeeping,
but cold epochs begin beyond every previously reserved generation. The bounded range record is
atomically replaced, checked with its complement and never reused after malformed identity or
exhaustion. This preserves the generation contract across loss of the last participant rather
than making a consumer infer cache identity from the current implementation's trust behavior.

Public-interface conformance runs the same creation, count, ownership, admission, registry and
notification cases against canonical file, alternate file and shared implementations over MemFs,
OsFs and FaultyFs. **ATOMIC-REGISTRY-PAYLOAD-UPDATE** means that a failed replacement preserves the
prior live payload and token. A torn-write control exposed the file path's in-place update; its
source repair publishes a fresh record by atomic rename under the existing claim lock, without
changing its registry lock discipline or replacing the file baseline. This whole-contract
completion is the count/factory work[^m10-5]; rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#shared-whole-contract).

[^m10-5]: `M10-5` in canopy's [COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m10-5) [completed 2026-11-15; `e0500f8`]

## Coordination cost and independent backends, 2026-11-15

The raw public-contract comparison measures sample batches and closed writer loops in four real
processes, alternating backend order across replications. The measured sampling interval excludes
warm-up and the handoff interval excludes process setup; it includes acquisition, ownership and
generation bookkeeping, modification opening/closing and release, but no document work. Each
completed turn is checked against its acquisition generation and modification count, so a timing
report cannot silently omit a turn or count raise. The Windows run uses the Windows filesystem;
the Linux run builds and executes from the native ext4 checkout with data and artifacts there,
not through the Windows-backed mount.

The shared backend reduces sampling cost by eliminating the per-sample file open/read and reduces
raw writer handoff cost by keeping ownership, generation and count transitions in mapped words.
Both platform comparisons observe that reduction without a lost generation, completed turn or
count raise. This finding is about the coordination mechanism, not end-to-end transaction latency:
the store's journal and document syncs remain their own costs. Finite closed loops cannot establish
a stationary tail or a fairness promise, and exclusion remains unordered in both implementations.

Store-level process checks exercise interleaved logged/unlogged writes, snapshot visibility, death
at each durable writer boundary and interrupted schema migration resumed by a target process.
Canonical file and shared processes refuse each other's roots before changing their reservation;
their admission identity is part of correctness, not an optional benchmark setting. The common
strict deadline-only multi-registration control uses the polled primitive, because default Windows
oplocks can deliver permitted early hints. Default notifications remain covered as hints whose
consumer samples again rather than inferring the changed condition from callback time alone.

The speculative path's merge-or-delete decision is **retain both implementations independently**:
the file backend remains the specified default baseline, and the shared factory is explicit opt-in
with its own root identity and epoch semantics. Its common contract and native process checks prove
the separate path without authorizing a collapse of the established one. The benchmark and public
process evidence are the final shared-memory milestone work[^m10-6]; commands and raw report schema
are in [README.md](../canopy-lock-holder/README.md#coordination-implementation-comparison), and
rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#coordination-cost-comparison).

[^m10-6]: `M10-6` in canopy's [COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#m10-6) [completed 2026-11-15; `e0500f8`]

## Sealed native wait capabilities on both platforms, 2026-11-15

The signal owner can lend a native wait capability that safe code cannot use to change its level.
Windows lends the published family's `SealedWaitOnlyEvent`; Linux lends acadia's owned
`SealedWaitOnlyFd`, a duplicate eventfd retained through clones. Neither sealed value offers safe
borrowed/raw-handle or descriptor extraction, ownership conversion, or an I/O interface. Debug
output exposes no numeric target identity. Opaque mixed-wait objects retain that same boundary.
The owner and any previously retained unsealed alias remain able to control the signal.

The safety profile is the type boundary, not platform access rights. Linux eventfd duplicates have
no wait-only kernel access mode: reading drains the level and writing raises it. Only the explicit
unsafe `as_waitable_fd` borrow can expose that descriptor, with a contract forbidding I/O, close,
flag changes and exposure of derived duplicates to mutating code. Windows' published unsafe
borrow applies the corresponding wait-only restriction to handles. Safe single/mixed waits observe
readiness without consuming the signal. Linux's sealed watch registers with the existing epoll
service and never lends a descriptor to its callback; Windows sealed thread-pool activations
expose rearming but no safe handle route.

Unsealed signal APIs remain distinct lower-layer capabilities: EventSignal's event/handle and
wait-only APIs, EventFdSignal's AsFd and Signal control operations are retained. Sealing a lent
value is not a reason to remove the owner's platform surface. The doorbell keeps the mutable
owner private and only lends sealed capabilities safely. Compile-fail tests pin extraction routes,
while kernel tests pin level preservation, mixed readiness, clone lifetime and service callbacks.
This is the native-wait capability work[^g-2], with rationale in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#sealed-native-wait-capabilities).

[^g-2]: `G-2` in canopy's [COMPLETED-CHECKLIST.md](../canopy/COMPLETED-CHECKLIST.md#g-2) [completed 2026-11-15]

## The shared implementation announces its steps to a test-support hook, 2026-11-15

canopy's conformance replay holds each process before each step of the coordination model and
compares the contract's state after it ([canopy's DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md)
"The contract's writer actions in the code"). The file-based implementation's steps are
filesystem calls, which the replay sees through its `Vfs`; the shared implementation's change
shared words, which it does not. So, with the `test-support` feature, the shared implementation
announces each such step before it acts[^ac-5]:

- **`SharedFactory::with_hook(hook)`** makes participants that call `StepHook::before` with a
  `SharedStep` before each step that changes the words, named for the model step it begins:
  setting the fence on acquiring, replacing the ownership record, reading and publishing the
  opening raise, reading and publishing the closing raise, the fence an unresolved operation sets,
  publishing the recovered count, clearing the fence, and clearing the record on release. A nested
  operation announces nothing, as it changes nothing. A hook may hold a step for as long as it
  likes. The announcements compile to nothing without the feature, and a factory without a hook
  is the ordinary one.
- **`inspect(vfs, root)`** reads the trust, fence, owner, generation and count words as one
  reading finds them, taking no lock and changing nothing, and finds none while no participant of
  the current boot has made them. On `MemFs` it finds the in-process words a participant of the
  same boot made; over the operating system's filesystem it reads the mapped file.
- **An epoch ends with its last participant.** A participant that dies or detaches while no other
  is attached ends the words' epoch, and the next participant begins an untrusted, fenced one. A
  dead holder's record therefore outlives it only while another participant keeps the epoch, which
  the replay must take into account wherever every process of a trace has ended.

[^ac-5]: `AC-5` in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ac-5) [completed 2026-11-15; `dfcd5be`]

## MemFs durability reference and replay, 2026-11-17

- **The reference separates live names, durable names and file data.**
  [MemFsDurability.tla](spec/MemFsDurability.tla) assigns every path an absent, directory or
  inode-tagged file node in both live and durable trees. File fsync advances data for one inode;
  directory fsync advances direct names and any carried directory subtree, not file contents.
  Crash replaces live names and readable bytes with the durable projection.
- **Carried directory state is part of the contract.** A directory moved after its contents became
  durable retains a snapshot at the destination even if the old parent is synced first. A directory
  standing where the durable path still names a file stores synced contents in a carried snapshot
  until its parent names the directory. Nested carried contents survive only when the relevant
  parent snapshot is refreshed; the root snapshot keeps descendants that no direct child sync owns.
- **Rename may leave two durable file names for one inode.** File rename is atomic in the live
  tree but directory durability is independent. Syncing the destination parent before the source
  parent leaves both names after a crash, both referring to the same inode. The reference does not
  require deduplication or directory-sync ordering across parents.
- **Unnamed descendants are lost precisely at the unsynced link.** Syncing a directory's own
  contents does not name that directory in its parent. A crash therefore retains an explicitly
  named ancestor while dropping descendants whose containing directory name was not synced.
- **The model is checked through a replay bridge.**
  [mem_model.rs](src/tests/mem_model.rs) drives MemFs through the same named actions and audits inode
  references after every step. It checks the four historical defect shapes and normal file, name,
  directory, rename and removal traces. Scenario witnesses prove the expected states are reachable;
  controls and probes make file-data, name, carried-snapshot and crash errors fail
  `ScenarioExpected`.

## MemFs directory removal in the durability reference, 2026-11-17

- **Removing a directory changes only the live tree and the carried snapshots.**
  [MemFsDurability.tla](spec/MemFsDurability.tla) models `remove_dir` as `RemoveEmptyDir` and
  `remove_dir_all` as `RemoveTree`. Both leave the durable tree to the parent's next directory sync:
  a crash before that sync restores the whole removed subtree, and a crash after it loses the whole
  subtree. `RemoveTree` empties every live path beneath the removed directory in one step, so the
  live tree never names a child of a missing directory.
- **Removal discards the carried snapshots at or beneath the removed path.** A snapshot carried by a
  directory that was moved, or that stands where the durable tree still names a file, describes
  that directory's contents. Once the directory is removed the snapshot describes nothing, and a
  directory created later at the same path must start empty rather than adopt it. This holds for
  an empty-directory removal too, because the snapshot outlives the live entries it was taken from.
  `MemFs` does the same through `drop_carried_under`.
- **Scenarios, controls and replay cover each rule.** [MemFsScenarios.tla](spec/MemFsScenarios.tla)
  witnesses an unsynced recursive removal undone by a crash, a synced one surviving it, and a
  moved directory recreated empty after either kind of removal. Controls that keep the carried
  snapshot, leave descendants behind or make the removal durable without a sync each fail, and the
  probe corpus catches each one. [mem_model.rs](src/tests/mem_model.rs) replays the four scenarios
  through `MemFs`.

## Review fixes: stale staging names and abandonment during a change, 2026-11-17

- **A publication's staging name being taken never fails it.** `OsFs::publish_new` stages under
  `<name>.publishing.<process id>-<counter>`, so a sibling can be left by a crashed process whose
  id this one reuses, with its counter restarted. A taken staging name is skipped for a fresh
  one, a bounded number of times, and is never removed: in another process-id namespace it may
  belong to a live publisher. Running out of names fails as `Other`, never `AlreadyExists`, which
  is the answer that the target itself is taken.
- **An abandoned file-based participant changes nothing after `abandon` returns.** Every change
  the file-based implementation makes to a record goes through `FileCoordination::publishing`,
  which holds the participant's table of holds for its duration and refuses once the participant
  is abandoned; `abandon` takes the same table. An abandonment therefore waits for a change in
  progress and, once it returns, no further change lands, as for a dead process whose locks may
  already be another participant's. A release whose bookkeeping races an abandonment reports
  nothing, as a death does. The shared implementation's held-resource guard gives the same
  property. A test harness that parks a call inside a change must make that call fail before it
  abandons the participant, or the abandonment waits for it forever.

## Second review fixes: lock-free sampling, MemFs naming and lock checks, 2026-11-17

> **SUPERSEDED in part, 2026-11-17, by "MemFs marks travel with what they describe; empty lock
> ranges; the poller retires outside its lock" below.** A rename does not copy the moved
> directory's unnamed marks to the new path; they travel with its carried snapshot and apply when
> it is materialized. The rest of this decision stands.

- **A file-based sample takes no lock.** The abandoned flag sits beside the table of holds as an
  atomic, set only while the table is held. `alive()` reads it without the table, so a sample --
  promised to take no lock and wait for no writer -- never waits for a change its own participant
  has in progress; `publishing`, `take_lock` and `abandon` read or set it under the table, which
  keeps an abandonment serialized with every change.
- **File-based creation checks liveness around each artifact**, as the shared implementation does,
  so a creation whose participant is abandoned while an artifact is made stops there.
- **MemFs names a moved directory's descendants by its snapshot.** A directory sync marks a path as
  having no durable name only when the durable tree lacks it after the carried snapshots above it
  are materialized, so syncing a descendant of a moved directory and then its parent keeps what the
  move carried. A rename carries the unnamed marks beneath the moved directory to the new path, so
  a descendant never named by syncing the moved directory stays unnamed after the move. Both are
  `MemFsDurability.tla` behaviors, pinned by scenarios and replays.
- **MemFs checks locks atomically with the change they guard.** An operation keeps the lock table
  from its check of the holds until its change to the tree is made; the table is always taken
  before the tree, as taking a lock already did. A lock taken while an operation is checked waits
  for that operation, so a holder never sees its locked bytes change.
- **The shared implementation's in-process model is keyed as MemFs resolves a path** (`..`
  applied, prefixes and `.` dropped), so every spelling that reaches one MemFs directory shares one
  model.
- **Linux `wait_native` reports a dead descriptor.** Readable data is reported first; otherwise an
  invalid, failed or hung-up descriptor is an error, as `wait_native_any` reports it, rather than a
  poll that returns at once until the deadline.

## MemFs marks travel with what they describe; empty lock ranges; the poller retires outside its lock, 2026-11-17

- **An unnamed mark belongs to the directory content it describes.** A rename that carries a moved
  directory's snapshot carries the marks beneath it with the snapshot, keyed by the destination
  path, and they apply only when the snapshot is materialized by a sync. Until then the
  destination's own marks stand, so its old durable contents survive a crash before the parent
  sync, as `MemFsDurability.tla` requires. A sync that replaces a durable directory -- refreshing
  it or materializing a snapshot over it -- drops the marks beneath it before the carried ones are
  adopted, so a mark never outlives the directory it was taken in.
- **A directory sync keeps marks only beneath children that are live directories.** A path whose
  name now holds a file, or nothing, has no descendants for a mark to protect, and a stale mark
  there would drop a file synced in its place.
- **An empty byte range overlaps no lock.** An operation that touches no bytes -- a zero-length or
  end-of-file read, an empty append, a truncation to the same length -- is never refused by
  another process's lock.
- **A rename of a path onto itself resolves its source**, so a missing source or parent fails as
  it does on `OsFs`.
- **The file poller retires registrations outside its lock.** Retired registrations are taken out
  of the state under the lock and dropped, with their rings run, after it is released, because a
  registration's drop may withdraw another notice and take the same lock.

## An identical creator finds the store it asked for, 2026-11-17

`Creation::create`, in both implementations, ends with `Created::Existing` when it finds the marker
under a reservation that names its implementation and records the plan's inputs, at its first
reading or under the creation exclusion. This is canopy's decision ([canopy's
DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Store creation is idempotent"), and
[Creation.tla](../canopy/spec/Creation.tla) is the model it follows.

- **The marker is made durable before `Existing` is returned**, with its directory entry, as
  `confirm_created` makes it: the marker's creator may have died between making it and making it
  durable, and a caller that goes on to use the store must not lose it to a crash.
- **At the first reading it returns without taking the creation exclusion.** The marker is the
  last artifact and every one before it is durable when it is made, so the store is complete;
  syncing the marker is idempotent whoever holds the exclusion. The model finds no property that
  needs the wait, as it found for an opener.
- **Any other marker is still `AlreadyCreated`**: a reservation of another implementation or
  other inputs, or none, means the root is another store.
- **`reserve` is unchanged.** It reads no artifact and so no marker; a caller that makes its store
  itself decides what a finished one means.

Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), "Finding the store is a success of
create".

## A creation's own publication leftovers are its artifacts', 2026-11-17

`Creation::create`, in both implementations, reads a sibling that `Vfs::publish_new` stages an
artifact in -- the artifact's name, `PUBLISHING_INFIX` and a non-empty suffix,
`is_publication_staging_of` -- as that artifact. A crash during an artifact's publication can leave
one, and `publish_new` gives it no meaning, so it must not make the root read as holding something
creation does not make: that would leave a reservation no later call could finish. Like an
artifact, such a leftover with no reservation still occupies the root, since no creation leaves an
artifact before its reservation.

- **The convention is named once, in the `Vfs` contract**, and `OsFs` builds its staging names
  from it; a test holds every name `OsFs` tries to the recognizer.
- **The model is unchanged.** [Creation.tla](../canopy/spec/Creation.tla) makes an artifact in one
  step that exists or does not; a staging sibling is how a publication reaches that step, which
  the model abstracts away, not something under the root that creation does not make.
- This is what lets canopy publish its marker with `publish_new` while requiring a root that holds
  nothing but creation's own entries (canopy's [DESIGN-NOTES.md](../canopy/DESIGN-NOTES.md) "Store
  creation is idempotent").

Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), "Recognizing a publication's
leftovers".

## A publication is a move on Windows, 2026-11-17

On Windows `OsFs::publish_new` stages and syncs the bytes as before, then gives the staged file
the target's name with `MoveFileExW` and no `MOVEFILE_REPLACE_EXISTING`. That move fails where
anything has the name, a file or a directory, so the exclusivity of `publish_new` holds, and the
file it publishes has one name. The staging sibling is removed only when the move fails. On Unix
the staged file is still hard-linked and its staging name removed.

- **Why not a hard link on Windows.** A file published as a link whose staging name is then removed
  could not be replaced by a rename for up to about a second, refused with `Access is denied`. The
  holder is outside the process -- reproduced with `std::fs` alone, with nothing of ours holding the
  file -- and is taken to be a scanner of new files; a file made by a rename was never refused. The
  `Vfs` contract therefore promises that a published file can be replaced by a `rename` as soon as
  `publish_new` returns: canopy publishes its marker with `publish_new` and its agent stamps
  replace it.
- **Every open stays at `std`'s default share mode**, which includes `FILE_SHARE_DELETE`, so
  unlink and rename behave as on Unix wherever the process alone holds a file. The exception is
  deliberate: a lock file opened by `open_exclusive` shares nothing, because there the share mode
  is the exclusion.
- A test publishes and immediately replaces a file forty times; with the hard link it failed on
  this host, with the move it has not.

Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), "Moving rather than linking a
publication on Windows".

## Fourth review fixes: markers, plan names, recovery order, foreign roots and MemFs locks, 2026-11-17

> **SUPERSEDED in part, 2026-11-18, by "Fifth review fixes: recovery's commit point, restart
> repair, foreign roots and the marker under abandonment" below.** The file implementation's
> recovery does not record the boot vouched after the count; the shared implementation stores
> `Vouched` before its count; a shared participant checks the reservation on every acquisition
> and attachment, not only at first use; and the marker is published under the guard that
> serializes abandonment. The rest of this decision stands.

A fourth review of the crate, made on 2026-11-17, raised twenty-three findings. Each one that held
was shown by a failing test, or by a model control that a probe catches, before it was fixed; the
work is milestones M22 to M27 in [CHECKLIST.md](CHECKLIST.md). Those that did not hold, or held
only in part, are recorded at the end of this decision.

Creation:

- **Coordination publishes the marker; the maker supplies its bytes.** `ArtifactMaker::marker`
  makes whatever the marker vouches for and returns the marker's bytes, and `make` is never called
  for the marker. Coordination publishes them with `Vfs::publish_new` and syncs the root, in both
  implementations. The marker is therefore complete or absent, which `Created::Existing` and
  `confirm_created` rely on: a maker's own write and sync made it durable but not atomic, and a
  write cut short left a prefix that the next creator found and synced as a finished store.
  Canopy's genesis maker vouches for the store in `marker` and returns the marker's bytes.
- **A plan's names are compared as a filesystem may compare them.** Two names are the same artifact
  when they are equal after Unicode simple uppercase folding, or ASCII folding for a name that is
  not UTF-8, so a plan is portable to a root on a case-insensitive filesystem. A plan is refused
  when an artifact begins with `RESERVED_PREFIX` in any case, checked on the name's bytes so a
  non-UTF-8 suffix does not hide it; when two artifacts fold equal; or when an artifact is a
  publication staging name of another, which a creator would read as that other artifact. A root's
  reading still recognizes coordination's own entries by their exact bytes, since that is how
  coordination spells them.
- **A shared participant abandoned during `reserve` publishes no reservation.** The reservation is
  published under the mutex that serializes `abandon` with creation holds, after checking that the
  hold is still in the table and the participant is alive, as the file implementation's record
  changes are.

Writer exclusion and recovery:

- **The vouched count is recovery's commit point.** [Coordination.tla](../canopy/spec/Coordination.tla)
  makes the state trusted and settled in one step, `RecCount`, and then clears the fence. The file
  implementation publishes the vouched even count, records the boot vouched, then removes the
  fence, and its `trust()` reports the state vouched when the count record is vouched and from this
  boot, whatever the boot record says. A recovery cut short before the count changes nothing a
  reader sees; one cut short after it leaves the state trusted and settled and the fence set, so
  recovery is still required, as the model's state is between `RecCount` and `RecClear`. A sample
  therefore never reads settled beside state that is not trusted. The shared implementation's
  count is a compare-and-swap that changes nothing when it fails, and nothing after it can fail
  before the fence is cleared, so its order is unchanged.
- **Ending a modification that is not open changes nothing.** `NoModificationOpen` is returned
  before the hold's state is touched, so it fences nothing.
- **Discovery on a root with no state directory reports that observation alone.** No holder has
  existed, so there is nothing to probe and no dead holder: it returns at once, untrusted, with no
  fence and no holder that died. Reading the owner afterwards could find a first writer that
  appeared in between and report it dead.
- **No participant uses a root reserved for another implementation.** The file implementation
  checks the reservation's implementation when it takes writer exclusion and when it attaches, as
  the shared one does at first use; an unreserved root is allowed, for the creation path.
  `SharedState::open`, which skips that check, is crate-private.

Identities and durability of the shared state:

- **File registry tokens are never reused within a process.** The count in a token is advanced by
  a checked read-modify-write, and running out is `Exhausted`.
- **The shared state directory's name is durable before any identity is issued from it.** Opening
  always syncs the root after ensuring `.coord-shared` exists, so a retry after an opener that died
  before syncing does not rely on an entry a power loss could remove.
- **The epoch record is replaced whole.** It is written to a staging file, synced, renamed into
  place and the directory synced, so a write cut short leaves the previous epoch, never a torn one
  that every later participant would refuse.
- **A first sample may initialize the shared state.** The sampling contract states the exception:
  a participant's first use initializes the state it samples, which can wait for another
  participant's initialization, never for a writer. Every later sample is one atomic operation.

Notification and fault injection:

- **A burst of due notifications costs linear work under the poller's mutex.** Both pollers turn
  the ringing and quiet identifiers into sets before taking the mutex.
- **Matchers run with nothing locked.** A predicate may use the decorator it is installed in; only
  the chosen rule's call count is taken under the lock.
- **`When::OnCall(k)` fires on call `k` only**, including `u32::MAX`; the call count saturates, so a
  call past the last countable one is no call `k` names.

The backends:

- **`MemFs::truncate` to a length no buffer can hold is refused** with `InvalidInput`, and the file
  is unchanged.
- **`OsFs::metadata` reports a directory's length as zero**, as `Metadata::len` specifies.
- **On Unix, `publish_new` fails if the staging name cannot be removed**, since the file would
  otherwise have two names. The contract states that a failure other than `AlreadyExists` may leave
  the file published whole, as a crash after its publication would.
- **A `MemFs` lock belongs to the file it was taken on.** Holds are keyed by the file's identity,
  so a lock follows its file through a rename, and two names of one file share its locks. A lock on
  a file that a crash erased no longer blocks a file made later at that name.
- **A `MemFs` rename between two names of one file changes nothing**, as POSIX `rename` specifies.
  A crash can leave two durable names for one inode (see "MemFs durability reference and replay").
- **The durability reference marks a removed directory's path replaced.**
  [MemFsDurability.tla](spec/MemFsDurability.tla) gives a directory recreated where one was removed
  no base in the durable tree, so a sync of its parent followed by a crash does not restore the
  removed directory's children. The scenario `remove-tree-recreate-in-place` witnesses it, the
  control `removal-keeps-the-path` and its probe fail without it, and
  [mem_model.rs](src/tests/mem_model.rs) replays it through `MemFs`.

### Findings that did not hold, or held only in part

- **Relaxed counters.** The staging-name counters of `publish_new` and of the file implementation's
  records, and the count in a registry token, only hand out distinct values. Every ordering gives a read-modify-write on one location a total order,
  which is all uniqueness needs, and none of them publishes other memory. The acquire-release
  convention is for atomics that order other data, so `Relaxed` stays.
- **Liveness in `begin_conversion` and `admit_readers`.** The review reported success paths without
  a liveness check. `begin_conversion` releases the attachment's locks and returns a conversion
  whose first poll checks liveness before reporting any lock held. `admit_readers` only releases the
  gate, which an abandonment has already released, so success reports nothing false. The cached
  answers of `gate` and `tiers` did need the check, and have it.
- **A first sample can wait.** The review read it as a broken promise. Initializing eagerly was
  rejected, because a shared participant then touches the root before it refuses a reservation of
  another implementation; the contract instead states the first-use exception above.
- **A partial marker.** The review proposed that coordination wait for the creation exclusion
  before trusting a marker. The defect was in the maker's contract, which promised durability but
  not atomicity, so the fix moved the marker's publication into coordination and left the reading
  unchanged.
- **MemFs locks across a crash.** A test asserted that a crash keeps a lock on a name whose file
  the crash erased. That followed from keying locks by name, so the test was restated: a lock on a
  file that survives the crash is kept, and one on a file the crash erased blocks nothing.
- **An atomic no-replace rename on Unix.** The review suggested one for `publish_new`. It is not
  portable to every filesystem a root may live on (see "A publication is a move on Windows"), so the
  link stays and a failure to remove the staging name fails the publication.

Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), "The fourth 2026-11-17 review".

## Fifth review fixes: recovery's commit point, restart repair, foreign roots and the marker under abandonment, 2026-11-18

A fifth review of the crate, made on 2026-11-18, raised ten findings and two suspicions, five of
them against fixes of the fourth. Each one that held was shown by a failing test before it was
fixed, except where noted; the work is milestones M28 to M31 in [CHECKLIST.md](CHECKLIST.md). What
did not hold, or held in part, is recorded at the end.

Writer exclusion and recovery:

- **Recovery is two steps, as [Coordination.tla](../canopy/spec/Coordination.tla)'s `RecCount`
  and `RecClear` are.** The file implementation publishes the vouched even count, which makes the
  state trusted and settled together, then removes the fence. It does not record the boot vouched:
  a step after the count that could fail on a root with no fence would leave trusted, unfenced
  state behind an error that says recovery is still required. A failure to remove the fence marks
  the hold fenced, so its release leaves the fence.
- **The shared implementation stores `Vouched` before its count.** A sample that reads the even
  count synchronizes with its compare-and-swap and so sees the state vouched; a death between the
  two leaves vouched state with an odd count, which the contract allows and which the dead
  holder's ownership still fences. A failed compare-and-swap restores the previous `Vouched`.
- **An acquisition repairs state from another boot resumably.** It reads the generation, writes it
  advanced, and only then remakes the boot record for this boot. Until the generation is readable
  the record from another boot is what lets the next acquisition read an unreadable generation as
  zero; remade before it, an acquisition cut short would leave every later one refusing the
  generation as malformed for the rest of the boot.
- **A shared participant reads the reservation on every acquisition and attachment**, as the file
  participant does, so one that used a root before another implementation reserved it is refused
  after.

Creation:

- **The marker is published under the guard that serializes abandonment.** `make_artifacts` takes
  the publication from its implementation: the file implementation publishes through `publishing`,
  which `abandon` waits for, and the shared one under the creation table `abandon` clears, with the
  creation's hold still in it. No marker is published once `abandon` has returned, as no
  reservation is.
- **Plan names fold by the Unicode simple uppercase mapping.** `char::to_uppercase` is the full
  mapping; where it is several characters the simple mapping is the character itself, except for
  the Greek letters with ypogegrammeni, which map to their titlecase letters. `simple_uppercase`
  supplies those, so every pair a case-insensitive filesystem compares equal is a duplicate.

The backends:

- **`metadata` and `exists` follow symbolic links; `read_dir` does not.** The `Vfs` contract
  states it: every path operation resolves a link, so a store root that is a link to a directory is
  a directory, while a listing names entries without resolving them and reports a link as
  `FileType::Other`. A Unix test pins both.
- **`OsFs::read` and `read_at` of a directory fail as `IsADirectory` on Windows**, where opening a
  directory as a file is refused with access denied; the contract names the kind, and the
  conformance case no longer accepts `PermissionDenied`.
- **A Linux range lock converts its offsets checked.** `LockSpan` bounds a range to `i64::MAX`,
  which a 64-bit `off_t` holds; a platform with a narrower one refuses the lock rather than locking
  other bytes.
- **`MemFs` drops a replacement mark once the path's absence is durable.** A directory sync that
  makes a child durably absent removes the marks at and beneath it, unless a carried snapshot is
  there, so creating and removing names does not accumulate marks.
- **A `MemFs` read refused by a lock copies nothing.**

### Findings that did not hold, or held only in part

- **`metadata` should describe a link itself.** The review read the entry taxonomy as requiring
  it. Canopy's `Store::init` asks `metadata` whether a root is a directory, and a root reached
  through a link must be one, so the contract was made explicit instead of changed.
- **Swapping the shared stores only reverses the discrepancy.** It reverses it to vouched state
  with an odd count, which the sampling contract permits: an odd count is never mistaken for a
  settled one.
- **A shared participant can still be granted exclusion on a root another implementation reserves
  while it holds exclusion.** Implementations share no lock by design (see "Creating a root
  through the file-based implementation"), so no check made by one can serialize with another's
  reservation. A root's implementation is settled by its reservation before operational use:
  canopy confirms creation before it acquires. Checking on every call closes the case the review
  found, a participant that used the root before the reservation existed.
- **No deterministic test pauses between the shared recovery's two stores.** The step hook's
  vocabulary has no step there, and adding one is a change to an interface agreed with the
  engineer; the order is argued above instead.

Rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), "The fifth review, 2026-11-18".
