# canopy COMPLETED CHECKLIST

Append-only archive of completed work items from [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-08-22 -- LOCK-1: the store-exclusivity decision

### <a id="lock-1"></a>LOCK-1 -- Recorded `D116`: a store root has exactly one writer, enforced by an OS advisory lock taken through the `Vfs` seam. *(completed 2026-08-22 13:10:13 -04:00)*

The decision and its full reasoning live in
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) `D116`, with the canopy-local elaboration in
[DESIGN-NOTES.md](DESIGN-NOTES.md) under the transaction-preconditions heading.

Two things worth retaining that are not the decision itself:

The item was written expecting the `Vfs` question to stay open, and it was closed during the
discussion instead. The argument that decided it was not about tidiness: canopy performs
**all** of its I/O through the `Vfs`, so a lock placed outside the seam would be invisible to
fault injection, and the unacquirable-lock branch is exactly the one no test reaches by
accident. That is why `LOCK-2` carries an explicit obligation to give `FaultyFs` a matching
`FaultOp` -- shipping the seam without it would forfeit the reason the seam was chosen.

A correction was folded in along the way. gaffer's `spill_lock` had been described earlier in
the discussion as a weaker version of this lock whose posture merely needed hardening. It is
not: it is an anti-deletion interlock (a Windows share-mode handle that makes `remove_dir_all`
fail while an engine is live), it attempts no writer exclusion, it is a no-op on POSIX, and it
bypasses the `Vfs`. There was nothing there to promote.

## Moved 2026-08-22 -- LOCK-2: the lock operation on the Vfs seam

### <a id="lock-2"></a>LOCK-2 -- Added `Vfs::lock_exclusive` (defaulted to `Unsupported`), the `VfsLock` guard, `VfsErrorKind::Locked`, `OsFs`/`MemFs` implementations, and `FaultOp::LockExclusive`. *(completed 2026-08-22 13:35:00 -04:00)*

Shape of what landed, beyond `D116`'s statement of it:

`VfsLock` is a **method-less** trait. A lock's entire behaviour is its lifetime, and there is
deliberately no `release()` -- an owner that must remember to call one is an owner that can
forget, and the case the lock exists for is the owner that runs no cleanup at all because its
process died.

`OsFs` needs no advisory-lock call on Windows: `share_mode(0)` makes the exclusion *be* the
open, so acquisition cannot race. POSIX needs a second `flock` step, which is why the two
implementations do not look alike. Both open with `truncate(false)` -- the handle is wanted
for its exclusivity alone, and the bytes belong to whoever holds it.

`MemFs`'s registry lives **outside** `Inner`, so `simulate_crash` does not disturb it: that
call models losing unfsync'd storage, not losing the process, and a lock is a property of its
holder rather than of the tree.

Two tests were written specifically to stop the design from silently decaying rather than to
cover a code path. `an_unacquirable_lock_is_injectable` is the witness for *why* the lock went
into the seam at all -- if that stops being possible, the argument for the design is gone.
`the_decorator_forwards_locking_to_its_backend` guards the trap that the defaulted
`Unsupported` creates: a wrapper that forgets to forward silently makes a capable backend
unlockable, and nothing else would notice.

A gap was found and queued rather than papered over: the POSIX branch has never been
compiled, because the pinned toolchain refuses `rustup target add` and no in-repo pipeline
builds Unix. See `LOCK-6`.

## Moved 2026-08-22 -- LOCK-3: the store takes its root's lock

### <a id="lock-3"></a>LOCK-3 -- `Store::init`/`open` take the root lock and hold it for the handle's life; contention is the named `StoreError::RootInUse`. *(completed 2026-08-22 14:10:00 -04:00)*

Two things the item did not anticipate, both worth keeping.

**"Release on `Store` drop" is really "on the last clone's drop".** `Store` is
`#[derive(Clone)]` and `WriteMonitor` owns a clone, so the clones are *one* writer rather
than several. The guard is therefore an `Arc<dyn VfsLock>`; re-acquiring per clone would
deadlock the store against itself. There is no `Drop` impl -- dropping the `Arc` is the
release.

**The two entry points lock at different moments, and the difference is load-bearing.**
`init` locks after its shape checks but before the first write, because two concurrent
`init`s can both pass the emptiness scan -- the lock, not the scan, is what stops the loser.
`open` locks after validation, which is read-only, so a directory that turns out not to be a
store never has a lock file dropped into it; `recover()` then runs under the lock, since it
replays the log and writes.

**The test fallout was the interesting part.** 18 tests failed, every one of them reopening
a store while still holding the first handle. None of them were modelling a restart, and
none could have caught a recovery path that depended on state the old handle still held. The
fix was to make the release explicit rather than to weaken the lock -- and in the CHAOS
harness, to make it *unforgettable*: `Chaos::reopen` now consumes the previous handle
(`impl Into<Option<Store>>`), so the type system enforces at 22 call sites what a `drop`
would have relied on someone remembering. `None` is reserved for the two sites that reopen
after a *failed* open, which returned no handle to release.

One test now deliberately keeps a second store open across a reopen: bilbo's
`two_engine_local_stores_are_wholly_independent` holds `first` while restarting `second`,
because a different root is a different lock -- which is exactly the independence it exists
to assert.

## Moved 2026-08-22 -- LOCK-4 and LOCK-6: the lock is tested, on both platforms

### <a id="lock-4"></a>LOCK-4 -- 9 unit tests over `MemFs` plus 5 cross-process tests over `OsFs`, including a holder killed outright so only the kernel can release its lock. *(completed 2026-08-22 14:35:00 -04:00)*

The cross-process half needed a real `[[bin]]` (`canopy-lock-holder`), because the property
under test is what the **operating system** does when a lock's owner dies having run no
destructor. A thread cannot witness that, and only a `[[bin]]` gets a `CARGO_BIN_EXE_*` path
for an integration test to spawn. The fixture opens the root, prints one `locked` line, and
blocks forever; the parent kills it with `TerminateProcess`/`SIGKILL`, so nothing in the
child releases anything.

`a_killed_holder_releases_the_root` asserts `RootInUse` **before** killing, so a pass can
never mean "the child never acquired anything" -- the failure mode that would have made the
whole suite decorative.

**Revert-probe.** `lock_root` was temporarily pointed at a per-process, per-call unique path,
which is a faithful simulation of no locking at all. 3 of 5 cross-process tests and 3 of 9
unit tests failed. The ones that still passed are the negative controls -- different roots do
not exclude each other, a released root is writable again -- which *should* pass either way,
so the signature is correct rather than merely green.

The unit tests also pin two things the item did not ask for but that the implementation made
possible to get wrong: a `Store` clone keeps the root held (releasing on the first drop would
hand the root away while a `WriteMonitor` still used it), and a refused `open` drops no lock
file into a directory that turned out not to be a store.

### <a id="lock-6"></a>LOCK-6 -- The whole `canopy` and `acadia` suites, including every lock test, execute and pass on Linux as well as Windows. *(completed 2026-08-22 14:35:00 -04:00)*

Closed by running `LOCK-4`'s suite under WSL2: 245 unit + 5 cross-process canopy tests and 55
acadia tests pass on a real Linux kernel, so the POSIX `flock` path is exercised rather
than merely compiled -- including the killed-holder case, where `SIGKILL` leaves the release
entirely to the kernel.

Two notes worth keeping. `flock` was the right primitive and not only for release-on-death:
its locks attach to the *open file description*, so same-process contention genuinely fails,
where `fcntl` record locks are per-process and would have made those tests pass while the
exclusion was fictional. And the earlier claim that this was blocked was simply wrong --
`rustup` is not this workspace's toolchain manager, `msrustup` is.

## Moved 2026-08-22 -- LOCK-5: the guarantee is stated, and the handoff corrected

### <a id="lock-5"></a>LOCK-5 -- Stated the one-writer guarantee in [DESIGN-NOTES.md](DESIGN-NOTES.md) as something consumers may rely on, and queued the corrected gaffer handoff. *(completed 2026-08-22 14:55:00 -04:00)*

The consumer-facing section says what the guarantee *is* and, as importantly, what it is not:
released by the last handle rather than the first, released when the owning process dies by any
means, never a lease or a queue (acquisition fails rather than waits), and scoped to the
filesystem handed in -- so a green `MemFs` test is not evidence about processes. It also states
that the `.lock` file is **not a signal**: it outlives the lock, is never reaped, and is wrong to
read in either direction.

**A correction was needed, and it matters.** This item's own handoff text -- written twice, in
`LOCK-5` and in the `PLANS.md` row -- said gaffer should "retire its bespoke `spill_lock` in
favour of the store's guarantee". That is wrong on two counts. `spill_lock` is an
**anti-deletion** interlock (a Windows share-mode handle that makes `remove_dir_all` fail while an
engine is live), not writer exclusion; and gaffer's spill root is **not a canopy store**, so `D116`
does not reach it and cannot be adopted in its place. Following the handoff as written would have
removed a working guard and replaced it with a guarantee that does not apply.

What `D116` actually gave gaffer is the *primitive* -- `Vfs::lock_exclusive`, now proven on both
platforms -- which unblocks the genuinely outstanding debt: the POSIX counterpart to the
Windows-only interlock, which gaffer's DESIGN-NOTES called "a separate, later decision" and which
no checklist had ever queued. That is now
[CHECKLIST-engine-operability.md](../../CHECKLIST-engine-operability.md) `EO-15`, in a new `EO-M4`
milestone rather than the parked section, since nothing blocks it. It is deliberately framed to
allow the honest answer that POSIX cannot offer the property at all -- delete-on-last-close means
an advisory lock only helps against deleters coded to check it -- rather than forcing an
implementation that would imply a guarantee it could not keep.

## Moved 2026-09-01 -- BM-M1: mutation regression coverage

### <a id="bm-m11"></a>BM-M1.1 -- Added focused unit tests for all observable regressions reported by the latest `canopy` mutation run, including API formatting and sources, lifecycle shutdown, quorum and recovery boundaries, durability sequencing, and genesis validation; reran the package mutation suite and classified the survivors. *(completed 2026-09-01 17:22:12 -04:00)*

The initial run reported 58 missed mutants. The expanded suite reduced a full 349-mutant rerun to
two missed arithmetic mutations in `group_by_transaction`; a focused follow-up test covering a
transaction first seen through `prepare` then caught both. The final targeted verification tested
all five mutants in that function: four were caught and one was unviable. The package's 282 unit
tests pass, and `cargo clippy --package canopy --all-targets` is clean.

## Moved 2026-09-02 -- BG-1: the `NodePath` value type

### <a id="bg-1"></a>BG-1 -- Add a `NodePath` value type: an ordered sequence of validated components, with accessors for its components, its parent, and its last component. *(completed 2026-09-02 21:19:16 -04:00)*

The addressing primitive for the hierarchical namespace `D222` restores. Landed in
[name.rs](src/name.rs), additive: nothing uses it yet, and `BG-2` keys the store on it.

**Bounds settled with the maintainer during implementation**, each with a distinct job: a component
is 64 characters, a path is at most 16 components deep, and the rendered path is at most 512
characters. The last is the one a per-component limit alone would miss -- the on-disk path is the
store root plus a partition plus this path plus a document name, and the classic Windows limit is 260
for the whole thing, which canopy cannot check because it never sees the store root. The component
limit is sized so a SHA-256 rendered as hex fits **exactly** (bag-end's longest minted identifier, the
`D4` idempotency key); a test pins that so the limit is not lowered without meeting it.

**`DocumentName::MAX_LEN` and `NodeName::MAX_LEN` were narrowed from 255 to 64 to match**, per the
maintainer's "one consistent limit for every name canopy validates". `NodeName`'s narrowing was not
cosmetic: `From<NodeName> for NodePath` is the bridge that lets `BG-2` key the store on paths without
moving every caller in the same change, and it is infallible only while the two limits agree -- at
255 a legal name could be an illegal component and the conversion would have panicked. A test asserts
the agreement rather than trusting it. `Nonce::MAX_LEN` follows `NodeName`'s and narrows with it.

**Refusals are per-component and carry the component's index**, so a caller learns *which* tier is
wrong rather than only that the path is. `.` and `..` are rejected as their own `RelativeComponent`
error even though the character rule already excludes them: a caller who writes `../SIBLING` has
assumed traversal exists, and "invalid character '.'" would send them hunting a quoting bug instead.

`Ord` is component-wise, so sorting groups siblings under their parent and orders parents before
children -- what makes an enumeration's output stable and walk-order-independent. `contains` is
likewise component-wise rather than a string prefix test, so `TICKETS` is not an ancestor of
`TICKETSX`; both are pinned by tests.

31 tests in [src/tests/node_path.rs](src/tests/node_path.rs). One found a real defect in its own
first draft: the total-length test built 32 components and tripped the depth limit instead, so it was
rewritten to sit at exactly `MAX_LEN` within the depth limit and assert that one more character is
refused -- which pins the boundary rather than merely staying under it. 313 canopy lib tests and all
integration suites pass; `cargo fmt --check` and `cargo clippy --all-targets` clean across the
workspace.

## Moved 2026-09-02 -- BG-2: the store keyed on paths, sharding retired

### <a id="bg-2"></a>BG-2 -- Key the store on `NodePath` and retire sharding. *(completed 2026-09-02 21:57:56 -04:00)*

The namespace `D40`/`D54` specified, twelve decisions late. Paths now map to one directory per
component (`p{i}/<c1>/.../<cn>/<document>`); `ShardDepth`, the `.shire` marker's `shard_depth`
field, `NodeName::relative_path` and `NodeName::shard_key` are gone. canopy no longer derives
structure from names -- it stores the structure it is given.

**`GENESIS_FORMAT_VERSION` moves to 2**, and a v1 store is refused rather than reinterpreted: its
directories carry a synthesized shard tier this build would read as an ordinary path component, so
every node would appear one level deeper than it is. A test pins that refusal specifically rather
than leaving it to the generic version check.

**The one-namespace rule is enforced before staging**, so a refusal costs no fsync and creates no
directory. Two named errors -- `NameIsChildNode` (a document cannot take a name a child node holds)
and `PathIsDocument` (a path cannot descend through a document). The `Vfs` would fail both anyway,
but as generic I/O errors; naming the conflict is the point, since a caller who has collided with
the other namespace has made a modelling mistake rather than hit a disk problem. `PathIsDocument`
reports the **shallowest** offending prefix, because a deeper report would name a directory that
only failed because of this one.

**Enumeration became a real recursive walk**, which is what the whole change was for: `PWQ-6`'s
rebuild needs to list a prefix's contents without traversing every ticket in the store. The walk
carries a component stack rather than rebuilding a path per directory, descends into directories
and records files -- so an entry's *kind* decides which namespace it belongs to and the traversal
never guesses -- and skips a file at the partition root, which has no node path to belong to.

**Transitional API shape:** the public entry points take `impl Into<NodePath>` so a caller holding
either a `NodeName` or a `NodePath` compiles unchanged, which is what kept the workspace green
without migrating every caller in this item. `BG-4` narrows them to `&NodePath` once `NodeName` is
gone. Three `From` impls make it work: `NodeName`, `&NodeName`, and `&NodePath`.

**Callers that had to move with the types, because nothing else compiles:** the commit log now
records a swap's target as a path string (with a round-trip test for a nested one); the quorum
reader, repair queue, scrub and in-flight records key on paths; and in `bilbo`, the watch registry
is keyed on `NodePath`, `QueryCursor` holds paths, and `TicketId` gained `node_path()` -- one
component today, so the ledger's on-disk layout is unchanged, but moving tickets under a real prefix
later is now a change to that one function rather than to every call site.

**Three existing tests failed and were right to.** Two pinned the `<shard>/<node>` layout, and one
fed `a/b` to the commit log as its example of an invalid node name -- which is now a *valid*
two-component path. That last one is the clearest single illustration of what changed, and its
replacement says so in a comment.

14 new tests in [src/tests/hierarchy.rs](src/tests/hierarchy.rs) covering nesting, sibling paths
whose names prefix one another (`TICKETS` vs `TICKETSX`, the trap a string-prefix layout would fall
into), both collision directions, that a refused write leaves nothing behind, and enumeration over a
tree. 291 canopy lib tests and every integration suite pass; the full workspace builds and tests
clean, `cargo fmt --check` and `cargo clippy --all-targets` clean.

One unrelated flake was seen once during a `--workspace` run and did not reproduce in seven
subsequent runs; recorded in [UNRESOLVED-TEST-FAILURES.md](../../UNRESOLVED-TEST-FAILURES.md)
rather than dismissed.

## Moved 2026-09-02 -- BG-3: the child listing primitive

### <a id="bg-3"></a>BG-3 -- Add a child listing primitive: given a path, return its immediate children, distinguishing documents from child nodes. *(completed 2026-09-02 22:05:00 -04:00)*

`Store::list_children` reads one directory per partition rather than walking the store, so it costs
`O(children)`. This is the primitive `D221` originally asked for as a node-scoped document listing
(`BG-1` in its first framing); with a real hierarchy it needs no special case, because listing a
path's children is simply what a hierarchical store does.

**Documents and child nodes are two separate lists, not one tagged list.** A caller almost always
wants one or the other -- a collection's members, or the tiers beneath a prefix -- and folding them
together would make every caller re-split them. Keeping them apart also puts the one-namespace rule
in the type: a name appears in exactly one of the two, never both. Documents are leaf names
(`DocumentName`); child nodes are **full paths**, so a caller can descend without reassembling
anything.

**An absent path lists empty rather than erroring.** A node is not a durable object in its own right
-- nothing creates one but a write beneath it -- so "no children" and "no such node" are the same
fact, and distinguishing them would be inventing state the store does not keep. This is also what
`BG-6`'s node removal will need to mean by "empty".

Semantics match `list_documents` exactly, deliberately: the union across partitions, sorted output,
dotfiles and malformed entries skipped, presence only. A test asserts the agreement directly --
whatever the full traversal reports at a path must be exactly what the child listing reports as that
path's documents -- so the two primitives cannot drift.

8 tests added to [src/tests/hierarchy.rs](src/tests/hierarchy.rs) (22 in that file now), including
that a listing does not reach into sibling prefixes, that it unions a document in one partition with
a child node in another, and that dotfiles and foreign entries are skipped. Full workspace builds and
tests clean; `cargo fmt --check` and `cargo clippy --all-targets` clean.

## Moved 2026-09-02 -- BG-4: `NodeName` removed, callers on paths (closes BG-M1)

### <a id="bg-4"></a>BG-4 -- Remove `NodeName` and `ShardDepth` from the public surface and migrate every caller to `NodePath`. *(completed 2026-09-02 22:19:24 -04:00)*

`NodeName`, `NodeNameError` and the last of `ShardDepth` are gone; `NodePath` is the only way to
address a node. `From<&NodePath> for NodePath` survives the two `NodeName` conversions it sat beside,
because it is what lets the store's `impl Into<NodePath>` signatures accept a borrow.

**A mechanical conversion, as planned:** each reserved name became a one-component path, so no
collection's on-disk layout moved. The migration funnelled through one helper per module --
`TicketId::node_path`, `singletons::node_name`, `local_node`, `idempotency_node`,
`pending_work_node`, and `gaffer`'s `node_for` -- which is why a type used in ~40 places took a
handful of edits. `TicketId::node_name` was **replaced** rather than kept beside `node_path`, since
carrying two spellings of one address is exactly the drift this item exists to end. Adopting real
prefixes stays per-collection work; `PWQ-5.1` is the first.

**A mistake of mine, found and repaired here rather than shipped.** While stripping the shard tests
from `names.rs` during `BG-2`, I truncated the file at the `ShardDepth` section marker -- which also
deleted the `DocumentName` and `Nonce` sections that followed it. 17 valid tests were silently lost
and the suite stayed green, because deleted tests do not fail. They are restored from
`eeb7afe:crates/canopy/src/tests/names.rs`, and only the genuinely obsolete `NodeName` section is
gone (its per-component validation is `node_path.rs`'s now). The lesson worth keeping: a
truncate-at-marker edit deletes everything after the marker, not the section it names.

**Verified against the real risk of a sweeping rename**, which is that a blanket replace compiles
while meaning something different. The whole workspace builds in debug and release, every test
passes, and `cargo clippy --all-targets` is clean -- including one warning it raised on a
`.clone().into()` left by the conversion, which was a genuine redundancy once both sides were paths.

One failure in the full-workspace run was the **already-recorded** green-door flake
([UNRESOLVED-TEST-FAILURES.md](../../UNRESOLVED-TEST-FAILURES.md) line 178,
`remove_engine_emits_a_skipped_event_when_no_file_was_present`), not a regression from this item; a
re-run was clean.

**`BG-M1` is complete.** `canopy` is the hierarchical path store `D40`/`D54` specified: paths in,
one directory per component on disk, one namespace per level, and a child listing that costs what
it lists. `BG-M2` (document and node removal) remains.

## Moved 2026-09-02 -- BG-5: transactional document removal

### <a id="bg-5"></a>BG-5 -- Add document removal as a transactional operation. *(completed 2026-09-02 22:57:14 -04:00)*

`Swap` gained an [`Effect`](src/transaction.rs) -- `Write(body)` or `Remove` -- so a removal is an
ordinary swap-set member. Writes and removals share one type because a transaction must mix them:
`D216` needs a queue entry's removal to commit with whatever else the action changed, which is
`D53`'s whole purpose. Constructors are `Swap::remove` and `Swap::remove_conditional`;
`Swap::body()` became `Option<&DocumentBody>`.

**A removal turned out to be simpler than a write, not harder.** It stages nothing -- there is no
body to place, and the committed document must stay untouched until after the commit point, which
"stage nothing" achieves exactly. Two consequences fall out: a crash before `prepare` needs no
rollback for removals (nothing was staged), and the intent sidecar is unnecessary. That sidecar
exists to disambiguate "temp lost before `prepare` became durable" from "already renamed"; a removal
has no temp, and absent is precisely the state it was asking for however it got there, so there is
no ambiguity to resolve and no `MissingStagedTemp` distinction to make.

**Idempotence is specified, not incidental.** Removing an already-absent document succeeds, because
`D53` recovery rolls a prepared transaction forward by replaying it -- a removal that failed the
second time would turn a crash between `prepare` and `end` into a store that can never finish
recovering. A caller wanting "fail if it is not there" writes `remove_conditional`, whose digest
precondition cannot hold against an absent document and so rejects before the commit point.

**The precondition vocabulary is reused rather than extended**, because the question a removal asks
about current state is exactly the question a write asks. There is deliberately no absence-conditional
removal: "delete it only if it is already gone" is a no-op whenever its precondition holds, so the
API offers no way to write one, and the commit log rejects a record carrying one rather than
replaying a contradiction.

**Two seams beyond the item's letter, each because leaving them would have been silently wrong:**

* `CommitObserver` gained a defaulted `removed(node, document)`. A removal has no body to hand
  `committed`, and skipping the notification would leave an observer reporting a document that no
  longer exists. The default is a no-op so existing observers compile, and its doc says plainly that
  ignoring a removal *is* a choice.
* bilbo's `handle.rs` require-folding assumed the swap it folds onto has a body. Made total across
  all four combinations rather than unwrapped -- a `Digest` require folded onto a removal becomes a
  conditional removal. Nothing in bilbo builds a removal yet (`PWQ-5.1` is first), so this is the
  landmine removed rather than a behaviour change.

`remove_document` was added as the LAY-2 per-partition sibling of `write_document`, since the
transactional path needed the primitive anyway and the layer's shape should stay symmetric.

The commit log's `WireSwap` carries `remove` and an optional `body` as separate fields rather than a
tagged union, so a record written before removals existed still reads: an absent `remove` is a write,
which every existing record is.

23 tests in [src/tests/removal.rs](src/tests/removal.rs) covering the effect vocabulary, all-partition
deletion, idempotence, sibling and node survival, mixed transactions (including that a rejected
precondition anywhere leaves the removal undone), the three precondition outcomes, the full `D53`
crash matrix (prepared rolls forward, unprepared rolls back, half-applied completes), commit-log
round-trip, and the per-partition primitive.

**Verified by revert-probe rather than trusted**: every test passed on the first run, so
`remove_committed` was temporarily neutered to return success without deleting -- 10 of the 23 failed,
confirming the tests bite. 335 canopy lib tests, the full workspace suite, debug and release builds,
`cargo fmt --check` and `cargo clippy --all-targets` all clean.

## Moved 2026-09-02 -- BG-6: transactional node removal (closes BG-M2)

### <a id="bg-6"></a>BG-6 -- Node removal with `rmdir` semantics, as a member of a swap set rather than a standalone operation, refused unless the node is empty *after* the transaction's own effects. *(completed 2026-09-02 23:29:00 -04:00)*

`BG-6` explicitly asked for a decision on whether node removal joins a swap set or stands alone,
and for that decision to be documented rather than assumed. It joins the swap set. The reasoning
and its one genuinely novel consequence are recorded here.

**The driving case decided the shape.** `D216`'s queue must, in one atomic step, remove a pending
entry's last document *and* the node that held it. A standalone `remove_node` cannot express that:
the caller would have to remove the document, then remove the node, and a crash between the two
leaves an empty node behind -- a leak that grows without bound over the queue's lifetime, and one
no reader could distinguish from a node whose entry is mid-write.

**Emptiness is therefore a post-transaction precondition, and that is new.** Every other `D53`
precondition asks about state *before* the transaction: this document holds that digest, that
document is absent. Emptiness cannot, because the driving case makes the node non-empty at the
moment the transaction begins and empty only once its own effects land. Phase 1 therefore computes
the node's contents *as the set will leave them* -- subtracting the documents this set removes at
that node, adding the ones it writes there -- and refuses only if something would still remain.

The addition side of that arithmetic is, today, unreachable: a set that writes at or beneath a node
it removes is refused outright at construction (`SwapSetError::WriteBeneathRemovedNode`) as
contradictory, since neither ordering gives the caller what they asked for. It is kept anyway as a
total accounting rather than an assumption, so the rule stays correct if that construction check is
ever relaxed.

**Soundness rests on `D116`.** A post-transaction check is only sound if nothing can change the
node between the phase-1 check and the phase-4 apply. `D116`'s single writer per store root
guarantees exactly that, which is why this shape is available to `canopy` and would not be to a
multi-writer store.

**The refusal carries what would remain**, split into documents and child nodes
(`RejectedPrecondition::NodeNotEmpty`). "Not empty" alone leaves a caller guessing which of two
quite different mistakes they made -- a sibling document they forgot, or a child node they did not
know existed. Because a node removal names no document, `TransactionOutcome::Rejected.document`
became `Option<String>`; `bilbo`'s require-folding treats a `None` as what it is -- never a
require's target, since a require always names a document -- and falls through to its optimistic
retry.

**The apply is non-recursive by construction, not by intent.** `apply_node_removal` uses
`remove_dir`, which the OS itself refuses on a non-empty directory. Even if the phase-1 check were
somehow wrong, `rm -r` is unreachable rather than merely unrequested. Removing an absent node is
success, for the same reason as `BG-5`: `D53` recovery replays a prepared transaction, so making the
second application an error would make a crash between apply and `end` unrecoverable.

**The commit log had to carry the removals**, or the whole transactional choice would be defeated:
past the commit point the removal is promised, and only the log can keep that promise across a
crash. `WireRecord::Prepare` gained a `remove_nodes: Vec<String>` field, defaulted and omitted when
empty, so a record written before node removals existed still reads as the no-removals transaction
it was.

28 tests in [src/tests/node_removal.rs](src/tests/node_removal.rs) covering the construction rules
(duplicate removal, write at and beneath a removed node, the sibling `RECORDS`-vs-`RECORD` case that
a string-prefix test would get wrong, and the document-removal-at-a-removed-node case that must be
allowed), all-partition removal, the driving last-document-plus-node case, both refusal reasons with
their payloads, idempotence, all-or-nothing across both effect kinds, sibling and parent survival,
re-creation after removal, the full `D53` crash matrix (prepared rolls forward, unprepared rolls
back, half-applied completes), and the commit log's durable shape including that an empty removal
list is not written.

**Verified by revert-probe rather than trusted**: all 28 passed on the first run, so three separate
neuterings were applied in turn -- a no-op `apply_node_removal` (8 failed), an emptiness check that
never refuses (5 failed), and a commit log that drops the removals (4 failed) -- confirming each
limb of the feature is independently pinned. 363 canopy lib tests, the full workspace suite, debug
and release builds, `cargo fmt` and `cargo clippy --all-targets` all clean.

## Moved 2026-11-10 -- Formal model tooling (M0 of [CHECKLIST.md](CHECKLIST.md))

### <a id="fm-01"></a>FM-0.1 -- JDK and pinned tla2tools.jar installed; versions and commands recorded in the repository instructions. *(completed 2026-11-10 08:35:37 -05:00)*

Microsoft Build of OpenJDK 21 via winget; `tla2tools.jar` v1.8.0 at
`%USERPROFILE%\.tla\1.8.0\`, its SHA-256 checked against GitHub's published digest. Recorded
under "TLA+ / TLC" in [copilot-instructions.md](../../.github/copilot-instructions.md). WSL has
no JDK, so TLC runs on the Windows host only.

### <a id="fm-02"></a>FM-0.2 -- `tools/run-tlc.ps1` model-checks a spec and exits non-zero on any violation. *(completed 2026-11-10 08:35:37 -05:00)*

[run-tlc.ps1](../../tools/run-tlc.ps1). Shown to gate both ways on a scratch spec: a violated
invariant exits 12, a holding one exits 0. TLC's generated trace-explorer spec is suppressed so
nothing is written beside a checked-in spec.

## Moved 2026-11-10 -- First spec slice (M1 of [CHECKLIST.md](CHECKLIST.md))

### <a id="fm-11"></a>FM-1.1 -- `spec/PointerPublication.tla` models one document's non-transactional write, collection, crash and reopen per `Vfs` call. *(completed 2026-11-10 08:48:39 -05:00)*

[PointerPublication.tla](spec/PointerPublication.tla) with
[PointerPublication.cfg](spec/PointerPublication.cfg) (three pointer replicas, two blob
replicas, three versions, two writes, one crash). Every filesystem step may fail instead
(`Fail`), a crash may fall between any two steps, and the starting transaction stays journalled
so every reopen re-runs its collection. The configuration checks exhaustively with no violation.

### <a id="fm-12"></a>FM-1.2 -- Safety invariants for the durable and live pointer majorities, shown to bite on the `D1` defect. *(completed 2026-11-10 08:48:39 -05:00)*

`DurablePointerHasBlob` and `LivePointerHasBlob`. With the `RV7-1` sync disabled
([PointerPublication-NoCollectSync.cfg](spec/PointerPublication-NoCollectSync.cfg)) TLC finds
`D1` unaided and with three pointer replicas rather than the one the review used: a write
renames two replicas but fails before the second parent sync, the live majority says the new
version while the durable one says the old, a warm reopen collects the old blob from every
replica, and the durable majority then names nothing.

### <a id="fm-13"></a>FM-1.3 -- The `E1` property, violated by the current protocol as reviewed. *(completed 2026-11-10 08:48:39 -05:00)*

`IdleImpliesSettled`: with no operation in progress the namespace has a majority, a version or
absence. Expressed as an invariant rather than a temporal property because "no writer running"
is a state, so the invariant is the stronger and cheaper form.
[PointerPublication-Settled.cfg](spec/PointerPublication-Settled.cfg) reports it violated with
no crash at all: a write fails after two renames (`[C, C, A]`), the next write reads `C` and
fails after one (`[B, C, A]`). This configuration is the acceptance check for `FM-2`.

## Moved 2026-11-10 -- Deciding the `STRANDED-VOTE` guard (M2 of [CHECKLIST.md](CHECKLIST.md))

### <a id="fm-21"></a>FM-2.1 -- Four candidate guards modelled and checked; resolving before each write and a durable publication record both hold. *(completed 2026-11-10 12:27:58 -05:00)*

The guards are selected by the `Fix` constant in
[PointerPublication.tla](spec/PointerPublication.tla), one configuration each. Resolving only when
the live replicas disagree fails (live unanimity hides a durable split), and a process-memory
fence fails (reopen clears it). The comparison, with costs, is the decision "Guards against
`STRANDED-VOTE`, compared in the model" in [DESIGN-NOTES.md](DESIGN-NOTES.md).

## Moved 2026-11-10 -- Write surface onto `Store` (M2 of [CHECKLIST.md](CHECKLIST.md))

### <a id="ws-1"></a>WS-1 -- `WriteMonitor` inventoried: the writer gate covers everything but the off-latency repair drain, which alone needs a thread. *(completed 2026-11-10 13:13:48 -05:00)*

The capability table and the callers are recorded under "What `WriteMonitor` provides, and what
each part needs" in the decision "The public write surface belongs on `Store`" in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## Moved 2026-11-10 -- Model several processes (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="odi-1"></a>ODI-1 -- Writers of different versions: schema versions with one-way exclusive upgrades, attachment in the contract, and a diagnostic registry of attached handles. *(completed 2026-11-10 14:56:41 -05:00)*

Decided in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Several processes may open one store"; the three
ends considered and the reasoning are in
[DESIGN-SESSION-2026-11-10-multi-process.md](design-sessions/DESIGN-SESSION-2026-11-10-multi-process.md).

## Moved 2026-11-10 -- Write surface onto `Store`, continued (M2 of [CHECKLIST.md](CHECKLIST.md))

### <a id="ws-2"></a>WS-2 -- The `Store` surface decided: logged, unlogged, read, management and repair verbs, an optimistic `snapshot`, read-only callback views, and a feed of every write. *(completed 2026-11-10 15:10:02 -05:00)*

Recorded under "Choices made for the surface" in the decision "The public write surface belongs
on `Store`" in [DESIGN-NOTES.md](DESIGN-NOTES.md); the discussion that reached it is in
[DESIGN-SESSION-2026-11-10-multi-process.md](design-sessions/DESIGN-SESSION-2026-11-10-multi-process.md).

### <a id="ws-3"></a>WS-3 -- The decided surface added to `Store`: source-drawn nonces under durable claims, logged verbs, read-only callback views, an optimistic `snapshot`, the outermost-holder log reset, the commit feed and a random source behind `random-nonce`. *(completed 2026-11-10 18:05:29 -05:00)*

The verbs are added beside the ones that take a nonce per call, under interim names (`update`,
`apply`, `write_replicated`, `drain_repair_backlog`), because moving the existing callers is
WS-4's work; WS-4 carries the convergence onto the decided names. The claim mechanics, the
outermost-holder reset, and the correction that a captured clone's write lands at once (the
writer gate is re-entrant per thread) are recorded under "Choices made for the surface" in
[DESIGN-NOTES.md](DESIGN-NOTES.md).

Two behaviours reached the existing verbs and their tests, because they are properties of the
store rather than of the new surface: a `begin` whose transaction id the commit log already
holds is refused for every transaction, so seeding helpers that reused one nonce were given
distinct ones and a reserved-name test empties the log before reusing `CON`; and the apply now
reads the log once before `begin`, so a recovery-read fault test targets the second log read.
A forced collision on each nonce-named artifact -- a held claim, a dead claim, blob staging,
namespace staging, a recorded transaction id, a quarantine name -- is refused with the orphan
left byte-for-byte intact.

### <a id="ws-4"></a>WS-4 -- Every caller moved to the `Store` surface: public writes draw claimed nonces from the source, `init`/`open` take `StoreOptions`, the building blocks are crate-private, and `WriteMonitor` is removed. *(completed 2026-11-10 18:29:09 -05:00)*

The decisions this item settled -- removing the monitor rather than hiding it, and keeping the
explicit-nonce verbs only as crate-private test internals -- are recorded under "Choices made
for the surface" in [DESIGN-NOTES.md](DESIGN-NOTES.md), and the monitor's own decision (ECAT-12)
carries a superseding banner.

The monitor's tests were ported to the `Store` verbs in `src/tests/verbs.rs`, behaviour for
behaviour, except three that tested the actor itself: dropping it, its `Debug` and error
types, and the idle-time repair drain, which the decided surface replaces with the explicit
`repair_reported_damage`. Integration tests that must name a nonce -- to place a fault on a
staging path, or to use a reserved device name -- do it through the store's source; the chaos
harness gained a queue-fed source for that.

Two integration tests needed a populated commit log after a successful write, which the
outermost write now empties: one replays two ended transactions and gets its log by making the
truncation fail, and the recovery-read fault test counts the two log reads a clean apply now
makes. The content-store scale tests drive `ingest_blob` and `retain_blobs` directly, so they
moved into the unit tests; where they should live is WS-5's decision.

### <a id="ws-5"></a>WS-5 -- The content-store scale tests stay unit tests at full scale, with a 2 s budget for their module. *(completed 2026-11-10 20:05:06 -05:00)*

They had run far over the 1 s unit-test budget. Two fixes in acadia removed most of the cost:
`MemFs` directory syncs became proportional to what changed[^mf-1], and each test now opens a
`MemFs::fork` of a store populated once per replica count[^mf-2]. A profile of what remained
found it spread across the ingest path with no hotspot, and the engineer accepted the
remaining overrun with a 2 s budget for this module. The decision is in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "The content-store scale tests are unit tests with a 2 s
budget".

[^mf-1]: `MF-1` in [COMPLETED-CHECKLIST.md](../acadia/COMPLETED-CHECKLIST.md#mf-1)
[^mf-2]: `MF-2` in [COMPLETED-CHECKLIST.md](../acadia/COMPLETED-CHECKLIST.md#mf-2)

## Moved 2026-11-10 -- Model several processes, continued (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="mp-1-1"></a>MP-1.1 -- Processes, death and writer exclusion are modelled in `Coordination.tla`, with the waiting model later steps reuse. *(completed 2026-11-10 20:46:56 -05:00)*

The anchor is hyphenated because `mp-11` is the anchor `MP-11` takes.

[Coordination.tla](spec/Coordination.tla) models processes that attach, request writer
exclusion, write in three phases, release, withdraw, are refused, and die at any step, apart
from a host crash that kills them all and leaves the coordination state untrusted. Writer
exclusion is an OS lock released on death plus an ownership record that survives it; acquiring
is three steps (lock, fence, record), and a holder told that recovery is required writes
nothing. Requests carry their own outcome and there is no queue in the state. Configurations
with two and three processes check the invariants and action properties. Death while the
modification count is being published needs the count and is MP-1.2's.

Why it is believed correct: each rule has a control that reverses it and must fail, and the
three controls fail on the property that guards that rule. Five witness configurations show
that a holder dying before its first effect, mid-write and after its last effect, a restart
alone, and refused, withdrawn and dead requests are all reachable, so no property holds for want
of a state to check. A probe that broke the contract's rules one at a time in scratch copies of
the spec was caught by the intended property each time, including a separate case that only
`UnresolvedWorkIsNeverInvisible` can catch. The scenarios in the design note were each confirmed
as behaviours of the model.

The work found one requirement the contract text left implicit: a clean release must clear the
ownership record before it drops the lock. It is recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Writer exclusion and its ownership record, modelled in
`Coordination.tla`" and queued in MP-5 and MP-8.

### <a id="mp-1-2"></a>MP-1.2 -- The modification count, the fence, the generation and reads modelled in `Coordination.tla`. *(completed 2026-11-10 21:37:03 -05:00)*

[Coordination.tla](spec/Coordination.tla) now brackets each write with the count, published in
two steps so a death or failure between them leaves the previous value; models every way an
operation ends, nested operations, and death at each point of a publication; advances the
generation on each acquisition and ties a holder's cache to it; and has readers in any process
sample, observe a transaction's two swaps in separate steps, sample again and classify with
`Store::undisturbed`. A host crash may roll the count back, and untrusted state reads as odd.

Why it is believed correct: the item's two required controls -- a sample assembled from separate
parts, and a cached sample invalidated late -- each violate the read classification, as do three
more controls for the other rules. Witnesses show each failure path, each death point during a
publication, settled, torn and failed-transaction reads, and caches both kept and reloaded. A
probe broke each new rule in scratch copies of the spec and was caught every time; nine of the
dedicated properties were then checked on their own against the fault meant for them, so none
holds only because a broader property fires first.

The model assumes the contract's ordering rules rather than checking them, since its steps are
sequentially consistent; that and two other implementation requirements are queued in MP-5. The
decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The modification count, the generation and
reads, modelled in `Coordination.tla`".

### <a id="mp-1-3"></a>MP-1.3 -- The admission gate and the attachment tiers modelled in `Coordination.tla`. *(completed 2026-11-10 23:37:55 -05:00)*

[Coordination.tla](spec/Coordination.tla) now has processes attach through the admission gate and
the reader and writer tiers in the contract's order, holding the gate only until they hold the
reader tier, checking the per-layer admission rules under the shared tiers, and being admitted or
refused; upgraders convert by releasing their tiers, taking the gate exclusively, then the tiers
exclusively in order, checking the schemas again, and either giving up or completing. Who holds
what follows from where each process is, so a death releases everything. Waiting handles can give
up, which a constant switches off so that a wait that never ends is a deadlock.

The item took the schema movement as given. The model has one abstract completion step, which
moves a layer to its target and the oldest reader schema with it for an upgrade unsafe for
readers, and records an upgrade in progress if its upgrader dies in the body; the body, intent,
resumable steps and cumulative reader compatibility are MP-2's, and clones and the drain MP-1.4's.

Why it is believed correct: the required control -- an upgrader that waits for the gate while
holding a tier -- deadlocks, as do an upgrade that begins holding writer exclusion and an unsafe
upgrade that takes the writer tier before the reader tier; three further controls break the
gate, the tiers' compatibility and the check's place. Deadlock-free configurations with no
process giving up or dying pass for each mix of upgraders, writers and readers. Witnesses reach
each case of the admission table, each way an upgrade ends and each wait the protocol creates.
A probe that broke each rule in scratch copies of the spec was caught by the intended property
each time, the order property was checked alone against the three deadlock controls, and the two
checks that were not caught were shown to be implied by the gate and the order.

The work found that the contract's upgrade sentence lists the writer tier before the reader tier
and an implementation that follows it deadlocks; the reader tier must come first. It is recorded
in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The admission gate and the attachment tiers, modelled in
`Coordination.tla`" and queued in MP-5.

### <a id="mp-1-4"></a>MP-1.4 -- Clones, leases and the upgrade drain modelled in `Coordination.tla`. *(completed 2026-11-11 02:48:25 -05:00)*

[Coordination.tla](spec/Coordination.tla) now counts the operations in flight on each handle --
its clones' and its own reads and writes -- as leases. An upgrader first drains: it refuses new
operations on every clone and waits for the leases to end while it still holds its tiers, and
only then releases them and asks for the gate. A call made from inside an operation is refused.
A drain can be cancelled, which leaves the handle attached; a conversion cancelled or failing
after the drain, including at the recheck, leaves it detached, from where it attaches again as an
ordinary admission or is dropped. A death ends every lease with its process.

Why it is believed correct: the required control -- an upgrade started from inside an operation,
here a callback during a read -- deadlocks on that read's lease, and four further controls break
the drain's wait, the refusal of new operations, the cancelled drain and the cancelled
conversion. Deadlock-free configurations with no process giving up or dying pass with clones'
operations in flight. Witnesses reach each case, including the call refused from inside a read
and a write and a death ending the leases. A probe that broke each rule in scratch copies of the
spec was caught by the intended property every time; it first found that a property written in
terms of the model's own lease count could not catch a fault in that count, so the two
properties about leases are stated over the raw variables.

The model gives a clone's operation no behaviour but holding a lease, so a wait inside such an
operation is not checked; the model's reason is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md)
"Clones, leases and the upgrade drain, modelled in `Coordination.tla`". The requirement that
the refusal be decided by the caller's own operation context, not by whether the handle has any
lease, is queued in MP-7.

### <a id="mp-1-5"></a>MP-1.5 -- The registry modelled in `Coordination.tla`. *(completed 2026-11-11 03:35:55 -05:00)*

[Coordination.tla](spec/Coordination.tla) now has a registry of entries, each with a claim lock and
a liveness lock that a death releases. A handle registers after its check and before its attach
returns -- claiming an entry, trying its liveness lock, writing its token, and letting go of the
claim lock last -- or attaches without an entry when every one is held, which the registry
records. It unregisters before it lets go of its tiers, including at an upgrade's conversion, and
updates its entry when it starts or stops draining. Listings run on a thread of their own,
visit the entries in turn under each claim lock, report an entry live only if the liveness probe
was refused, and say whether they may be incomplete. Every acquisition of a registry lock is a
try, a death releases the locks and changes no entry, and a host crash discards the registry.

Why it is believed correct: the required control -- a listing that inspects an entry without
claiming it -- reports a dead handle live, and five further controls break the registrant's
claim ordering, the entry's following of the attachment, claiming before writing, probing before
reuse, and the listing's rule for saying it may be incomplete. A configuration with no giving up
or dying and a registry finds no deadlock. Witnesses reach each way a registrant moves on, an
entry reused after its handle died, both kinds of unregistration, each kind of listing report,
and a death at each point of registering. A probe that broke each rule in scratch copies of the
spec was caught by the intended property every time, and the one mutation that stopped clearing
an entry on unregistration was shown to be invisible to every property.

The work found that the contract's "listings then say they may be incomplete" cannot be carried
by the entries: a listing that infers it from what it sees misses a handle that attached
unregistered, and a claim lock held for a moment can send a registrant on while an entry is free.
The registry needs an overflow record that listings read. It is recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "The registry of attached handles, modelled in
`Coordination.tla`" and queued in MP-5.

### <a id="mp-1-6"></a>MP-1.6 -- The creation reservation modelled in `Creation.tla`. *(completed 2026-11-11 07:27:48 -05:00)*

[Creation.tla](spec/Creation.tla) is a model of its own, because creation ends at a marker that
[Coordination.tla](spec/Coordination.tla) begins with and needs a file's existing and its
being durable as separate facts. Creators and openers use one of two implementations whose
creation exclusions do not exclude each other. A creator reads the root, takes its
implementation's exclusion, reads the root again under it, and publishes the reservation with an
exclusive publish-if-absent or, as a resumer, confirms that the marker is absent, the reservation
unchanged and the root unoccupied. It makes the reservation durable, then each artifact durable
before the next, the marker last, and lets go of the exclusion when the marker is durable. A
resumer compares its inputs with the reservation's. A death changes nothing in the root; a host
crash keeps what is durable and keeps or loses each other file on its own. An opener reads the
root and opens it only through the implementation the reservation names.

Why it is believed correct: the required control -- a reservation published in two writes --
lets another creator read a reservation that is not complete, and the other controls break the
reservation's durability before the artifacts, the durability of each artifact before the next,
the exclusivity of the publication, the exclusion a resumer takes, the resumer's second read of
the root, its comparison of inputs, an open's check of the implementation, an open's making the
marker durable, and the refusal of an occupied root. Configurations with no giving up or dying and
with failures find no deadlock, and witnesses reach each outcome of a creator and an opener, a
resume after a death and after a host crash, and each state a crash can leave. A probe that
broke the resumer's syncing, the crash's keeping of what is durable and the occupied-root checks
in scratch copies of the spec was caught by the intended property every time, and removing the
opener's exclusion was shown to be invisible to every property.

The work found that an opener must make the marker durable before it relies on it, since a
creator that dies between publishing the marker and making it durable leaves it visible and
exposed to a host crash (`UNSYNCED-MARKER`). It is recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Creating a store root, modelled in `Creation.tla`" and
queued in MP-5; the requirement that publish-if-absent leave a complete file or none even when
power is lost is queued in AC-1.

### <a id="mp-2-1"></a>MP-2.1 -- Transactions and the commit log modelled in `Coordination.tla`. *(completed 2026-11-11 08:10:04 -05:00)*

[Coordination.tla](spec/Coordination.tla) has a switch, `Txn`, that makes a write a transaction
over a commit log, staging and the namespace: `begin`, ingest, `prepare` (the commit point), the
two swaps, `end` and the sweep of the staging, with the count bracketing the whole of it. A
failure before `prepare` is rolled back -- the staging swept and the log reset, with nothing
visible in the namespace -- and one at or after it, including the failure of the prepare append
itself, whose record may or may not have been written, sets the fence and leaves the count odd and
the log unresolved. The log is reset only by a holder with nothing in flight, and only a settled
one. With the switch off a write is the abstract operation the earlier items use, and every
earlier configuration is unchanged.

Why it is believed correct: the required controls -- a reset made while a transaction is in
flight, and one made by an acquirer told recovery is required -- each discard a prepared
transaction that only the log lets recovery finish, and four further controls break the commit
point's ordering: a swap before `prepare`, a `prepare` before ingest, an `end` before the swaps,
and a failure after `prepare` handled as a rollback. Witnesses reach each point at which a
transaction can fail, each point at which a holder can die with another process attached and the
next acquirer be told, and a host crash leaving a prepared transaction. A probe that broke each
rule in scratch copies of the spec was caught by the intended property every time. Two
isolated-property runs that expected a control to break a property it was not written for passed,
which showed that property is not what that control breaks.

The model keeps only the log's tail, since transactions are serialised and recovery depends on
nothing earlier; its reasons, and what it leaves to
[PointerPublication.tla](spec/PointerPublication.tla), are in [DESIGN-NOTES.md](DESIGN-NOTES.md)
"A transaction and the commit log, modelled in `Coordination.tla`". Recovery itself is MP-2.2's.

### <a id="mp-2-2"></a>MP-2.2 -- Recovery and its discovery modelled in `Coordination.tla`. *(completed 2026-11-11 09:09:07 -05:00)*

Under the `Txn` switch, [Coordination.tla](spec/Coordination.tla) lets a holder that was told
recovery is required, or whose own transaction failed unresolved, run recovery from the commit
log alone: a prepared transaction is rolled forward through its swaps and its end record, one
that was begun and not prepared is rolled back by sweeping its staging before the log is reset,
the even count and the trusted state are published together, and the fence is cleared last.
Every step is idempotent and takes what it needs from durable state, so a recoverer that stops
between steps, or dies, leaves the rest to the next, which finds the fence, the record or the
untrusted state and starts from the log. Discovery, a call any attached handle can make, reads
the fence, the restart state, the ownership record and whether the lock is held in one step,
reports recovery required for the fence, untrusted state or a record beside a free lock, reports
a live holder's transaction as not requiring it, and changes nothing but its own report. A
bound, `MaxDiscoveries`, keeps discovery out of every configuration that sets it to zero, so
every earlier configuration is unchanged.

Why it is believed correct: the required controls -- discovery that trusts the ownership record
alone, a fence cleared when recovery starts, a trusted count published before the namespace is
whole, and a recovery that rolls a prepared transaction back -- each violate the property that
states the rule, and the first three also show the harm: a live holder's work reported as
abandoned, a stopped recoverer's unresolved work left unmarked so that the next transaction
begins over it, and a read that accepts a half-applied store. Further controls break discovery's
reading of the fence and of the restart state, a discovery that clears the fence it finds, and
trust restored before the swaps. Witnesses reach each recovery path, each step a recoverer can
die after with the next acquirer told, a recovery resumed after an earlier one died, once
between its swaps, a crash that rolled the count back over a prepared transaction, and
discovery finding each signal, a live holder's transaction and a healthy store. A probe that
broke each rule in scratch copies of the spec was caught by the intended property every time.

The model settled what the contract left open: the count and the trust are one publication that
also drops cached samples, the fence is cleared after it and not before, and discovery is one
observation, which an implementation must make consistent. Each is queued in MP-6 and MP-10,
and FM-4.1 now names only what lies beneath the model's staging unit. The design note
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Recovery and its discovery, modelled in `Coordination.tla`"
records four timelines. Recovery of an upgrade's intent is MP-2.3's.

### <a id="mp-2-3"></a>MP-2.3 -- The schema upgrade's body modelled in `Coordination.tla`. *(completed 2026-11-11 11:16:43 -05:00)*

Under a switch, `UpSteps`, [Coordination.tla](spec/Coordination.tla) gives an upgrade a body in
place of its one-step completion. An upgrader that holds its tiers takes writer exclusion last,
and finds it free; it starts only from a store that needs no recovery, so one told that recovery
is required releases and its upgrade is refused. Under exclusion and inside the count's bracket
it records a durable intent naming the layer being upgraded, makes `UpSteps` idempotent migration
steps that are each durable when they end, advances the layer's schema only after the last,
clears the intent, and publishes the closing raise. The body cannot be rolled back: a failure or
a cancellation in it sets the fence and leaves the intent and the count odd, as the death of its
upgrader does. Recovery of an interrupted upgrade is resuming it, before the rest of recovery,
from the intent, the progress and the schema that durable state shows, by a handle that speaks
the target schema; a writer is admitted during an upgrade only if it speaks the target schema, so
a handle that speaks only the source schema never holds exclusion to resume it. A bound,
`MaxUpgrades`, counts upgrade starts apart from write requests, and with `UpSteps` zero every
earlier configuration is unchanged.

Why it is believed correct: the required controls -- a migration step made before the intent, a
schema advanced before the last step, an interrupted upgrade rolled back, and a resumption by a
handle that speaks only the source schema -- each violate the property that states the rule, as
do an upgrader that goes on after being told that recovery is required and a clone that starts
an operation during the body. Witnesses reach the intent recorded, each step, the schema
advancing and an upgrade completing through its body; an upgrader that gives up, is refused or
fails; an upgrader dying before the intent, before its first step, between steps, after its last
step and after the schema advances, each followed by a next acquirer that is told; a recovery
resuming at each of those points, being stopped and dying; and discovery meeting a live
upgrader's body and finding an interrupted upgrade. A probe that broke each rule in scratch
copies of the spec was caught by the intended property every time.

The model settled what the contract left open: an upgrader finds writer exclusion free rather
than waiting for it, because it holds the writer tier exclusively; an upgrade is refused rather
than begun over unresolved work; and an interrupted upgrade is never rolled back, so a store
whose upgrade failed stays recovery-required for every handle that speaks only the source
schema, and only a handle that speaks the target schema can recover it. Each is queued in MP-7
and MP-10. The design note [DESIGN-NOTES.md](DESIGN-NOTES.md) "The schema upgrade's body,
modelled in `Coordination.tla`" records four timelines. Which readers may stay attached through
an upgrade, across layers, is MP-2.4's.

### <a id="mp-2-4"></a>MP-2.4 -- Cumulative reader compatibility modelled in `Coordination.tla`. *(completed 2026-11-11 12:34:26 -05:00)*

A scenario of [Coordination.tla](spec/Coordination.tla) can have more than one layer, each with its
own current schema and oldest admissible reader schema, and each upgrader names the layer it
upgrades and the floor of its upgrade, the oldest reader schema that can read what the upgrade
leaves. The store judges an upgrade against the layer's oldest admissible reader schema and never
against the current one: an upgrade whose floor is at or below it is safe for readers, and any
other takes the reader tier exclusively, which drains every reader before the upgrade begins, and
sets the oldest schema to its target when it completes. The judgment is recorded in the upgrade's
intent with the layer, the floor and the target. The admission rules are applied to every layer of
every attaching handle, and an upgrade of one layer leaves the rules, the oldest schema and the
current schema of every other layer as they were. For each layer the model keeps the data floor,
the highest floor among its completed upgrades, so that a reader that cannot read the data can be
named. Every earlier configuration explores exactly the states it did.

Why it is believed correct: the required control -- an upgrade judged against the current schema
and not against the oldest -- violates the judgment itself, the data floor and a reader's ability
to read the data, in one layer, with a body, and in the second of two layers. Further controls
leave the readers of an unsafe upgrade attached, set the oldest schema of every layer and not only
its own, and apply the admission rules for an upgrade in progress to every layer. Witnesses reach a
reader of an older schema attached through a safe upgrade, a reader two schemas behind through two
in a row, an upgrade that is unsafe against the oldest schema and safe against the current one, an
upgrade of one layer beside handles of another, a writer admitted to resume an interrupted upgrade
of one layer as an ordinary handle of the other, two layers upgraded in turn, and an unsafe upgrade
of the second layer. A probe that broke each rule in scratch copies of the spec was caught by the
intended property every time, once the scenarios started the layers at different schemas: two
probes first survived because both layers' upgrades moved to the same schema, so moving the wrong
layer changed nothing.

The model settled what the decision left open: an upgrade must state the oldest reader schema that
can read what it leaves, and the store, not the upgrade, judges it, because "safe" means nothing
without the oldest schema it is safe against; and an unsafe upgrade sets the oldest schema to its
target, so readers from its floor to its target are refused although they could read, which is the
decision as written and is left alone. The first is queued in MP-7 and the drain of readers is
tested in MP-10. The design note [DESIGN-NOTES.md](DESIGN-NOTES.md) "Cumulative reader
compatibility, modelled in `Coordination.tla`" records two timelines.

### <a id="mp-2"></a>MP-2 -- The part of transactions that processes interact through, recovery, and the schema upgrade's body modelled in `Coordination.tla`. *(completed 2026-11-11 12:34:40 -05:00)*

[Coordination.tla](spec/Coordination.tla) models a transaction over a commit log ([MP-2.1](#mp-2-1)),
recovery and its discovery ([MP-2.2](#mp-2-2)), the schema upgrade's body ([MP-2.3](#mp-2-3)) and
cumulative reader compatibility over several layers ([MP-2.4](#mp-2-4)). Each is behind a switch
or a bound that leaves every earlier configuration exploring exactly the states it did, so the
abstract write and the one-step upgrade remain the cheaper form for configurations that do not
exercise the detail. Whether they are retired once the refined forms are proven is the engineer's
decision, kept open in ODI-3.

## Moved 2026-11-11 -- Model several processes, decided (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="odi-3"></a>ODI-3 -- Both forms of a write and of an upgrade stay in the model for now. *(completed 2026-11-11 15:42:14 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The model clients rely on, and the model the
code is held to"; the options, with what each costs, are in
[DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md](design-sessions/DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md).
It rests on the targeted check of each work item, the tiers and runs scoped to what changed, and
schedules no work of its own.

## Moved 2026-11-11 -- Tooling for the model's checks (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="spec-1"></a>SPEC-1 -- The tools that keep the TLA+ configurations honest are in the repository, and the probe mutants are data beside the spec. *(completed 2026-11-11 17:03:11 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Probe mutants are data in the repository,
matched so that they survive edits to the spec"; the discussion is section 5 of
[DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md](design-sessions/DESIGN-SESSION-2026-11-11-checking-cost-and-the-client-model.md).
What moved, into [tools/](../../tools/):

- the generator of every `spec/Coordination*.cfg`, divided along its tables -- constants, main
  configurations, controls and witnesses -- under
  [tools/gen-coordination-cfgs/](../../tools/gen-coordination-cfgs/), and the generator of every
  `spec/Creation*.cfg`;
- the sweep's self-test, the counterexample summary, the header formatter, and a library of what
  the sweep and the probe runner share;
- [run-probes.ps1](../../tools/run-probes.ps1) and its mutants, from five scratch scripts to data
  under `spec/probes/`. Each mutant was re-expressed until it applied to the current spec and came
  out as recorded under the runner: caught by the property written for its rule, or equivalent
  where it says nothing checks the rule.

The state counts that a scratch script compared by hand did not move: they are derived numbers, and
`SPEC-2` records them with the last green run. Verified by regenerating every configuration, which
changed no file, and by the item-tier sweep of both specs, which found no mismatch.

## Moved 2026-11-11 -- Classifying the model's properties (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="cm-1"></a>CM-1 -- Each property of the models in `spec/` is tagged client or internal, and a client-visible one states its guarantee and the outcome it shows through. *(completed 2026-11-11 18:03:33 -05:00)*

The classification and the outcomes a caller cannot interpret without help are in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Who a property is for". The tag is the first line of a
property's header comment in `Coordination*.tla`, `Creation.tla` and `PointerPublication.tla`, and
[check-audience-tags.ps1](../../tools/check-audience-tags.ps1) fails a spec in which a property
that a configuration names has none, which is how the properties MP-3 and MP-4 write are tagged as
they are written. The edit changed comments only: the modules with their comments removed are
identical to what they were, and a sample of each spec's configurations and the probe corpus came
out as before.

## Moved 2026-11-11 -- Refining the publication model (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="mp-3-1"></a>MP-3.1 -- The `PointerPublication*.cfg` configurations are written from tables. *(completed 2026-11-11 18:25:35 -05:00)*

[gen-pointer-publication-cfgs.ps1](../../tools/gen-pointer-publication-cfgs.ps1) writes every
`spec/PointerPublication*.cfg` as the Coordination and Creation generators write theirs. It
reproduced the seven configurations that had been written by hand byte for byte, and the
item-tier sweep found each at the outcome its header states.

### <a id="mp-3-2"></a>MP-3.2 -- The publication record has live and durable state, and a settle makes it durable before it rolls forward. *(completed 2026-11-11 18:32:44 -05:00)*

The decision and its timeline are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The publication record's
live and durable state, modelled in `PointerPublication.tla`". The record is what a process reads
and what a crash leaves, behind the `RecordDetail` switch, with every fsync able to fail; with the
switch off the configurations that hold explore the states they did. The refinement found that the
guard as designed was unsound: a settle that rolls the replicas forward from a record whose fsync
failed makes the renames durable and not the record, and a crash then strands the vote. The
control that omits the settle's fsync reproduces it, and the corrected guard holds. The control
the item planned, one that ignores a failed clear, does not fail: a stale record is replaced by
the next write before it moves a pointer. The case that does fail needs an operation that moves
pointers without writing a record, which is MP-3.3.

### <a id="mp-3-3"></a>MP-3.3 -- Logged writes on one document, mixed with unlogged writes, and a settle that looks at the durable record. *(completed 2026-11-11 18:44:15 -05:00)*

The decision and its timeline (T25) are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The publication
record's live and durable state, modelled in `PointerPublication.tla`". A transaction's write is
the prepare record, the pointer swaps, the end record and the collection, behind the
`LoggedWrites` switch, with a recovery that rolls a prepared transaction forward; the ended record
a reopen re-runs collection for is state. The failure the item predicted is real: a logged write
that follows a failed clear collects the version the stale durable record names, and a crash
replays the record over the pointers. It is reproduced by the control that makes a settle look
only at the file, and prevented by a settle that fsyncs the record file in every operation. Both
guards hold beside logged writes. A logged write that skips the record guard fails, and one that
skips resolving does not, since its own recovery sets every replica. The cost of the corrected
settle, one fsync per operation, is recorded for FM-2.2.

### <a id="mp-3-4"></a>MP-3.4 -- The operations that settle before they act: checks, collection, repair, scrub and a lost pointer replica are modelled, and a control omits each settle. *(completed 2026-11-11 18:56:35 -05:00)*

The decision and its timeline (T26) are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The publication
record's live and durable state, modelled in `PointerPublication.tla`". The `Maintenance` switch
adds a precondition check, a standalone collection pass, a repair and a scrub, each beginning with
the settle under the record guard, and the loss of a pointer replica of a settled document. The
controls that omit the settle fail for the check, the collection pass, the unlogged write and the
logged write. The item predicted the same for repair and scrub, and it does not hold: each writes
only the vote's version to a replica that has none, so the model finds nothing wrong without the
settle, and whether the implementation settles before them is left to FM-2.2 with that finding.
Both guards hold with every invariant in the refined configurations. A lost replica separates
them without failing a safety property: the resolve guard restores it at the next write, and under
the record guard a publication that then fails part-way leaves no majority until the next settle.
A restart is modelled as recovery; a clean open that settles nothing is a question about many
documents and is carried to MP-4.

### <a id="mp-3"></a>MP-3 -- Refine the model of one document's publication until the choice between the two guards that hold rests on equal evidence. *(completed 2026-11-11 18:56:35 -05:00)*

The record guard's "holds" was provisional because the model wrote and cleared the record
atomically and never mixed logged with unlogged writes, and the resolve guard had never been
checked beside a logged write. Both are now checked with the record's live and durable state,
logged and unlogged writes on one document, and the operations that read or repair a document,
each refinement behind a switch that leaves the earlier configurations exploring the states they
did. The refinement found that the guard as designed was unsound (T24) and that a settle must
fsync the record in every operation (T25), and it recorded what each guard's result is, in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Guards against `STRANDED-VOTE`, compared in the model".
Sub-items: [MP-3.1](#mp-3-1), [MP-3.2](#mp-3-2), [MP-3.3](#mp-3-3), [MP-3.4](#mp-3-4).

## Moved 2026-11-11 -- Composing the publication model with the coordination contract (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="mp-4-1"></a>MP-4.1 -- The composition is a model of its own that extends `Coordination.tla` with documents, so it refines the contract by construction. *(completed 2026-11-11 19:56:33 -05:00)*

The decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Composing the publication model with the
coordination contract". The engineer first chose to restate the contract and check refinement
from a separate root, with documents only in the composed model and a `Docs` constant of two in
the item tier and three in one milestone configuration. Checking the mapping before building
found four things in `Coordination.tla` that block it. The engineer chose to fix the one that is a
gap in that model, recovery of an unlogged write, as its own item. Starting the model showed that
extending `Coordination.tla` gives refinement by construction with nothing restated, and the
engineer chose that form instead.

### <a id="mp-4-2"></a>MP-4.2 -- `Coordination.tla` recovers an unlogged write behind the `UnloggedRecovery` switch. *(completed 2026-11-11 20:03:23 -05:00)*

Without `Txn` a told holder could only release, so an unlogged write that failed unresolved left
the fence set for good. Under the switch it may recover: it makes a store half applied whole,
forward or back, then ends as a transaction's recovery does. The new main holds every invariant
and property with a death, a crash and a read. The existing controls for publishing the count
before the namespace is whole and for clearing the fence at the start fail with the switch on.
The new witnesses reach a roll forward, a roll back, recovery after a restart that left nothing
half applied, and recovery after a holder's death. The configurations without the switch explore
the states they did, and the one mutant whose text the change moved was re-expressed and caught.

### <a id="mp-4-3"></a>MP-4.3 -- `SharedPublication.tla` composes both guards with the contract over several documents, with two writers, a death and a host crash. *(completed 2026-11-11 20:55:34 -05:00)*

The findings are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Composing the publication model with the
coordination contract", with timeline T27. Both guards hold every per-document invariant, the
client property that the outcome is fixed once recovery has run, and the property that only a
holder of exclusion changes a document, with unlogged and with logged writes. The item planned
one process, but `Coordination` uses each process once, so a single process cannot recover after
its own death or a restart; the configurations have two writers. The guards' work at the start of
a write was left out of the model, and a property checks that a holder who is not told recovery
is required always finds that work with nothing to do, which it does under both guards. The
controls fail as planned: an open that settles, a recovery that skips a document, a recovery that
leaves the documents under either guard, and no guard at all.

### <a id="mp-4-4"></a>MP-4.4 -- Every condition that sets the fence is checked in `SharedPublication.tla`, with three writers, three documents and three requests in the milestone tier. *(completed 2026-11-11 21:28:12 -05:00)*

The findings are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Composing the publication model with the
coordination contract". Under the `Coordination` controls that replace an abandoned ownership
record without the fence, ignore a host restart, or roll a prepared transaction back, a holder
who is not told recovery is required finds a document unsettled. The first needed three
requests: a told holder that releases without recovering still leaves the next acquirer told by
the record itself, and only the acquirer after that relies on the fence. The in-process fence of
`PointerPublication` turned out to be the contract's fence with no guard, which the existing
control already fails. `tools/run-probes.ps1` copies the modules a spec extends and accepts any
specification name, and a probe of the composition's recovery is caught. The milestone
configurations of both guards hold every invariant and property.

### <a id="mp-4-5"></a>MP-4.5 -- Each write chooses its form behind `Coordination`'s `MixedWrites` switch, and the composition mixes logged and unlogged writes across processes. *(completed 2026-11-11 21:47:32 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Composing the publication model with the
coordination contract". The choice is made by the step a write takes from the opened operation,
and the log tells the two apart while the write is between its effects, so no variable was
added. `Coordination`'s mixed main and both guards' mixed composed configurations hold every
invariant and property, a `Coordination` witness reaches an unlogged write after a transaction,
and a composed witness reaches recovery rolling both kinds forward in one behaviour. The
configurations without the switch explore the states they did. Four mutants whose text the
change moved were re-expressed and caught.

### <a id="mp-4-6"></a>MP-4.6 -- A cyclic workload for `Coordination.tla` behind the `Cyclic` switch, and the tools to check liveness. *(completed 2026-11-11 23:44:58 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "What the contract's lack of a fairness
requirement does". Under `Cyclic` a process that exits may attach again, requests, reads,
discoveries and listings are not counted, and the count, the generation and the versions wrap
within `CycleMod` through `Nx`, `Pv`, `Room` and `Tick`; deaths, crashes and upgrades stay
bounded. The liveness module is `CoordinationLiveness.tla`, and its configurations are written by
`tools/gen-coordination-cfgs/Liveness.ps1`, which names each one's specification. The outcome
reader recognises a violated temporal property. Both witnesses are reachable, the cyclic control
in which an upgrader keeps its tiers deadlocks, and every configuration without the switch
explores the states it did. Twelve mutants whose text the counter arithmetic moved were
re-expressed and caught.

### <a id="mp-4-7"></a>MP-4.7 -- The contract's fairness is checked on the cyclic workload: the store makes progress, a writer and an upgrader can each be passed over forever, and the points of conflict are listed. *(completed 2026-11-12 08:09:46 -05:00)*

The findings, the starvation timelines T28 to T30 and the points of conflict are in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "What the contract's lack of a fairness requirement does".
Fairness on a resource alone starved a handle whose shared take was grantable throughout, which
no lock does, so with the engineer `ContractSpec` was made weakly fair on each process's
acquisition, and release and I/O completion strongly fair, since a holder's nested operations
interrupt them. Fairness is stated only for the roles that can take each step, which keeps the
formula within what TLC can expand. The store makes progress, an upgrade holding its tiers and a
driven recovery complete, and dropping any one fairness condition stops the store. A waiting
writer and a waiting upgrader can each be passed over forever. A handle waiting to attach is
served, beside a second writer and beside an upgrader, which the item had expected to fail.

### <a id="mp-4-8"></a>MP-4.8 -- Strong fairness on each acquisition serves a writer and an upgrader beside one handle, not an upgrader beside two whose holds of the gate overlap, and withholding shared admission serves it. *(completed 2026-11-12 11:18:31 -05:00)*

The results are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "What the contract's lack of a fairness
requirement does" and "The acquisition layer is a model of its own". TLC could not expand
`FairSpec`'s formula for three processes in the coordination model, first refusing it and then
running out of memory, so the coordination model's three-process checks use `UpgraderFairSpec`,
the part of `FairSpec` the upgrader's service depends on; there an upgrader beside two readers is
passed over (T30), and in T30's cycle every enabled acquisition is made, so it is passed over
under `FairSpec` too. The upgrader beside one reader and the exclusive-first variant held in the
coordination model after hours each. The acquisition model then established every result in
seconds, the waiting writer under `FairSpec` with three writers among them, and found that
withholding shared admission serves the upgrader under the contract's fairness alone.

### <a id="mp-4-9"></a>MP-4.9 -- The acquisition layer is a model of its own, `spec/Acquisition.tla`, which establishes the service results in seconds under the assumption that every hold ends. *(completed 2026-11-12 11:18:31 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The acquisition layer is a model of its
own". The model has writer exclusion, the gate and the tiers, abstract holds, and no counter to
wrap, so its invariants and liveness are checked together. Its configurations are written by
`tools/gen-acquisition-cfgs.ps1`; every one, the seven fairness controls and three witnesses
included, came out as its header requires, and six probes of its exclusion conditions and of
exclusive-first are caught. The first probe of the writer tier survived in the unsafe scenario,
where a writer cannot reach the writer tier while an upgrader holds it, so it runs in a scenario
with an upgrader safe for readers and a writer, which the reopened gate lets reach it.
`tools/run-tlc-all.ps1` gained `-Parallel` and `tools/run-tlc.ps1` `-HeapGB`, and the sweep an
`evidence` tier that it runs only on demand.

## Moved 2026-11-12 -- Merging the acquisition model into the coordination model (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="mp-4-10"></a>MP-4.10 -- `Coordination.tla` refines `Acquisition.tla` under a mapping, checked as `AcquisitionRefined` in every main configuration. *(completed 2026-11-12 12:19:00 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The coordination model refines the
acquisition model". The item asked for the coordination model's take and release actions to
conjoin the acquisition model's; with the engineer it was decided instead to keep the coordination
model's own steps and check them against the acquisition model's by a refinement property, since
its controls need their own guards and a conjunction would prune a disagreement where a property
reports it. The mapping is in `spec/CoordinationAcquisition.tla`. The acquisition model gained the
ways a wait or a hold ends other than by service, behind `Escapes`, `Refusals` and `Deaths`, and
`CountUpgrades`, all leaving its earlier configurations as they were; its positive results were
checked again with them on, which needed progress restated where a waiter may leave, and found
that with upgrade starts not counted an attaching handle can be passed over forever. Two probes
that break a take or a release only in a way the refinement sees are caught by
`AcquisitionRefined`; the one that gives up while holding the reader tier, run where a process
may die, survived as a death until it was moved to a configuration where none dies. TLC names a violated action property by its location,
so `tools/lib/Tlc.ps1` resolves a location to the definition that holds it, and
`tools/check-run-tlc-all.ps1` copies `Acquisition.tla` beside its copy of the coordination model.

### <a id="mp-4-11"></a>MP-4.11 -- `Coordination.tla` shows every hold ends on bounded configurations under the fairness of a process's own steps, and its cyclic workload and liveness configurations are retired. *(completed 2026-11-12 13:15:05 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Every hold ends, checked in the
coordination model". `spec/CoordinationLiveness.tla` now states `HoldSpec` and the hold-ends
properties -- exclusion released, the gate dropped, an admission, a drain, a detach, an upgrade
holding its tiers and a recovery under way each ending -- checked on five bounded configurations
in the milestone tier, since the liveness pass takes minutes each. Each fairness condition dropped
gives a control that fails the property it guards, the release and the last effect each because a
holder's own loop -- a recovery stopped and restarted, an operation nested again and again --
otherwise keeps the hold. With the engineer it was decided to retire every `Coordination-Live-*`
configuration, the cyclic controls and witnesses, and the `Cyclic` and `CycleMod` constants,
whose findings the acquisition model now carries; the SharedPublication configurations dropped
those constants with them.

### <a id="mp-4"></a>MP-4 -- The model is checked with several processes: publication composed with the coordination contract, and the contract's fairness moved into an acquisition model the coordination model refines. *(completed 2026-11-12 13:15:05 -05:00)*

Its parts are `MP-4.1` to `MP-4.11` above and in the earlier groups of this file. The item's own
text asked for two independent halves, the first composing the coordination contract with
[PointerPublication.tla](spec/PointerPublication.tla) for FM-2.2, the second checking what the
contract's lack of a fairness requirement does and moving those results into a model of the
acquisition layer the coordination model builds on, each refinement behind a switch that leaves
every earlier configuration exploring the states it did.

### <a id="mp-4-12"></a>MP-4.12 -- Nothing nests under a hold in `Coordination.tla`, since nothing outside the store holds exclusion across a call. *(completed 2026-11-12 15:23:25 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "No caller code runs under exclusion but the
commit observer": with a transaction built without exclusion and submitted once, and the rule that
caller code under exclusion never calls into the store, no caller's operation runs inside another.
The nested operation, its control `inner-restores-parity`, its witness, its probe, and the strong
fairness on a write's last effect that only the nesting loop needed are gone from the model, with
the hold-ends control that dropped that fairness. Every Coordination and SharedPublication
configuration in the item tier and every hold-ends configuration came out as its header requires;
without the nesting loop and its fairness condition the hold-ends checks took a third to a fifth of
the time they had, which is what the change was for.

### <a id="odi-7"></a>ODI-7 -- The rule for caller code under exclusion is a step and three controls in `Coordination.tla`, and the commit observer's documentation states it. *(completed 2026-11-12 15:50:00 -05:00)*

The engineer took the rule -- a callback under exclusion waits on nothing, does not call into the
store, returns, and does work bounded in time -- as decided, and asked for it in the model's
precise form, recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) "No caller code runs under exclusion
but the commit observer". Behind the `Callbacks` switch, off in every earlier configuration, the
store calls back with `Observe` once an operation's effects are done, and
`ACallbackChangesOnlyWhereItsHolderIs` holds in every main configuration. Each clause has a
control that comes out as its header requires: a callback that writes to the store violates that
property, one that waits for exclusion its holder has deadlocks, and one that never returns keeps
its hold forever. The time bound is not the model's, since no step has a duration; it is a
parameter of CQ-1's workload. The store does not check the rule at run time; the commit
observer's documentation states it as a requirement on its author.

### <a id="cq-1-1"></a>CQ-1.1 -- The wait budget, the simulator as a crate of its own, unordered service as a uniform random choice with swept barging, and hold times measured on this machine, agreed with the engineer. *(completed 2026-11-12 16:13:39 -05:00)*

The budget and the measured hold are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) "How long the
contract makes a caller wait". The holds were measured with the existing ignored throughput test in
[throughput.rs](tests/throughput.rs).

### <a id="cq-1-2"></a>CQ-1.2 -- `crates/contention-sim` simulates one exclusion under each discipline, a snapshot under writes, and an upgrader behind attachments, checked against the M/G/1 baseline. *(completed 2026-11-12 16:13:39 -05:00)*

The crate is a member of the default workspace. Its unit tests pin the generator, the
distributions' moments, the statistics, the Pollaczek-Khinchine baseline against the simulated
mean, each discipline's ordering property (first-come-first-served passes no waiter over, bounded
bypass passes none over more than its bound), the snapshot approximation and the gate's draw.

### <a id="cq-1-3"></a>CQ-1.3 -- The three workloads ran against the budget; the predictions, workload and method are in DESIGN-NOTES, and the work they imply is queued. *(completed 2026-11-12 16:13:39 -05:00)*

The findings are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "How long the contract makes a caller
wait". The snapshot's spinning retry fails a read whenever a write overlaps it, which spawned
`RD-1` in M5; the findings on bursts and on an upgrade's drain were written into `ODI-2`, which
decides avoidance and fairness from them.

### <a id="cq-1"></a>CQ-1 -- A quantitative model of contention for the three workloads, run against an agreed budget, with its findings recorded. *(completed 2026-11-12 16:13:39 -05:00)*

Its decisions, its simulator and the work it spawned are recorded under its sub-items `CQ-1.1` to `CQ-1.3`
above; the findings are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "How long the contract makes a
caller wait". The simulation answered every question the item asked, so no probabilistic model
checker was raised.
