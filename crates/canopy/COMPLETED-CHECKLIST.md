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

## Moved 2026-11-12 -- Deciding the contract's fairness (M3 of [CHECKLIST.md](CHECKLIST.md))

### <a id="odi-2"></a>ODI-2 -- The contract promises no order among waiters at any point of conflict; the alternatives were weighed and none was taken. *(completed 2026-11-12 18:54:39 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The contract promises no order among waiters
at any point of conflict". The engineer was offered, for each point of conflict, an avoidance or the
weakest fairness the evidence supported -- bounded bypass on writer exclusion gated on modelling it,
group commit, exclusive-first admission, asking attached handles to let go when an upgrade drains --
and chose none: the burst breach on writer exclusion is accepted, the gate stays unordered, the
drain is a documented wait, and an attaching handle's service is stated to rest on upgrades being
finitely many. No work was spawned; CQ-2's benchmark is what reopens the decision.

### <a id="odi-4"></a>ODI-4 -- A timeline is replayed by TLC from a scenario table, in a form the trace corpus adopts. *(completed 2026-11-12 19:02:10 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "A timeline is replayed by TLC from a
scenario table". The engineer chose the TLC replay now and the trace corpus in M7: the work became
`SPEC-2.2`, and `FM-3.1` was extended to carry the replays into the corpus. SPEC-2 was split into
`SPEC-2.1` to `SPEC-2.3` in the same change, since the replay and the scheduled workflow are each
work of their own beside the impact analysis.

### <a id="spec-2-1"></a>SPEC-2.1 -- `run-tlc-all.ps1` and `run-probes.ps1` select by what changed, from a definition graph of each spec and a git-ignored record of the last run that came out as required. *(completed 2026-11-12 19:22:40 -05:00)*

The analysis is [Impact.ps1](../../tools/lib/Impact.ps1); how the tools use it is in their headers
and in the "TLA+ / TLC" section of the repository's Copilot instructions. Every reference is
resolved by name, the instance of `Acquisition.tla` included -- a reference `Acq(...)!D` reaches
the instanced module's `D` and the instance's own definition -- so SANY's semantic export was not
needed. A subscript (`[][Next]_vars`, `WF_vars(A)`) is read as naming what follows its underscore;
the first form of the analysis missed it, and the check that an edit to `vars` selects every
configuration is what found it. [check-run-tlc-all.ps1](../../tools/check-run-tlc-all.ps1) checks
the selection on a toy spec with a record of its own: nothing selected when nothing changed or only
a comment did, one witness for an edit to that witness, every configuration for an edit to `vars`,
one configuration for an edit to its `.cfg`, a changed distinct-state count reported, and a probe
selected by an edit to a definition its base reaches.

### <a id="spec-2-2"></a>SPEC-2.2 -- Timelines T1 to T27 are replayed by TLC from scenario tables under `spec/timelines/`, each against its own specification so that an edit selects only the replays it touches. *(completed 2026-11-12 19:40:30 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "A timeline is replayed by TLC from a
scenario table"; the generator is [gen-timeline-replays.ps1](../../tools/gen-timeline-replays.ps1)
and [trace-to-scenario.ps1](../../tools/trace-to-scenario.ps1) drafts a table from a
counterexample, since TLC names each action of a trace with its arguments. Every timeline replays
against the current spec, the projected steps of T18 and the steps T13 and T16 say their
configurations do not reach included; T13 needs a crash and T15 a second request, which their
tables add as overrides. T3 needs its reader to attach after the crash, since a crash ends every
attachment in the model and the timeline predates attachment; its table marks that as an omitted
step. SharedPublication's steps are all named `SpNext` in a trace, so T27's actions were read off
its state changes. The first form had one specification for every timeline, so an edit to any
action selected every replay; giving each timeline its own made a scratch edit that disabled
`SetFence` select exactly T1, T2 and T18, each of which then failed at that step.

### <a id="spec-2-3"></a>SPEC-2.3 -- `.github/workflows/tla-sweep.yml` runs the probes and the full sweep weekly, only when what it covers changed since the last green run. *(completed 2026-11-12 19:47:05 -05:00)*

The workflow installs the pinned JDK and `tla2tools.jar`, checks the jar's digest, digests every
file under the spec directory and `tools/` into its cache key, and skips when that key has a green
run on record; a manual dispatch can force a run. It runs on a hosted Windows runner, since the
tools and TLC are run on Windows. Running it is gated on the workflow reaching the default branch,
where GitHub dispatches workflows from, so its verification and the hosted-or-self-hosted decision
were spawned as `SPEC-2.4` in `M3+`, with the engineer's agreement.

### <a id="spec-2"></a>SPEC-2 -- The runs that follow a spec edit are selected by what the edit can affect, timelines are replayed among them, and a scheduled workflow runs the full sweep. *(completed 2026-11-12 19:47:05 -05:00)*

Its parts are `SPEC-2.1` to `SPEC-2.3` above; the decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md)
"Runs that follow a spec edit are selected by what the edit reaches". The item as written:

> Scope the runs that follow a change to what the change can affect, so that once a model's rules
> are authored the standard cost of a spec edit is small. Build in `tools/` an impact analysis over
> a spec's module chain: the definitions each definition refers to (resolved by name where the
> chain has no `INSTANCE`, from SANY's semantic export where it does), the definitions each
> configuration reaches from the names its `.cfg` lists, and the definitions that differ from the
> last green run. A configuration is selected when it reaches a changed definition, when its own
> `.cfg` changed, or when a declaration (`VARIABLES`, `CONSTANTS`, `ASSUME`) changed; a probe is
> selected by the same rule, applied to the configuration it runs on and the definition it
> mutates. A reference by name over-approximates, so a selection is never too small, but it
> cannot see that a new disjunct is dead behind a switch: an edit to a definition every
> configuration reaches -- `Init`, `vars`, `Next` -- selects every configuration. That is accepted
> while a model's rules are being authored and is not the standard; selection narrows as edits
> become local to a property, a scenario or a witness, and it can stand in for a hand-written
> `Verify:` line.
>
> The last green run is recorded per configuration as a digest of what it reaches, its `.cfg`, the
> TLC version and its options, in git-ignored state or a workflow's cache and never in a
> checked-in document. The record also holds each configuration's distinct-state count, so that an
> edit meant to leave earlier configurations exploring exactly the same states -- a switch added
> behind a default that changes nothing -- is checked by comparing counts, which replaces a list
> of counts kept by hand. `run-tlc-all.ps1` takes the selection as an option, and a full run
> ignores the record. A scheduled workflow runs the full sweep and the full probe run -- weekly is
> the proposal -- only when the spec, its configurations, the tools or the pinned TLC changed
> since the last green run; the repository's weekly code-scanning schedule is the pattern, and
> whether a hosted runner finishes the sweep within its time limit decides whether the runner is
> hosted or self-hosted. A timeline's replay is a configuration like any other, so the selection
> covers it.

The instance of `Acquisition.tla` was resolved by name too, so SANY's export was not needed; TLC's
options other than the jar change no outcome, so the digest does not include them.

## Moved 2026-11-12 -- Implementing the `STRANDED-VOTE` guard (M4 of [CHECKLIST.md](CHECKLIST.md))

### <a id="fm-2-2"></a>FM-2.2 -- `STRANDED-VOTE` is guarded by a durable publication record, behind genesis version 6 and a migration from version 5. *(completed 2026-11-12 20:30:51 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "`STRANDED-VOTE` is guarded by a durable
publication record". The engineer chose the record over resolving before each write. An unlogged
write records its pointer durably before renaming any replica and clears it once every replica
names it; every operation that takes the writer gate, and `open`, settles a pending record first,
syncing the file before acting on it. Repair and scrub settle too, by the engineer's choice rather
than for any property the model checks. The format barrier and the migration of a version-5 root,
the first part of MP-6, landed here: `open` refuses a version-5 root with
`StoreError::MigrationRequired`, and `Store::migrate` advances it under the root lock with the
marker read again under the lock. `PointerPublication-Settled.cfg` now checks the protocol as
built; the unguarded counterexample is kept as `PointerPublication-Control-Unguarded.cfg`.

Revert-probes pinned the mechanism: with the settle disabled the `STRANDED-VOTE` sequence and the
reopen and power-loss tests failed, and with the settle's fsync of the record removed the tests
for a clear and for a settle interrupted by a crash failed.

### <a id="fm-2-3"></a>FM-2.3 -- A pointer vote with no majority that nothing was modifying is reported as `StoreError::PointerUnavailable`, not as contention. *(completed 2026-11-12 20:34:41 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "A pointer vote nothing was modifying reports
its disagreement". `read_document` samples the modification count before each pointer vote and
again when the vote finds no majority; when `Store::undisturbed` holds of the two it returns the
vote's error, so a caller's `read_settled` loop stops instead of waiting out a writer that does not
exist. The scatter test that read as contention was split in two: under an open modification it is
still contention, and with nothing modifying it is the error. A revert-probe of the undisturbed check
failed exactly the nothing-modifying test and the lost-replica case (T26), so the classification is
pinned.

## Moved 2026-11-12 -- Readers in other processes (M5 of [CHECKLIST.md](CHECKLIST.md))

### <a id="rg-1"></a>RG-1 -- The doorbell's spec barrier: `DoorbellContract.tla` states what a user observes, and `Doorbell.tla`, the mechanism, is checked by TLC to refine it. *(completed 2026-11-12 22:21:00 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The doorbell's contract and its mechanism,
modelled in `Doorbell.tla`". The configurations are generated by `tools/gen-doorbell-cfgs.ps1`:
the refinement and the contract's two guarantees hold for one, two and three producers; the four
controls named in the item each lose a wake; and three witnesses show a ring skipping its raise,
the consumer waiting with everything taken, and a busy consumer taking again without a system call.
A scratch run in which the consumer lowered and cleared on every pass still lost a wake with the
clear before the reset, which is how the decision states the reason for that order.

### <a id="rg-2"></a>RG-2 -- `canopy::ring::Doorbell` realizes `Doorbell.tla` step by step over acadia's signal, checked against the contract over every interleaving and run from each platform's thread-pool wait. *(completed 2026-11-12 22:33:47 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The doorbell in code,
`canopy::ring::Doorbell`". The doorbell is `src/ring/doorbell.rs`; acadia gained `wait_native`, the
platform's own wait on a borrowed event or descriptor. The tests in `src/tests/ring.rs` explore
every state the doorbell's own steps reach with one, two and three producers and check the
contract's guarantees at every final state; the same search finds the lost wake of each of
`Doorbell.tla`'s four controls, which shows it can fail. Over the platform's signal, the doorbell is
pumped by many producers, waited on with the platform's own wait, and registered with a Windows
thread-pool wait (`windows-threadpool-sys`'s `ThreadpoolWait`) and, on Linux, an `epoll` set a pool thread waits on; all of it ran on Windows and
under WSL. The check of the fences by a checker of the Rust memory model was split out as RG-2.1
(M5+), owned by the engineer.

## Moved 2026-11-13 -- The doorbell's orderings checked by loom (M5+ of [CHECKLIST.md](CHECKLIST.md))

### <a id="rg-21"></a>RG-2.1 -- The doorbell uses acquire and release alone, and loom checks that each of its orderings is needed and that together they lose no wake. *(completed 2026-11-13 14:20:00 -05:00)*

The engineer chose loom and asked for the check to be built, and directed that the doorbell use no
`SeqCst`. The finding and its limits are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The doorbell's
orderings, checked by loom". The doorbell's two sequentially consistent fences were replaced by
making the consumer's `Clear` an `AcqRel` swap, which meets every producer's `Test` on the record.
The checks are `src/tests/ring_loom.rs`, run with `RUSTFLAGS="--cfg canopy_loom"`; under that cfg
the ring's atomics are loom's (`src/ring/sync.rs`) and acadia supplies `LoomSignal`. Loom is first
shown to tell the doorbell's bare shape apart, met by read-modify-writes and by a clear that is a
store; then a `Test` that does not release, a `Clear` that does not acquire, and a `Clear` that is a
store each lose a wake, which loom finds. The ordering of the signal's raise and lower against the
record, which loom cannot show, was spawned as `RG-2.2`.

### <a id="rg-22"></a>RG-2.2 -- acadia's `Signal` promises that each operation is ordered as an acquire-release operation on the level, which carries the doorbell's orderings against its signal. *(completed 2026-11-13 14:50:00 -05:00)*

The engineer took the contract as the obvious one, following from the platforms' documentation,
and asked for it to be written down. It is the `# Ordering` section of `Signal`'s documentation in
`crates/acadia/src/signal.rs`, argued per platform in [acadia's
DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "A signal's operations are ordered as acquire-release
operations": `SetEvent` and the wait functions are documented to order memory around them,
`ResetEvent` only needs its effect done before it returns, an `eventfd`'s counter changes under its
own lock, and the condition-variable signals are a mutex. With it, the consumer's lower happens
before its clear, which a producer's `Test` acquires, which happens before that producer's raise
([DESIGN-NOTES.md](DESIGN-NOTES.md) "The doorbell's orderings, checked by loom"). The ring's queues
(API-3) are checked by loom and rest on the same contract.

### <a id="rg-3"></a>RG-3 -- `Doorbell::watch` runs the consumer's side on the platform's thread pool through acadia's `Signal::watch`, and canopy's tests consume the doorbell through it. *(completed 2026-11-13 15:30:00 -05:00)*

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "A consumer watches the doorbell from the
thread pool". `Doorbell::watch(take, failed)` takes `self: &Arc<Self>`, runs a pump that does not
wait over `take` each time the signal is found raised, and hands a failed pump to `failed`;
`canopy::ring` re-exports acadia's `Watch`. The tests take all of several producers' work through a
watch over the condition-variable signal and the platform's signal, and show a dropped watch taking
nothing rung for afterwards while a second watch on the same doorbell takes it; they ran on Windows,
and the Linux path, acadia's `epoll` service, was not run on this machine, whose WSL has no Rust
toolchain. The thread-pool tests that used the `unsafe` accessors were replaced, and canopy's
`windows-threadpool-sys` and Linux `libc` dev-dependencies removed with them.

### <a id="rg-31"></a>RG-3.1 -- canopy's ring tests and acadia's unit tests pass on Linux under WSL, the watch running through acadia's `epoll` service. *(completed 2026-11-13 15:45:00 -05:00)*

Run once the engineer had installed Rust in WSL; rustup installed the pinned 1.98 toolchain on
first use. The run used the Windows registry cache (`CARGO_HOME` on `/mnt/c`, `--offline`) and a
target directory under `/tmp`, as [copilot-instructions.md](../../.github/copilot-instructions.md)
describes.

### <a id="nt-1"></a>NT-1 -- `Coordination.tla` models the notification: a waiter registers, a change notifies it as a weakly fair step of its own, and it samples again before deciding. *(completed 2026-11-13 15:14:41 -05:00)*

The decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "A notification is a step of the coordination
model". Behind the constant `Notifications`, false in every earlier configuration, a waiting
operation's steps run only while its notice is `sample`; `Register` puts it to sleep when it is not
ready, and `Notify`, under weak fairness, wakes it once it is. The invariant `ReadyMatchesTheWaits`
holds the hand-written `Ready` to the guards of the wait steps, so the notifier cannot drift from
what it notifies about. Believed correct because each control fails for the reason it was built to:
`Control-NotificationLost` deadlocks with a waiter whose notice was lost and no process left to
change anything, and `Control-HoldsEnd-NotificationLost` violates `ADrainEnds` with the drainer's
notice lost. The spurious control leaves every property holding, the witness shows a waiter woken by
a real change, and the configurations that predate the switch came out as before.
`Coordination-Notify` is tagged for the milestone tier.

## Correction 2026-11-13 15:17:29 -05:00 -- the completion stamps of RG-2.1, RG-2.2, RG-3 and RG-3.1 are estimates

The stamps on [RG-2.1](#rg-21) (14:20:00), [RG-2.2](#rg-22) (14:50:00), [RG-3](#rg-3) (15:30:00) and
[RG-3.1](#rg-31) (15:45:00) were not read from the clock. The clock was read once before
them, at 2026-11-13 13:41:15 -05:00, and each was written as a round estimate afterwards. They
run ahead of real time: all four items were complete before [NT-1](#nt-1), whose stamp,
15:14:41, was read from the clock. Their true order is the order of the entries above, and each
falls between 13:41:15 and 15:14:41 -05:00. The entries are left as they were written.

## Moved 2026-11-13 -- The coordination interface defined (M6 of [CHECKLIST.md](CHECKLIST.md))

### <a id="m6-1"></a>M6-1 -- Define the coordination interface as `acadia::coord`, with the file-based layout, the conformance harness, a second implementation and abandonment. *(completed 2026-11-13 20:52:22 -05:00)*

The decisions are in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "The coordination
interface is acadia's, as a layer of its own above `Vfs`" and "The coordination interface,
defined". Each part M6-2 to M6-9 implements is a trait in `acadia::coord` with its contract stated
on it, and joins `Coordination` as a supertrait when its item builds it. The file-based
implementation lays out its state beneath the root with every name under the reserved prefix, and
keeps every lock a participant takes in a table that abandonment empties without bookkeeping. The
harness runs each case against both implementations over `MemFs`, `OsFs` and `FaultyFs`, and runs
the cross-process cases by starting the test binary again as a helper, on Windows and on Linux under
WSL. Making abandonment keep its locks failed the three abandonment cases.

### <a id="m6-2"></a>M6-2 -- Creation exclusion and the creation reservation, built in the file-based implementation and joined to `Coordination`. *(completed 2026-11-13 21:13:05 -05:00)*

The decision is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "Creating a root through
the file-based implementation". The conformance cases cover every case the item named --
simultaneous creation through two implementations, a creator suspended before and after publishing
its reservation, an interrupted creation resumed and refused for other inputs and another
implementation, a resumer that waited while its creation finished, a root holding something creation
does not make refused to a creator, to a resumer and to a creator it appeared to while it waited, a
creator that died between making its marker and making it durable followed by an open and a crash,
and a crash at every step -- with both implementations over every backend where the case allows. A
combined revert-probe -- one reading of the root, a plain write for the reservation, an open that
syncs nothing -- failed the cases built for each.

### <a id="m6-3"></a>M6-3 -- Restart recognition from the boot identity the filesystem reports, with untrusted state requiring recovery, joined to `Coordination`. *(completed 2026-11-13 21:21:05 -05:00)*

The decision is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "A host restart is
recognised from the boot identity the filesystem reports". `Vfs::boot_identity` is new: the kernel's
boot id on Linux, `KUSER_SHARED_DATA.BootId` on Windows, a crash count in `MemFs`, forwarded and
faultable by `FaultyFs`. The file-based implementation keeps a boot record in its state directory
and trusts the state only when the record names this boot and a recovery has vouched for it. The
cases cover state never made, made and vouched, a crash that keeps the record and one that loses
it, a filesystem that cannot identify a boot or fails to, an unreadable record, an abandoned
participant, and state vouched for in another process, over both implementations and every
backend, on Windows and Linux. A revert-probe ignoring the recorded boot failed the crash case.

### <a id="m6-4"></a>M6-4 -- The modification count, published whole with its boot and standing, raised by the outermost modification, joined to `Coordination`. *(completed 2026-11-13 21:28:14 -05:00)*

The decision is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "The modification count
is one record, carrying its boot and its standing". The holder's raising is built and reached by
writer exclusion's hold, which M6-5 and M6-6 supply. The cases cover a sample during a publication
suspended at its write and at its rename, a holder killed before its raise is published and one
killed in another process with its count raised, each publication failing, a reader with the record
open while it is replaced, a sample after a restart, a count never published, untrusted or garbled,
nested and unresolved modifications, and an abandoned participant, over both implementations and
every backend, on Windows and Linux. Revert-probes on the standing and on the fence after a failed
close each failed their cases.

### <a id="m6-5"></a>M6-5 -- Writer exclusion in the file-based implementation: one lock file held whole, across every thread of every process, released by a death. *(completed 2026-11-13 21:48:36 -05:00)*

The decision is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "Writer exclusion is one
lock file, held whole, in the participant's table of holds". The item and M6-6 meet at
`ExclusionHold`, whose report of what an acquisition found is M6-6's ownership record, fence and
generation; so this item built the exclusion and its release, and M6-6 builds the hold over it and
joins `WriterExclusion` to `Coordination`. The cases cover refusal without waiting, re-entry, a
dropped hold letting the next in, a waiter let in by a release, many threads holding it alone,
releases overlapping tries, abandonment, a hold outliving its borrow, separate roots, a holder in
another process refusing this one until it is killed, and a waiter in another process let in by
this one's release, over both implementations and every backend, on Windows and Linux. A
revert-probe that kept the lock after its hold was dropped left the waiting cases waiting.

### <a id="m6-6"></a>M6-6 -- The `ExclusionHold` over the exclusion, with the ownership record, the fence, the generation, the boot record and the count's raising kept under it, joining `WriterExclusion` to `Coordination`. *(completed 2026-11-13 21:58:50 -05:00)*

The decision is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "The hold of writer
exclusion keeps its bookkeeping in files replaced whole". The cases cover the first acquisition of
new state, the generation found and advanced with and without another holder between, a clean
release and a dropped hold, a holder that dies in this process and one killed in another, the fence
outliving the clean release of the holder that found a death, a live holder releasing never
reported dead, acquirers failing at each step before their record, a release that cannot clear its
record, the recovery gate on ordinary modifications, a completed recovery, nested raising through
the hold, a restart, an unreadable generation, and the whole hold reached through `dyn
Coordination`, over both implementations and every backend, on Windows and Linux. Revert-probes on
the release order and on setting the fence each failed their cases.

### <a id="m6-7"></a>M6-7 -- The admission gate and the reader and writer tiers, with ordinary attachment and the upgrade taking the gate, then the reader tier when unsafe for readers, then the writer tier, joining `Admission` to `Coordination`. *(completed 2026-11-13 22:15:18 -05:00)*

The decision is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "The admission gate and
the tiers are locks in two files, held in the participant's table of holds". The cases cover
readers and writers attaching together, the gate free after attachment, a reader-safe upgrade
admitting readers and holding off writers, a reader-unsafe upgrade waiting for every reader and
admitting no one, only the unsafe upgrade keeping the gate, a refused upgrade holding nothing, an
upgrade let in by the last reader leaving, two upgraders taking turns, a writer arriving during an
unsafe upgrade waiting at the gate, a writer waiting inside the gate beside an unsafe upgrader
without deadlock, the deaths of an attachment and of an upgrader, and admission reached through
`dyn Coordination`, over both implementations and every backend; and, across processes, a reader
holding off an unsafe upgrade and an unsafe upgrade holding off attachment until each is killed, on
Windows and Linux. Revert-probes on the tier order, on each release of the gate, and on the
upgrade's release of its shared tiers each failed their cases. The overlapping-tries case of
writer exclusion now runs each contender until it has held the lock once, since on Linux a
contender could lose every one of a fixed number of tries.

### <a id="m6-8"></a>M6-8 -- The registry of attached participants: entries claimed and probed as tries, registration and unregistration around the tiers, an upgrade's conversion included, and the overflow record discarded by a host restart, joining `Registry` to `Coordination`. *(completed 2026-11-13 22:29:29 -05:00)*

The decision is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "The registry is a pair of
lock bytes and a record file per entry, every lock taken as a try". The cases cover listing with
tiers and payload, an empty registry, re-registering and an update refused while inspected, a
detached entry cleared, a dead participant's entry never listed and reused after each of several
deaths, a reuser paused between its liveness lock and its token -- by holding its locks, and by a
listing run during the reuser's own write -- a registrant moving on from a claimed entry and from a
live one, an owner unable to claim its entry leaving it dead, an unreadable live record counted
unknown, every entry held attaching unregistered with the overflow kept until a host restart, an
entry cleared while its tiers are held at a drop and at a conversion, the conversion removing the
entry and the upgrade registering anew, a cancelled upgrade, a listing past an upgrade holding the
gate, abandonment, simultaneous listings, a churn of deaths beside a listing, and the registry
reached through `dyn Coordination`, over both implementations and every backend; and, across
processes, an entry listed until its process is killed and then reused, on Windows and Linux.
Revert-probes on the listing's claim, the registrant's claim, the order of leaving, the conversion
and the overflow record each failed their cases.

### <a id="m6-9"></a>M6-9 -- Notification when the count moves or a deadline passes, delivered by a ring the registrant supplies, from one poller per process sampling each registration with its own backoff, joining `CountNotification` to `Coordination`. *(completed 2026-11-13 23:19:41 -05:00)*

The decision is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "One poller per process
samples the count for every registration, each with its own backoff". The cases cover a ring when
the count moves, at once when it already differs, at the deadline and not before, exactly once, never
after the notice is dropped, many registrations on many participants each ringing for its own
reason, no sampling while nothing is outstanding or after a registration has rung and a bounded
number while one is, a ring that panics, an abandoned participant, and notification reached through
`dyn Coordination`, over both implementations and every backend; and, across processes, a count
raised in another process ringing a registration in this one, on Windows and Linux. Revert-probes on
the backoff, the count comparison, withdrawal, removal after ringing and the panic guard each failed
their cases; the removal probe first survived, and the sampling case was extended to catch it.

## Moved 2026-11-13 -- Naming the store (M7 of [CHECKLIST.md](CHECKLIST.md))

### <a id="m7-1"></a>M7-1 -- Rename "Shire" to "store" in code and live documentation, keeping the marker's file name and serialized key until the format advance. *(completed 2026-11-13 23:42:11 -05:00)*

The decision is in [canopy's DESIGN-NOTES.md](DESIGN-NOTES.md) "A store is called a store". The
rename covers `StoreId` and its accessors, `StoreError::EmptyStoreId` and `NotAStore` with their
messages, the docs and comments that called a store root a Shire, komorebi's `store` field and
`open_store` phase, every test root and identity, and the example in
[copilot-instructions.md](../../.github/copilot-instructions.md). The genesis keeps serializing the
identity as `shire_id`, by a serde rename on the one field, so a root written before and after
opens in either build; a case reads the marker back to pin it, and removing the rename failed it
and the cases that open markers carrying the key. The marker's file name is unchanged.

### <a id="m7-2"></a>M7-2 -- Advance the genesis to version 7 as the barrier, migrate a version-6 root to the coordination layer's format, and name the marker `.store` with the identity under `store_id`. *(completed 2026-11-14 00:15:01 -05:00)*

The decision is in [canopy's DESIGN-NOTES.md](DESIGN-NOTES.md) "A root is migrated to the coordination
layer's format"; acadia's `Creation::reserve` is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md)
"A root that already holds artifacts is reserved without creating any". Cases: init records the
implementation before anything else, an interrupted init finished by an identical init and refused to
any other, a root reserved for another creation, a root holding the previous marker, an unmigrated root
refused by `open`, a migration that leaves the new names only and survives a power loss, other versions
unsupported, a directory that is not a store, a held root, an interruption at each of ten durable
steps with and without a power loss (`open` refused while the record exists, the next run finishing
it), a forged record, a record for another implementation, a root reserved by another creation, a
garbage record, the root read again under the lock by `migrate` and by `open`, and two migrations at
once. Revert-probes that each failed their cases: publishing the new marker before the implementation
record, removing the old marker before publishing the new one, not writing the record, accepting any
implementation on resume, not reading again under the lock, `open` ignoring the record, and `init`
reserving after its trees; and acadia's four for `reserve`. Confirming the implementation record on
`open` came with M7-3, which gave `open` a coordination handle.

### <a id="m7-3"></a>M7-3 -- Move the modification count into the coordination state, add a lock-free read-only open, and test both across real processes on Windows and on Linux under WSL. *(completed 2026-11-14 01:03:02 -05:00)*

The decision is in [canopy's DESIGN-NOTES.md](DESIGN-NOTES.md) "The modification count is the
coordination state's". The engineer chose to pull the lock-free read-only open (`Store::open_read_only`,
with `StoreError::ReadOnly`) into this item, because a reader in another process had no handle to
sample through while `open` held the root's lock for the handle's lifetime. Cases, in process: a reader
opening beside a writer and several readers beside one; reads of twelve documents and of a later
version; the count a reader samples equal to the writer's after each of ten writes; odd at every
namespace rename of one write; a snapshot taken inside a write contended and one after it settled; every
write verb refused to a reader with the count, the documents and a pending publication unchanged; an
unresolved transaction read odd by a reader that recovers nothing and settled by the next writer's open;
lost coordination state read odd until a writer vouches for it; the count kept across opens; clones
sharing one count; a directory that is not a store, a root no creation made, and a root not yet
migrated each refused; and an opening and a closing raise that cannot be published. Across processes: a
fixture writer (`canopy-store-writer`) read from another process while paused part-way through a write,
after finishing, after being killed outright, by a successor writer process, by several readers, and
over four successive writers, on Windows and on Linux under WSL. Revert-probes that each failed their
cases: no raise in `begin_modification`, an unresolved end closed as settled, a closing raise that
does not mark the handle, no completed recovery after `open`, no recovery purpose during `open`, no
refusal in `writable`, no confirmation in the read-only open, and no completed recovery after `init`.

### <a id="m7-4"></a>M7-4 -- Make `Store::open` change no store data and report what it found, with discovery, explicit resolving, an incremental resumable recovery run and a convenience that runs to completion. *(completed 2026-11-14 07:51:56 -05:00)*

The decision is in [canopy's DESIGN-NOTES.md](DESIGN-NOTES.md) "Recovery is a run the caller drives";
acadia's `RecoveryDiscovery` is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "Whether
recovery is required is observed with the writer lock tried". The engineer set the shape: leftover
work belongs to a hold of writer exclusion, not to an open handle, so the first opener, the nth
opener and a quiesced store with several handles attached differ only in whether the coordination
state can be vouched for; `Store::open` keeps acquiring exclusion until M9 makes acquisition per
modification, changes no store data, and returns a `RecoveryNeed` beside the handle; *resolve*
(`Store::resolve`) finishes the root's work list -- directory-sync record, publication, commit log
and claims -- and completes the recovery when the state was vouched, and *recover* is the general
case. Cases in acadia, on both implementations and every backend: a root with no state, a vouched
store nobody holds, a live holder's transaction not reported, a dead holder's record reported,
discovery leaving the next acquirer to be told and the generation unadvanced, a fence reported held
and free and not cleared by asking, a recovery in progress reported until it completes, a holder
asking about itself, untrusted state, a host restart, a discovery that finds the lock held not
waiting, an abandoned participant and a failing filesystem. Cases in canopy: opening changing no
store data and requiring recovery, a handle refusing writes and still reading until recovery
completes, a root that needs none, recovery in place after an unresolved transaction, discovery by a
reader and by a writer, a live writer's write never reported as abandoned, lost state reported and
vouched for, an open reporting a failed transaction and resolving making it writable, resolving a
begun transaction back and a prepared one forward, untrusted state and a host restart needing an
explicit recovery, a clean root opening with nothing found, each kind of work left behind reported
on opening without requiring recovery and finished by resolving, a pending publication reported and
resolved, a reader and a writer inside its own operation reporting that they cannot tell, resolving
a store with nothing left changing nothing, resolving untrusted state leaving recovery required, the
phases and totals reported in order, a finished run unchanged by a step, a run stopped at each of ten
points finished by a later run and by resolving after the next open, a run stopped before completion
leaving the fence, a failed step retried, the directory-sync, publication and claims phases, a reader
refused recovery, and clones sharing a recovery. Across processes, on Windows and on Linux under
WSL: a live writer not reported, and a killed writer reported, its successor told on opening that
the holder died and left work, and recovered. Revert-probes that each failed their cases: acadia's
discovery trusting the record whatever the lock says, not reading the fence, clearing the fence, and
ignoring the restart state; canopy's completion skipping the coordination state's recovery, a phase
order that completes before the transactions, a step not marked recovery's, a directory record never
emptied, a refusal never lifted, an open that does not mark the handle, resolving that never
completes a vouched recovery, resolving that completes one nothing vouched for, an open that
resolves, a work list that ignores claims, and resolving that keeps claims. The permission-table
check is queued in M7-5 of [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-11-14 -- M7 permission-table coverage

### <a id="m7-5"></a>M7-5 -- Check the M7 permission table for recovery-required and read-only handles, including repair and scrub verbs. *(completed 2026-11-14 08:02:51 -05:00)*

The recovery-fenced writer case checks that `repair_blob`, `repair_pointer`, `scrub_content`, `repair_reported_damage` and `quiesce` return `StoreError::RecoveryRequired` without changing the store tree or commit log. The read-only write-refusal case covers direct blob and pointer repairs and retains its unchanged-state assertions. Post-migration and post-crash tests call `recover()` before relying on data from the reopened handle. The permission contract is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Several processes may open one store" and "Recovery is a run the caller drives".

### <a id="m7-6"></a>M7-6 -- A snapshot waits on the coordination notification and resamples until a store-option deadline, rather than retrying a fixed number of times after a yield. *(completed 2026-11-14 12:31:30 -05:00)*

`Store::snapshot` samples the count, runs its read only against an even count, and accepts the run only if the count is unchanged afterwards. When a write is in progress it registers a notification on the sampled count and the one deadline, waits for it, and samples again; a notification is a hint, so every wake resamples. `StoreOptions::with_snapshot_timeout` sets the deadline, defaulting to `DEFAULT_SNAPSHOT_TIMEOUT` (one second), which exceeds a write's tail hold. `StoreError::SnapshotContended` reports a deadline that passed, with the number of reads run. A store that needs recovery is still refused at once. Tests cover a snapshot that waits out a held transaction and reads its final state, one held off past a short deadline reporting contention, and a snapshot disturbed by a writer on every read. The decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "How long the contract makes a caller wait" and "A change to the coordination state is delivered as a notification".

### <a id="m7-7"></a>M7-7 -- A Windows count notification notified by the operating system through an oplock on the published count, with the poller kept as the fallback. *(completed 2026-11-14 12:50:28 -05:00)*

A rename over the published count was confirmed to break a read-and-handle-caching oplock on it and not a read oplock; the break holds the renaming writer until the holder acknowledges it. The registration takes the lock on an overlapped handle bound to the thread pool, acknowledges the break in the completion, samples on pool timers until the count differs, and rings; the deadline is a pool timer, so a waiting registration holds no thread and samples nothing. Where the lock is not granted, the filesystem is not the system's, no count file exists yet, or the count moved while registering, the poller takes the registration. `FileFactory::polled` makes a factory poll alone, because a holder that cannot answer a break delays a writer. The conformance suite's notification cases run against both forms, and Windows cases show a break ringing a registration with no reads of the count while it waited and no poller thread, a writer not held up by registrations, no wake lost while a writer publishes without pause, and a notice dropped from its own ring. The decision is in [acadia's DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "A count notification on Windows is a lock the system breaks".

## Moved 2026-11-14 -- Agent metadata and format barrier

### <a id="m8-1"></a>M8-1 -- Add the genesis v8 agent and schema barrier, explicit agent inputs, identity refusal and resumable v6/v7 migration. *(completed 2026-11-14 13:13:57 -05:00)*

`StoreOptions` requires an explicit `Agent`. Creation records its identifier, software version and
description with the current and oldest-reader schemas of both layers in the version-8 marker.
Both open modes refuse a different agent identity and leave recorded metadata unchanged. Creation
reservations bind every agent input; migrations from versions 6 and 7 bind retries to the complete
agent and target marker. A version-7 migration preserves the coordination reservation rather than
trying to reserve the root again with different creation inputs. Tests exercise missing metadata,
reader-floor validation, both source formats, changed-input refusal and migration interruptions
with and without simulated power loss. The decision is in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Agent metadata is behind the genesis version-8 barrier".

## Moved 2026-11-14 -- Snapshot timer lint conformance

### <a id="timer-lint-1"></a>TIMER-LINT-1 -- Apply the timer lint policy to snapshot waits. *(completed 2026-11-14 13:17:17 -05:00)*

The production notification wait carries the call-local `expect` required by
[clippy.toml](../../clippy.toml), explaining that the snapshot deadline bounds a lost notification.
Snapshot tests use `test_timers::mpsc::recv_timeout`; the production wait and its deadline behavior
are unchanged.

## Moved 2026-11-14 -- Store attachment and schema admission

### <a id="m8-2"></a>M8-2 -- Attach and admit every store handle under shared tiers, with a best-effort registry entry retained through its last clone. *(completed 2026-11-14 14:09:16 -05:00)*

Creation and both open modes retain acadia attachments in clone-shared coordination state. The
marker is reread and each schema layer is checked under the tiers, before registration and writer
exclusion. Writers require the current schema; readers may speak any schema from the reader floor
through current. Refused handles leave no entry or tier hold, and final teardown unregisters before
releasing tiers. Registry payloads contain named encoding tags, the agent, spoken schemas, tiers,
process ID and draining state; ordinary operations do not rewrite them.

Tests exercise admission boundaries on both layers, registration and final-clone lifetime, marker
changes during an admission wait, mandatory attachment failure, registry exhaustion, failed
registration, and unregister-before-release ordering. Acadia's first-registration error path
attempts to mark possible incompleteness without replacing the original error or invalidating
the attachment. The decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Store admission holds
shared tiers for the handle's lifetime" and acadia's [DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md)
"A failed first registration records possible incompleteness".

## Moved 2026-11-14 -- Independent agent-version stamps

### <a id="m8-3"></a>M8-3 -- Stamp the agent version after successful mutations with generation-keyed caching and an independent WriteOutcome stamp result. *(completed 2026-11-14 14:39:43 -05:00)*

Public mutations return `WriteOutcome<T>`, retaining their successful value beside an independent
agent-stamp result. Stamps run under the operation's exclusion, compare the recorded version using
a generation-keyed cache, preserve other marker metadata, and advance the cache only after durable
publication succeeds. A stamp failure does not fail the original mutation or require recovery.
Rejected transactions and no-ops skip stamping; repairs, scrub documents, backlog work, resolving,
recovery units and quiescence include successful maintenance work without extending scrub exclusion.
Implicit settlement of a nonempty publication record is included.

Tests cover successive agent versions, unchanged-version I/O avoidance, failed and rejected writes,
publication failures before and after rename, durable retry, stale-generation reload, preservation
of unrelated metadata, repairs and empty or productive maintenance runs. Existing test callers
consume the new receipts, and the public example distinguishes the operation from its stamp.
The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Successful mutations stamp the agent version
independently".

## Moved 2026-11-14 -- Store management and diagnosis

### <a id="m8-4"></a>M8-4 -- Add fresh store properties, durable description updates, typed attachment listings and a tier-free diagnostic view. *(completed 2026-11-14 15:04:20 -05:00)*

`Store::properties` reads the current marker and reports the agent, schemas, reader floors,
coordination identity and store layout. `set_agent_description` durably changes only the description
before independently stamping the agent version, and preserves retry durability after a visible
rename whose directory sync failed. Read-only and recovery-fenced handles refuse the mutation.

`attached_handles` decodes canopy's registry payload while retaining unknown entries and possible
incompleteness. `DiagnosticView` offers properties, listings and recovery discovery without an agent,
tier hold, registry entry or document access. Tests exercise these calls through writers, readers
and diagnostic views, escaped payloads, unsupported payload versions, registry exhaustion, failures
and a held admission gate. Compile-fail documentation excludes document reads and description
mutation from the diagnostic surface. The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md)
"Store management and diagnosis do not borrow document access".

## Moved 2026-11-14 -- Operation-owned writer exclusion

### <a id="m9-1"></a>M9-1 -- Make writer exclusion operation-owned, with shared fencing, generation-aware durability and guarded discovery. *(completed 2026-11-14 15:34:58 -05:00)*

Writer attachments retain their tiers while outermost operations acquire and release coordination
exclusion. Nested entries share that hold, including guards dropped out of order. The local gate
serializes clones; independent handles and processes take turns through the contract. Acquisitions
refuse shared recovery state before effects, and another handle's recovery lifts refusal.

Found and advanced generations distinguish consecutive own holds from an intervening holder,
invalidating durability knowledge and agent metadata only in the latter case. Stamps remain inside
the operation. Scrub and backlog claims end with their current hold, so an intervening resolver
cannot release a live operation's claim. Recovery steps refresh phase records after another holder
and restart on a changed modification count, preventing a paused completion from clearing a newer
transaction's log.

Opening performs discovery without acquiring exclusion. Acadia's read-only inspection hook keeps
root-work-list reads inside a free-exclusion probe; busy observations leave the work list unknown.
Creation reserves its inputs and rechecks under exclusion; identical concurrent creators return
`AlreadyInitialized` once one has completed. Offline migration alone retains the legacy root lock.

Regression tests exercise alternating writers, shared fences, interleaved recovery, stale completion,
agent-version changes, consecutive-hold cache reuse, scrub claim cleanup, concurrent creation and
probe exclusion. Existing process fixtures explicitly hold coordination exclusion and distinguish
attachment from a live write and from recovery after process death. The decision is in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Writer exclusion belongs to an operation"; rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#operation-owned-exclusion).

## Moved 2026-11-14 -- Writer and recovery process coverage

### <a id="m9-2"></a>M9-2 -- Exercise writer exclusion, recovery and discovery across Windows and Linux processes. *(completed 2026-11-14 16:23:22 -05:00)*

The command-driven writer fixture interleaves logged and unlogged mutations across attached
processes, checks independent agent stamps, and reports explicit checkpoints before effects,
after one namespace replica has moved and after document effects before exclusion is released.
The death matrix verifies actual replica divergence at the publication and transaction checkpoints,
shared refusal before recovery, and successful convergence driven explicitly by a survivor.

Recovery can be stopped after a unit of every phase and resumed by another process, with two
processes also driving the same run concurrently. A completed recovery's released attachment is
distinguished from unfinished recovery: killing after completion does not manufacture a fence.
Other scenarios cover a failed transaction in a live process, a reset waiting behind another
transaction, a scrub beside another process's resolver, version stamps and registry membership,
same-process OS-backed handles, and refusal through a genuinely distinct coordination factory.
That factory is exposed only through acadia's opt-in test support.

The suite executes on both Windows and Linux kernels. The WSL cargo bridge is an opt-in Rustup
toolchain selected by the cargo MCP tools, preserving the project compiler and separating artifacts.
Its tests cover path conversion, unsupported roots and argument quoting. Damage paths use
validated names so canonicalization cannot be hidden by Windows's case-insensitive filesystem.
The process decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Process tests stop at protocol
boundaries" and [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#process-test-barriers); the WSL workflow is
in [copilot-instructions.md](../../.github/copilot-instructions.md#running-tests-on-linux-via-wsl).

> **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> `M9` -> `M9-3`
> (leases and the upgrade drain). See [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-11-14 -- Attachment leases and the upgrade drain

### <a id="m9-3"></a>M9-3 -- Lease attachment work and add a cancellable, tier-retaining upgrade drain with caller-context refusal. *(completed 2026-11-14 17:04:23 -05:00)*

Attachment-bound operations hold leases across reads, callbacks, writer waits, repairs, management
calls and whole scrub or recovery convenience passes. Caller-driven recovery leases each unit.
Nested calls continue work already admitted, while a drain refuses new work through every clone.
Leases retain the attachment independently of external store-clone lifetime.

`Store::drain_upgrade` returns a guard that retains shared tiers, waits for operations to end,
and cancels on explicit request or drop. Cancellation wakes waiters and leaves the attachment
usable; stale guards cannot cancel a later drain. Caller context spans synchronous operations
across attachments, preventing a callback from waiting for another handle's work blocked by the
caller's own writer hold. Other clones' operations are drained rather than refused as callers.

The existing registry payload records the current draining state without holding operation
admission or agent state across registry I/O. Initial update errors are retained on the guard;
cancellation reports an update failure without undoing operation admission. Recovery-status queries
and observer installation are fallible so a draining attachment can refuse them explicitly.

Regression tests pin work-family refusal before callbacks, nested completion, queued writers,
read and write callback self-refusal, cross-attachment context, cancellation wakeup, guard identity,
retained tiers, registry transitions and failures, stalled registry I/O, and context cleanup on
unwind. The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Attachment operations are leased
and drains are cancellable"; rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#attachment-lease-context).

## Moved 2026-11-14 -- Upgrade conversion and reader compatibility

### <a id="m9-4"></a>M9-4 -- Convert upgrade tiers incrementally, enforce cumulative reader compatibility and restore detached handles through ordinary admission. *(completed 2026-11-14 18:33:06 -05:00)*

After the drain, conversion releases shared attachment before acquiring gate, unsafe reader tier
and writer tier in order. Pending polls wait on no lock, and cancellation releases partial holds.
The source remains detached until ordinary reattachment rechecks every layer and installs new
agent, nonce and access options. The blocking coordination baseline retains its separate native
wait and reader-reopening contract.

Reader safety is judged against the retained oldest reader, not the current schema. A safe upgrade
admits existing and target readers after its atomic marker announcement; unrelated layers retain
ordinary admission. Unsafe conversion drains all readers and sets the completed floor to target.
Schema-only completion preserves other marker fields and independently stamps the performing agent.
Publication, cancellation and recovery preserve the target-reader admission guarantee: once an
announcement is visible it is fenced for explicit recovery, never rolled back beneath target readers.
Migration bodies remain the following work item.

The prepared schema-only guard holds exclusion across finalization while already holding the
writer tier exclusively, and this exception is recorded explicitly. Recovery finishes the schema
descriptor after journal work and claims, before clearing its fence. Properties expose the active
request and the registry records held upgrade tiers best-effort.

Tests exercise source and target readers, consecutive safe upgrades, cumulative unsafe floors,
layer isolation, metadata preservation, changed-source refusal, partial acquisition cancellation,
diagnostic exclusion contention, interrupted announcements, source reattachment, numeric schema
boundaries and independent stamp failure. The decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Schema-only upgrades convert tiers and publish admission" and
acadia's [DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md) "Incremental upgrade conversion retains the
admission gate"; rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#schema-conversion-and-announcements).

## Moved 2026-11-14 -- Durable caller-driven migration bodies

### <a id="m9-5"></a>M9-5 -- Drive idempotent migration units from durable intent and resume them after journal settlement behind the v9 barrier. *(completed 2026-11-14 19:35:24 -05:00)*

The migration engine declares unit count before intent publication, holds exclusion across calls
under the exclusive writer tier, and exposes a thread-bound unit capability. Units settle journal
work before publishing their durable cursor, report independent stamp failures, and fence failures,
cancellation or panics without losing intent. Finalization refuses unfinished bodies, advances the
schema durably and clears intent only afterwards. Schema-only upgrades retain their separate path.

Intent-aware admission refuses source-only writers even between intent and marker publication.
Recovery exposes the upgrade-body callback after ordinary journal and claim work, and refuses
missing units rather than pretending recovery completed. Cursor reload, refusal restoration and
callback-control checks protect interrupted and competing recoveries. The version-9 marker barrier
excludes earlier recovery code; offline version-8 migration preserves its metadata.

Tests pin interrupted cursors, failed intent and cursor writes, post-schema intent cleanup,
transaction settlement before replay, missing callback refusal, panic cleanup, recursive-control
refusal, recovery-required start and the metadata-preserving barrier migration. The design is in
[DESIGN-NOTES.md](DESIGN-NOTES.md) "Caller-driven migration units have durable intent"; rationale is
in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#durable-migration-unit-cursors).

Concrete store-owned Canopy units remain a separate blocked obligation in
[CHECKLIST.md](CHECKLIST.md): the engineer chose to reject undefined transitions until a concrete
schema-change specification exists, rather than invent a migration. That is not recorded as done.

> **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy-lock-holder` -> `M9` ->
> `M9-6` (schema upgrade process coverage). See [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-11-14 -- Schema upgrade process coverage

### <a id="m9-6"></a>M9-6 -- Verify schema upgrade interruption and admission across Windows and Linux processes. *(completed 2026-11-14 19:57:03 -05:00)*

Schema-aware writer fixtures declare source, target, reader floor and units, expose parent-driven
conversion and body checkpoints, and explicitly resume through target-speaking processes. Unit
transactions migrate document batches with stable names and values, so an interrupted cursor is
checked against actual data rather than only metadata progression.

Tests kill upgraders after intent, between units, after the final unit and after schema publication
but before intent removal. Target recovery converges the schema and data, clears intent and permits
a subsequent ordinary write; source-only processes are refused. A post-transaction callback failure
is tested separately from process death, leaving a fence and intent without an abandoned owner.

Other scenarios retain live writer blockers and readers of every admissible schema, identify them
through the registry, and verify killed processes cease being listed. An acknowledged reader gate
attempt remains blocked until unsafe completion, then is refused by the new reader floor. Recovery
required state refuses an upgrader before intent or marker changes. Competing upgraders release
shared tiers and finish or refuse without a tier/gate deadlock.

The suite executes on the Windows and Linux kernels through the native fixture and the opt-in WSL
cargo bridge. Its contract is in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Upgrade boundaries are checked
across processes"; rationale is in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#upgrade-process-checkpoints), and fixture commands are in
[README.md](../canopy-lock-holder/README.md#upgrade-commands).

## Moved 2026-11-14 -- Bounded multi-process contention comparison

### <a id="m9-7"></a>M9-7 -- Compare contended writes, snapshots, publication and upgrade drains across Windows and Linux with explicit bounded-evidence limits. *(completed 2026-11-14 22:06:02 -05:00)*

The benchmark drives actual logged transactions, cooperative bursts, paired metadata arrival
schedules, armed count notifications and safe/unsafe attachment drains through real processes.
Acquisition generations identify bypass; an injected prepared-transaction failure exercises
explicit recovery independently of rare perturbation sampling. Raw and reduced reports retain
unfinished work, unknown tails and censored waits rather than treating them as successful budgets.

The engineer approved bounded windows with the original workload inputs, not shortened lifetimes
or a claim of equivalent stationary evidence. Windows notification-mode measurements expose
publication and snapshot delay beside readers. The Linux kernel executes the same table over the
Windows-backed WSL mount; its longer holds expose capacity limits, not native-ext4 performance.
Neither platform provides a supported stationary contradiction that would reopen the unordered
baseline. Subsequent native-filesystem work has a separate Linux checkout rather than overwriting
the Windows worktree or relabeling the completed run.

The method and both comparisons are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "Bounded process
measurements retain censoring", "Windows finite-window comparison" and "Linux finite-window
comparison"; their rationale is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#bounded-process-evidence).
Commands and input tables are in [README.md](../canopy-lock-holder/README.md#bounded-contention-campaign).

> **CROSS-COMPONENT HANDOFF:** next work is in component `crates/acadia` -> `M10` -> `M10-1`
> (shared-memory coordination state). See [CHECKLIST.md](CHECKLIST.md#m10----a-shared-memory-coordination-implementation).

## Moved 2026-11-14 -- Shared-memory state epochs

### <a id="m10-1"></a>M10-1 -- Create shared-state epochs with explicit trust, liveness, restart recognition and simulated abandonment. *(completed 2026-11-14 23:17:41 -05:00)*

The shared backend's state is an unnamed file-backed mapping whose shared liveness proves whether
an epoch still has participants. Setup remains held through initialization and the transition to
a shared liveness hold. Last-participant loss, changed/unknown boot and inactive corrupt metadata
create an untrusted recovery-fenced epoch; live corrupt metadata is refused without repair.

The MemFs model shares atomic words by immutable instance and simulated boot and distinguishes
abandonment from clean bookkeeping. Deterministic tests exercise initialization failures and
restart independently; a killed real process proves a child retains the trusted epoch after its
parent detaches and the next opener cannot trust state after that last process dies. The state
protocol is in acadia's [DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md#shared-coordination-state-belongs-to-a-live-epoch-2026-11-14),
with rationale in [DESIGN-RATIONALE.md](../acadia/DESIGN-RATIONALE.md#shared-state-epochs).

## Moved 2026-11-14 -- Shared writer ownership and recovery evidence

### <a id="m10-2"></a>M10-2 -- Implement shared writer ownership, recovery fences, checked generations and recovery-evidence-preserving release. *(completed 2026-11-14 23:59:41 -05:00)*

Writer holds retain their participant and its shared epoch independently of the original borrow.
The participant's hold table releases locks on abandonment without clearing ownership. Acquisition
and release use acquire/release RMWs on the ownership word; count openings precede effects through
an acquire/release RMW, nesting raises once, and unresolved inner endings preserve the shared
fence and odd parity until completed recovery. Diagnostic probes release through RAII without
advancing generation, including after a panic. Counter exhaustion is a typed refusal, not a wrap.

Focused controls pin two existing file-path failures: an unresolved inner ending must fence all
participants, and failure to publish that fence must retain ownership as recovery evidence after
release. Those failures reproduce before their source repairs. File and shared mechanisms remain
independent rather than being consolidated. A killed native writer leaves its mapped ownership
and odd count visible to the retained peer and the next recovery, separately from last-process
epoch recreation; modeled reboot invalidates old holds before any subsequent effect.

The contract and the named unresolved-modification evidence guarantee are in acadia's
[DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md#shared-writer-holds-preserve-recovery-evidence-2026-11-14),
with rationale in [DESIGN-RATIONALE.md](../acadia/DESIGN-RATIONALE.md#shared-writer-evidence).

## Moved 2026-11-15 -- Shared admission and registry membership

### <a id="m10-3"></a>M10-3 -- Implement shared admission, conversion and registry with one resource lifetime and explicit incomplete membership, including planned M10-4. *(completed 2026-11-15 01:12:31 -05:00)*

The admission and registry interfaces were implemented together because the attachment/upgrade
trait bounds require real `Registrant` behavior. Ordinary gate/reader/writer order, native blocking
conversion and incremental gate/tier polling remain distinct specified paths. Cancellation and
changed-safety refusal release partial holds; safe incremental conversion opens readers only after
explicit admission, while unsafe conversion retains the gate. Registration drops before tiers.

Mapped registry slots carry stable tokens, declared tiers and immutable references to arbitrary
opaque payload bytes. Claim/liveness probes never wait, failed updates retain their old payload,
claimed or malformed live slots are unknown, and first-registration failure or capacity overflow
marks incomplete membership without invalidating attachment. A persistent epoch ordinal prevents
token reuse after last-participant loss; model instances isolate copied filesystem metadata from
live source words. Dead-epoch payload cleanup runs only under proven exclusive liveness.

Focused tests pin stale-token refusal, entry reuse after abandonment, copied-world isolation and
conversion cancellation. A killed registered native reader releases the tier blocking an unsafe
conversion and ceases being listed live. The contract is in acadia's
[DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md#shared-admission-and-registry-retain-one-resource-lifetime-2026-11-15),
with rationale in [DESIGN-RATIONALE.md](../acadia/DESIGN-RATIONALE.md#shared-admission-membership).

<a id="m10-4"></a>Planned M10-4's registry work is included in the coupled admission and membership
implementation above, not deferred or replaced by a registration placeholder.

## Moved 2026-11-15 -- Shared count notification and public factory

### <a id="m10-5"></a>M10-5 -- Expose the complete lazy shared factory, atomic count sampling and bounded one-shot notifications with common public-contract coverage. *(completed 2026-11-15 01:31:27 -05:00)*

The public factory opens without mutation, checks reservation identity before initializing shared
state, and supplies its own resumable creation and confirmation path. Owned creation locks release
on abandonment, while a callback that abandons its participant cannot publish a later artifact.
Native sampling is one acquire/release count RMW without a writer lock or composite record.
One process-wide shared poller provides independent bounded backoff, one-shot change/deadline/error
callbacks and lifetime cancellation, containing callback panics outside its mutex.

Durably reserved generation ranges prevent reuse after the last participant exits while preserving
mapped ordinary handoff. Common public-interface cases exercise the canonical file, alternate file
and shared implementations over MemFs, OsFs and FaultyFs. A native child publication changes its
parent's mapped count and rings a notice; the checked count, not the ring alone, establishes the
publication. A failed-update control exposed the file registry's torn in-place update and pins its
source repair through atomic replacement under the existing claim lock.

The specification and atomic registry update guarantee are in acadia's
[DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md#shared-count-sampling-and-notifications-expose-the-whole-contract-2026-11-15),
with rationale in [DESIGN-RATIONALE.md](../acadia/DESIGN-RATIONALE.md#shared-whole-contract).

> **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy-lock-holder` -> `M10` ->
> `M10-6` (sampling/handoff comparison and mixed-implementation process checks). See
> [CHECKLIST.md](CHECKLIST.md#m10----a-shared-memory-coordination-implementation).

## Moved 2026-11-15 -- Native coordination cost and mixed implementation checks

### <a id="m10-6"></a>M10-6 -- Compare file/shared reader sampling and writer handoff on Windows and native Linux and verify mixed-implementation process refusal. *(completed 2026-11-15 07:08:59 -05:00)*

The public-contract benchmark measures batched sampling and closed writer loops in real processes,
alternates backend order, preserves raw request/acquisition/release times, and checks every
generation and count raise. Windows and native ext4 comparisons observe cheaper sampling and
handoff with mapped state without treating those finite closed loops as a fairness guarantee or
as end-to-end document transaction latency. Native source, artifact and report placement is explicit
through the opt-in WSL root override; the mounted-workspace default remains available.

Process fixtures keep default file behavior and explicitly select the shared backend for
interleaved logged/unlogged writes, snapshots, death at durable boundaries and target-schema
recovery of interrupted upgrades. Canonical file and shared processes refuse each other's roots
before changing their reservation. The file and shared paths are retained as deliberately
independent supported implementations, not merged after the speculative path's proof.

The cost finding and retain-both decision are in acadia's
[DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md#coordination-cost-and-independent-backends-2026-11-15),
with rationale in [DESIGN-RATIONALE.md](../acadia/DESIGN-RATIONALE.md#coordination-cost-comparison).
Commands and report semantics are in [README.md](../canopy-lock-holder/README.md#coordination-implementation-comparison).

> **CROSS-COMPONENT HANDOFF:** next work is in component `crates/canopy` -> `M11` -> `M11-1`
> (TLC trace corpus for the conformance harness). See [CHECKLIST.md](CHECKLIST.md#m11----conformance-harness).

## Moved 2026-11-15 -- Matched sealed native wait safety

### <a id="g-2"></a>G-2 -- Lend sealed Windows/Linux native wait capabilities with safe waits and explicitly unsafe raw-target integration. *(completed 2026-11-15 07:36:20 -05:00)*

Windows Doorbell::native_wait_event lends the published family's sealed event and retires the
legacy native_wait_handle API chosen by the engineer. Linux Doorbell::native_wait_fd lends an
owned sealed eventfd with the same safe-code boundary: no safe AsFd/raw-fd/ownership extraction,
opaque mixed poll targets and redacted Debug identity. Its existing native_fd borrow remains
explicitly unsafe for compatible custom integration. Signal owners retain their unsealed APIs;
Linux's lack of wait-only kernel rights is handled by the type seal rather than a claimed mask.

Kernel tests pin level-preserving single/mixed waits, clone lifetime, Windows sealed thread-pool
rearming and Linux sealed epoll-service producer/consumer pumping. Compile-fail doctests reject
safe target extraction and Windows activation handle access. Raw descriptor/handle integration
requires an unsafe no-mutation contract; the recommended Doorbell::watch path remains inside the
sealed protocol. The canonical decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md#native-wait-lending-matches-the-sealed-safety-profile-2026-11-15)
and acadia's [DESIGN-NOTES.md](../acadia/DESIGN-NOTES.md#sealed-native-wait-capabilities-on-both-platforms-2026-11-15),
with rationale in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#native-wait-safety-parity).

## Moved 2026-11-15 -- TLC trace corpus

### <a id="m11-1"></a>M11-1 -- Export TLC behaviours as a JSON trace corpus: every timeline's replay, random walks of the protocol as built, and the named defects' counterexamples. *(completed 2026-11-15 10:07:58 -05:00)*

[export-trace-corpus.ps1](../../tools/export-trace-corpus.ps1) writes `tests/traces/<Spec>/` from
TLC: each timeline's replay with its scenario table's step ids, the longest of a seed's random
walks of the configurations [corpus.psd1](spec/corpus.psd1) lists, and the error traces of the
configurations that reproduce `DANGLING-DURABLE` and `STRANDED-VOTE`. A second whole export
rewrote no file, so the corpus is reproduced from the specs, tables and seeds alone. Scenario
tables gained an optional `Departs`, which the replay generator ignores and the exporter carries
into the corpus. The canonical decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md#the-trace-corpus-2026-11-15) and
[DESIGN-NOTES.md](DESIGN-NOTES.md#replaying-a-trace-against-the-code-2026-11-15), with rationale in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#trace-corpus).

Executing the item restructured the milestone with the engineer: the replay harness became one
item per spec bridge, and the bite check moved to a trace of the protocol as built, since the
model shows the record guard alone prevents `DANGLING-DURABLE`.

## Moved 2026-11-15 -- PointerPublication replay against the store

### <a id="m11-2"></a>M11-2 -- The replay harness and `PointerPublication`'s bridge: every `PointerPublication` trace replays against `Store` on `MemFs` + `FaultyFs`, with the model refined to the store's commit-log handling. *(completed 2026-11-15 11:11:28 -05:00)*

The harness under `src/tests/conformance/` reads the corpus, routes the store's calls through a
recording `Vfs` over `FaultyFs<MemFs>`, groups them into runs, and compares each run's change of
the document's pointer, blob, record and log state, live and durable, with the trace's next
change. Failures and crashes land at the first call of the model's next step once both sides have
made the same changes. Control traces are accepted only where their `departs` step falls in the
range in which the store left them, and tests altering a trace or its mark confirm the harness
refuses each kind of mismatch.

The first replays found the store handling the commit log in four ways the model did not; the
engineer chose to refine [PointerPublication.tla](spec/PointerPublication.tla), every
configuration still came out as its header requires, and T25 was superseded by T31. Canonical
decisions:
[DESIGN-NOTES.md](DESIGN-NOTES.md#the-publication-model-describes-the-stores-commit-log-2026-11-15)
and [DESIGN-NOTES.md](DESIGN-NOTES.md#the-pointerpublication-replay-2026-11-15), with rationale in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#publication-replay).

## Moved 2026-11-15 -- Creation replay against the coordination contract

### <a id="m11-3"></a>M11-3 -- `Creation`'s bridge and the step scheduler: every `Creation` trace replays against acadia's creation contract through both implementations, with the model refined to the code's decisions. *(completed 2026-11-15 14:37:31 -05:00)*

Each model process is a participant of its own -- the file-based implementation or the shared one
-- in a thread of its own over its own simulated process of one `MemFs`, held before the first
call of each step until the replay lets it take that step. The root's reservation, artifacts and
intruder, live and durable, are compared after every step, and every outcome the model gives a
process is compared with what its call returned. A death abandons the process's locks; a crash
settles the model's choice of survivors before the power goes. The exporter gained a `derived`
header field so each trace carries its scenario's roles, implementations and inputs, and the
corpus gained simulations of every creation scenario.

All four control timelines left the model exactly where their broken rule acts, and are marked so.
The simulations found the code deciding in two places otherwise than the model: a creator's
reading under the exclusion, and an opener's order. With the engineer, a scratch copy of the spec
making the code's decisions was model-checked first and held every property; the engineer then
chose to refine [Creation.tla](spec/Creation.tla), whose configurations, replays and probes all
came out as required, and T32 pins the takeover the refinement admits. Executing the item also
found that `Store::init` does not create through the contract, which is queued as `M12-4`.
Canonical decisions:
[DESIGN-NOTES.md](DESIGN-NOTES.md#the-creation-model-describes-the-coordinations-decisions-2026-11-15)
and [DESIGN-NOTES.md](DESIGN-NOTES.md#the-creation-replay-2026-11-15), with rationale in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#creation-decisions) and
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#creation-replay).

## Moved 2026-11-15 -- The contract's writer actions mapped to the code

### <a id="m11-4"></a>M11-4 -- Map `Coordination`'s writer actions to the code, and agree the step hook with the engineer: the inventory, the hook's interface, the abstract write's binding, and the writer-only corpus. *(completed 2026-11-15 18:00:04 -05:00)*

Each writer action's first call in each implementation is recorded, and the actions that make no
call of their own are taken within the step before them. The engineer chose to bind
`Coordination` to acadia's contract with the replay as the contract's client, approved the shared
implementation's step hook as proposed, and moved escapes to the item that binds eligibility and
upgrades. Two generated configurations, `Coordination-Writers` and `Coordination-WritersTxn`, are
writers alone with a death and a host crash; their simulations keep the walk with the most kinds
of action, an exporter option added because their longest walks repeated one recovery loop. The
inventory found that the shared implementation's count publication cannot fail. Canonical
decision: [DESIGN-NOTES.md](DESIGN-NOTES.md#the-contracts-writer-actions-in-the-code-2026-11-15),
with rationale in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#writer-action-binding).

## Moved 2026-11-15 -- The contract's writer actions bound to the code

### <a id="m11-5"></a>M11-5 -- `Coordination`'s writer actions bound to the code: every writer trace replays against acadia's contract through each implementation whose behaviour it is, with the model refined to the code's recovery and crash. *(completed 2026-11-15 21:08:09 -05:00)*

Each model writer is a thread making the client's calls on a participant of its own, held before
each step's first call or, for the shared implementation, its hook's announcement, while a keeper
participant keeps the shared state alive across deaths. The first replay left the code in two
places: `complete_recovery` publishes the count and clears the fence in one call, and the shared
implementation's state rebuilt after a crash is fenced. A scratch copy of the spec making both
held every item-tier configuration, and the engineer chose to refine `RecStop` and `Crash` in
[Coordination.tla](spec/Coordination.tla). Few of the walks were the shared implementation's
behaviour, so the engineer chose two constants, `PublicationsFail` and `CrashFences`, and a
generated `Coordination-WritersShared` configuration whose walks all are. The bridge's self-tests
alter traces -- a count, a told acquisition, a mark -- and each leaves the trace at the altered
step. Verify: `-Filter 'Coordination-Writers*'`, the `ReplayCoordination` and `SharedPublication`
item tiers. Canonical decision:
[DESIGN-NOTES.md](DESIGN-NOTES.md#the-coordination-writer-replay-2026-11-15), with rationale in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#coordination-writer-replay).

## Moved 2026-11-16 -- Shared publication conformance

### <a id="m11-6"></a>M11-6 -- The SharedPublication Store replay binds document and recovery steps to both coordination implementations. *(completed 2026-11-16 14:17:30 UTC-05:00)*

The bridge infers actions from consecutive model states, pauses independent Store processes
at their I/O or shared-atomic boundaries, and compares document, record, directory-debt and
coordination projections after each step. Concrete traces cover file-backed and shared-backed
coordination, live and recovered collection, partial durable clears, and stale recovery runs.
An altered pointer expectation is rejected at the affected transition; removing the recovery
record sync and bypassing staging cleanup are caught by their named model properties.

The store fails unresolved after an unlogged record append. The model retains rolled-back
transaction targets through collection, attempts reset before count publication and fence clear,
and gives each recovery unit a fresh exclusion hold. The checker uses a documented
over-approximation and state view while concrete traces retain the unreduced state. The
experimental Units models and corpus were merged and removed. The full model sweep remains
the M11 milestone gate; this item uses its focused concurrency, reference, control and witness
configurations.

Canonical decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md) "The SharedPublication Store
replay", "State-space reduction for recovery-unit verification", and "Directory-debt units in
the publication composition", with the reduction argument in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#recovery-quotient-argument). An intermittent creation
test observation unrelated to the changed creation code is recorded in
[UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md).

## Moved 2026-11-16 -- Queued client contract

### <a id="m14-0"></a>M14-0 -- The queued client contract, request lifecycle, ambiguity rules and ready-pair scheduling are decided. *(completed 2026-11-16 21:22:21 -05:00)*

The opaque TLA+ boundary owns abstract commands, local request correlation, submission and
completion outcomes, pair lifetime, logical state and recovery state. `started` and pre-start
withdrawal stay internal to the ring; a client sees pair loss as `CompletionUnavailable` and
classifies its effect through the logical and recovery state. Local ready pairs use round-robin
bounded-bypass service; coordination remains unordered below that rule. A fundamental transaction
submit is one-shot, optimistic closure retries are bounded and side-effect-free, and a durable
application token is the mechanism for proving an ambiguous retry.

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md#the-queued-client-contract-is-the-public-model-2026-11-16),
with the alternatives and fairness cost comparison in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md#the-queued-client-contract).

## Moved 2026-11-16 -- Opaque client TLA+ contract

### <a id="m14-1"></a>M14-1 -- The standalone client contract, negative controls and mechanism-free toy dependent are implemented. *(completed 2026-11-16 21:44:32 -05:00)*

[ClientContract.tla](spec/ClientContract.tla) exports only session and pair lifetime, accepted or
refused submission, local request correlation, completion and effect certainty, logical contents
and recovery state. It carries `ClientSpec` and `ClientLiveSpec` separately; request IDs and pair
lifetime bookkeeping are derived from the exported history rather than exposed allocator state.
[ClientContractCheck.tla](spec/ClientContractCheck.tla) owns duplicate-delivery, known-loss,
out-of-order and completion-overfill controls outside the public module.
[ToyClient.tla](spec/ToyClient.tla) substitutes only its logical contents and checks the contract
without importing ring, coordination, publication or filesystem definitions. The configurations
reach both submission refusals, every effect class and the three pair-loss histories, and their
negative controls fail their named client properties.

## Moved 2026-11-16 -- Opaque optimistic transactions

### <a id="m14-2"></a>M14-2 -- Opaque transactions are built outside exclusion and `transform` retries changed reads with a bound. *(completed 2026-11-16 22:13:14 -05:00)*

[transaction.rs](src/transaction.rs) exports an opaque `Transaction`, empty `TransactionOptions`,
and `TransactionBuilder`. A builder reads settled documents without writer exclusion, records a
digest or absence check, and collects writes, removals and node removals; its internal operation
representation remains private. `Store::transact` submits once and returns a committed or rejected
`Commit`; `Store::transform` holds ordinary operation admission, builds on a fresh builder outside
exclusion and retries a changed precondition with bounded attempts and a scheduler yield. Its
closure is documented as rerunnable and side-effect-free. Read, effect, validation, retry-exhaustion
and concurrent lost-update behavior are covered by the transaction and write-surface unit tests.
The public crate documentation describes the builder and optimistic retry path in place of the
old closure-under-exclusion behavior.

## Moved 2026-11-16 -- Ring spec barrier

### <a id="m14-3"></a>M14-3 -- The ring contract, two-doorbell mechanism, round-robin scheduler and controls are modelled. *(completed 2026-11-16 22:54:52 -05:00)*

[RingContract.tla](spec/RingContract.tla) exposes only pair lifetime, local request identity,
accepted or refused submission, completion delivery, and completion unavailability. [Ring.tla](spec/Ring.tla)
models queued pushes, explicit batch notification, completion reservation, one engine consumer,
per-pair FIFO, started versus queued drop behavior, completion publication, and round-robin service
of ready pairs. The submission bell and two completion bells instantiate
[DoorbellContract.tla](spec/DoorbellContract.tla) rather than importing the doorbell mechanism. A
separate fair configuration proves work-taken and waiter-wake properties under explicit drain-or-drop
service assumptions. [RingCheck.tla](spec/RingCheck.tla) keeps overfill, started-entry cancellation,
out-of-order service and lost-wake mutations outside the normal mechanism.

## Moved 2026-11-17 -- In-process sessions and queue pairs

### <a id="m14-4"></a>M14-4 -- Sessions expose asynchronous queue pairs, readiness, bounded waits and parked command execution. *(completed 2026-11-17 00:00:05 -05:00)*

[session.rs](src/ring/session.rs) implements a session-owned ring engine and `Send` but not `Sync`
submission and completion ends. A push reserves completion capacity, batches notify the submission
doorbell, and receives a pair-local `RequestId`; completion delivery carries that identity and is
available through direct take, bounded pump or platform thread-pool watch. Commands cover opaque
transactions, reads, snapshots and enumeration. The engine serves ready pairs round-robin and parks
writes on a modification-count notification when local or cross-process writer exclusion is
unavailable, so an engine worker is not held waiting. Dropping either end closes the pair,
withdraws queued entries, discards unavailable completions and lets already-started work finish.
`Session` read, list, snapshot and transaction helpers submit one command and wait over the same
ring surface. The queue shape and started/drop distinction are held by the M14-3 model's controls
and witnesses; a Loom model checks queue publication through the Doorbell, and the Rust tests cover
backpressure, order, drop, watches, many pairs and no-thread-wait parking.

## Moved 2026-11-17 -- Retire the under-exclusion planning callback

### <a id="m14-5"></a>M14-5 -- The public API no longer exposes `Store::plan_and_apply`. *(completed 2026-11-17 07:11:53 -05:00)*

The under-exclusion caller callback is removed without a compatibility shim. Every in-tree test
or integration scenario now expresses its read-modify-write through opaque `TransactionBuilder`
checks and `Store::transform`, while direct fixed writes remain direct transaction submissions.
Drain admission checks execute through the sanctioned commit observer when they must inspect the
operation's exclusion context. The concurrency race still verifies that paired updates stay atomic
and no increment is lost, and the internal writer-nesting test retains the commit-log settlement
check without exposing an application callback. Current crate and README documentation describe
only the transaction and snapshot surfaces that remain.

## Moved 2026-11-17 -- Decide and implement bounded store primitives

### <a id="m14-6"></a>M14-6 -- Bodies remain opaque and append is the first bounded store-evaluated primitive. *(completed 2026-11-17 07:19:11 -05:00)*

- [x] **M14-6.1** -- Decide the primitive policy from CQ-1 and the opaque-transaction contract.
  Bodies remain unparsed bytes. Numeric `add` and `maximum` operations are not adopted because
  they would make canopy own a width, signedness, byte order and overflow policy. Append is adopted
  as the first format-free primitive; the decision is recorded in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) "Store-evaluated transaction primitives stay byte-oriented"
  and [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) "Byte append is the bounded format-free primitive".

- [x] **M14-6.2** -- Add bounded raw-byte append to `TransactionBuilder` without exposing an
  operation enum. The operation treats absence as empty bytes, runs under writer exclusion, and
  materializes an ordinary swap before prepare. The suffix is bounded by `MAX_APPEND_BYTES` and
  the result by `MAX_APPEND_RESULT_BYTES`; rejection precedes the commit point. Tests cover binary
  bytes, absent and existing documents, atomic multi-target use, exact and over-limit boundaries,
  duplicate and node-removal conflicts, stale-check rejection, concurrent no-loss behavior and
  exactly-once observer delivery of the materialized body.

## Moved 2026-11-17 -- Refine the client contract from ring and store outcomes

### <a id="m14-7"></a>M14-7 -- Ring and store outcomes refine one opaque client history. *(completed 2026-11-17 08:01:00 -05:00)*

[ClientComposition.tla](spec/ClientComposition.tla) projects Ring acceptance, local request identity,
per-pair order, ready-pair service, withdrawal and completion availability into
[ClientContract.tla](spec/ClientContract.tla). A projection step uses request state and pair
liveness only; it contains no `started` field and exposes no doorbell or engine cursor. Store
completion supplies definite-applied, definite-no-effect or uncertain outcome data, logical
contents and recovery state. Pair loss first removes availability; a started operation may later
settle and change contents or require recovery without manufacturing a completion, while the client
continues to see only `CompletionUnavailable` and uncertain effect. [ToyClient.tla](spec/ToyClient.tla)
continues to instantiate the public contract unchanged and retains its properties through the
composition projection. Main and fair refinement configurations cover the contract; controls fail
the promised outcome, order, ambiguity and ready-pair service checks; witnesses reach unavailable
completion and uncertain post-loss settlement; mutation probes catch each realization or projection
promise without a surviving mutant.

## Moved 2026-11-17 -- Check Store against the composed client contract

### <a id="m14-8"></a>M14-8 -- Store sessions are checked against the opaque client projection at the API. *(completed 2026-11-17 08:14:22 -05:00)*

[client_contract.rs](src/tests/conformance/client_contract.rs) keeps a test-only oracle containing
only the public pair, request, completion, effect and recovery projection. It deliberately has no
queue, engine, doorbell or started state. Real `Store::session` calls are mapped into that oracle for
normal commits, precondition rejection, local completion-capacity refusal, pair-closed refusal,
pre-start withdrawal, completion during another pair's parked head, started work after pair loss,
continuously ready competing pairs and independent Store attachments over separate `MemFs`
process views. A pre-prepare append fault completes with definite no effect. A post-prepare rename
fault completes as uncertain, remains recovery-required across a simulated crash, and remains
uncertain after recovery even when a later read sees the applied body. The oracle's own control
rejects out-of-order completion and known-effect pair loss, so the bridge fails rather than silently
accepting either projection error.

## Moved 2026-11-17 -- Document the opaque client contract for dependents

### <a id="m14-9"></a>M14-9 -- Dependents have a complete outcome and retry guide for the client contract. *(completed 2026-11-17 11:38:53 -05:00)*

[README.md](README.md) now states the client-visible meaning and required caller action for accepted
and refused submissions, delivered completion, bounded-wait expiry, pair loss, disturbed reads,
snapshots and recovery-required state. Each promise names the corresponding client-model property:
submission coherence and capacity, exactly-once delivery, per-pair order, unknown effect on pair
loss, and recovery only with uncertainty. The guide explains that a timeout or pair loss is not
cancellation, that `RequestId` is pair-local correlation rather than a durable receipt, and that a
later read cannot universally decide whether an arbitrary uncertain operation ran. Applications
that need that distinction carry an idempotency token or equivalent durable evidence in the same
transaction. It also bounds the ready-pair fairness claim: round-robin prevents cross-pair skipping
but makes no wall-clock or global coordination-order promise. The public session, completion and
error rustdoc repeats these rules at the points a dependent consumes them.
