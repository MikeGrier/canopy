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

[^ac-1]: `AC-1` in [CHECKLIST.md](CHECKLIST.md#ac-1)

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

[^ac-3]: `AC-3` in [CHECKLIST.md](CHECKLIST.md#ac-3)

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

[^ac-2]: `AC-2` in [CHECKLIST.md](CHECKLIST.md#ac-2)

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
