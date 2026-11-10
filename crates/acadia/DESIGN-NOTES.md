# DESIGN NOTES -- acadia

Crate-local design notes for the filesystem seam (D55/D56). The shared decision register lives
in `DESIGN-NOTES.md` at the root of the originating repository (not imported here); this file
elaborates the decisions that land in this crate rather than re-deciding them. Names such
as `bungo` and `canopy-core` are crates in that repository.

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
`bungo`, `EngineStateWriteFaultVfs` and `ReadHookVfs` in `canopy-core` -- and every one of them spent
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
