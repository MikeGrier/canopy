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
