# acadia design rationale

## Discovery inspection

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Recovery discovery lends a
read-only inspection". A caller that reads records after discovery returns cannot know whether
a writer acquired exclusion between those reads. A callback inside the probe preserves the
existing observation boundary without changing ownership, generations or recovery state.
Exposing a bookkeeping-bearing writer hold for diagnostics would instead alter the state being
diagnosed. The inspection is skipped wherever no free probe exists.

## Alternate process factory

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Test support supplies a distinct
file coordination implementation". A process test needs a real second factory to demonstrate
implementation admission, not an injected mismatch error or a hand-forged reservation. The
existing alternate implementation supplies that independent identity and creation exclusion.
Keeping it behind test support leaves the canonical factory's specification unchanged.

## Incremental tier conversion

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Incremental upgrade conversion
retains the admission gate". A native blocking conversion cannot cancel between acquisitions.
An owned polling conversion keeps partial holds explicit and lets dropping it cancel promptly.
Retaining the gate in the result lets the consumer make its schema admission visible before readers
can attach. The blocking API's automatic reader reopening is a distinct contract, and its native
wait path is preserved rather than emulated by polling.

## Shared state epochs

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Shared coordination state belongs to
a live epoch". A global page-file mapping on Windows can require a privilege unavailable to an
ordinary process; a session-local name would silently isolate writers in different sessions.
An unnamed file-backed mapping makes the root's own file identify the same object without either
condition. Because the file survives mapping closure, shared liveness supplies the independent
proof of whether trusted state still has participants rather than inferring it from stored bytes.

Setup must remain held while the exclusive liveness probe becomes a participant's shared hold.
Releasing it between those events lets another initializer reset words that the first participant
is already using. Initialization failures cannot publish trust, and a later inactive retry sets a
recovery fence regardless of how much header data the failed attempt left behind. The boot record
is durable identity evidence rather than a durable copy of the shared counters.

The in-memory model exercises the same lock and epoch rules while replacing a native mapping with
owned atomic storage. The real-process check is still needed: modeled abandonment cannot establish
that the OS releases liveness when the last process dies without a destructor. This duplication
isolates the shared implementation while keeping the established file-based path stable.

## Shared writer evidence

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Shared writer holds preserve recovery
evidence". A hold has to retain its participant independently of the borrow it was acquired through,
because dropping that borrow's owner cannot end a live operation. A participant-owned resource
table makes simulated abandonment release those locks without running a clean release's metadata
updates. The next acquirer, not the dead holder, records the recovery fence.

Nested unresolved work cannot become settled solely because its outer caller reports a settled
ending. The file path's early nested return and its attempt to clear ownership after failed fence
publication both lost the evidence that other participants need. Focused controls reproduce those
failures before their repairs; the shared implementation preserves the same owned guarantee.
Retaining ownership when the filesystem cannot publish a fence trades an unnecessary recovery for
never silently admitting ordinary work over an unresolved operation.

Acquire/release RMWs on the ownership word make the handoff explicit, while an acquire/release
opening count transition orders the effect that follows. No sequentially consistent operation or
assumption about how an OS lock happens to drain a CPU's buffers is needed. Counter exhaustion is
checked before effects because discovering that a closing raise cannot be represented after data
has changed would manufacture an avoidable unresolved operation.

## Shared admission membership

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Shared admission and registry retain
one resource lifetime". The trait bounds couple the planned admission and registry items:
attachments and upgrades are registrants, so an independently exposed tier handle with a dummy
registration result would assert a contract it cannot satisfy. Combining the work preserves the
owned interfaces and their failure semantics rather than treating the temporary missing half as
permission to narrow either feature.

Registry metadata belongs in the mapped state, but payload size belongs to the caller. Fresh
immutable payload files retain that arbitrary byte contract without a fixed-slot size ceiling;
the claim lock makes changing the reference and declared tiers one inspection boundary. Liveness
must begin before publishing the token or a listing could attribute a new participant's lock to
an old record. Failed updates keep the old reference, and failed first registration still marks
diagnostics incomplete because its caller can legitimately retain attachment.

Persistent epoch ordinals distinguish tokens across state recreation, while a newly minted local
model instance distinguishes copied files from the live source's atomic words. An independent
in-memory filesystem can have the same boot counter and copied instance bytes, so neither alone
is a namespace boundary. Cleanup belongs only after proving exclusive liveness: removing an old
payload under a live reader would turn maintenance into a registration failure.

## Shared whole contract

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Shared count sampling and
notifications expose the whole contract". A facet implementation cannot serve the storage engine
until its factory also satisfies creation and admission identity. Lazy initialization preserves
the nonmutating open contract, while separate creation exclusion and atomic reservation publication
arbitrate with other implementations instead of letting incompatible backends coordinate one root.

Acquire/release count RMWs state both sides of the reader's bracket without relying on syscall
side effects or sequentially consistent fences. A polling implementation meets the owned notice
contract when registrations have bounded independent backoff and no thread of their own. Keeping
its own poller isolates the shared path while the file implementation retains its native Windows
notification behavior. A callback is only a hint; native process checks pair it with a changed
mapped count so a deadline alone cannot manufacture publication evidence.

Cold epochs need non-reused generations because a consumer's cached generation must not silently
match a recreated one. Durable range reservation moves that cost to initialization and rare range
renewal, rather than fsyncing each writer handoff. Malformed reservation identity is a refusal
because inventing a new low counter would reuse values whose consumers can still remember.

The common failed-update control applies to each implementation's public registry contract. The
file path's torn in-place record made its live entry unreadable after a failure, where the shared
path retained its old immutable payload. Atomic replacement under the original claim lock repairs
the specified guarantee at the owning layer; no caller workaround or backend consolidation is needed.

## Coordination cost comparison

The canonical finding is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Coordination cost and independent
backends". Measuring raw sampling and handoff isolates the mechanism removed by mapped state;
measuring document transactions alone would let journal and filesystem syncs obscure that cost.
Alternating backend order reduces a fixed warm-cache/order advantage, while checking every
generation and count raise prevents a fast but incomplete loop from being reported as efficient.

Native Linux source, artifacts and benchmark data belong to the native checkout because a kernel
running over a Windows-backed mount is not an ext4 storage comparison. The explicit bridge root
override supplies that checkout without changing the mounted-workspace default or the current
Windows worktree. Replicated closed loops compare the mechanism under those operating conditions;
they do not bound stationary tails or justify a fairness change.

Keeping both implementations is the completed speculative path's deliberate disposition, not an
unreviewed permanent duplication. The file-based platform and its Windows notification path remain
independent baselines, while the shared path retains a distinct identity and conservative epoch
loss/recovery behavior. Mixed-implementation process refusal prevents the consumer from accidentally
coordinating one root with two mechanisms that need not exclude each other.

## Sealed native wait capabilities

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Sealed native wait capabilities on
both platforms". I/O safety keeps a borrowed descriptor open but does not prevent safe code from
reading or writing it. Linux cannot narrow an eventfd duplicate to wait-only kernel rights, so
matching Windows' unsafe profile requires an owned opaque type rather than returning BorrowedFd
from a safe accessor. Even Debug must hide the numeric identity because RawFd is an integer.

Safe waits and a sealed epoll-service watch perform their own internal borrow, while custom
integration uses an explicit unsafe extraction with the no-mutation contract. The duplicate's
lifetime is independent of the original owner, and clones share its ownership. Existing unsealed
owner APIs remain available: sealing the consumer's view does not collapse the platform layer or
pretend it protects against a caller that already owns an unsealed alias.

## The shared implementation's step hook

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "The shared implementation announces
its steps to a test-support hook". canopy's replay could not see the shared implementation's steps
any other way: they change atomic words and make no filesystem call. A hook through which each
step announces itself, compiled only with `test-support`, was chosen over bridging only the
file-based implementation, which would leave the shared one held to no model, and over running
each coordination call whole, which would hide every interleaving within a call. A factory of
hooked participants beside the ordinary one keeps `SharedFactory` a unit value that can be copied,
as its callers use it. The reading of the words takes no lock so that it changes nothing the
replay compares, and in particular holds no liveness, which would keep an epoch alive that the
model lets end.

## A finite path model carries the durability facts MemFs must preserve

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "MemFs durability reference and
replay". The prior differential oracle compared optimized delta directory sync with full
reconciliation, but both paths shared the same in-memory durable-tree bookkeeping. It therefore
could not independently say that a carried directory, an unnamed nested directory or a crash after
one parent sync produced the intended result.

The reference keeps names and bytes as separate facts and assigns file inodes explicitly. That
small abstraction is the mechanism needed by every named defect: a durable name can survive with
empty file data, a rename can leave the same inode under two names, and a directory snapshot can
outlive its path in a live tree. Modeling the whole MemFs implementation as nested Rust maps would
turn the TLA+ file into an implementation copy and make a shared mistake harder to see.

A carried snapshot is retained as flat path-indexed data because the contract question is which
durable entries survive each sync and crash. The scripted scenarios are witnesses for the four
historical failures, while the smaller unrestricted model explores create, write, remove, sync,
rename and crash combinations around the file-alias shape. Controls break one durability promise at
a time, and the Rust bridge checks the same final projections against MemFs while auditing inode
reference counts after every replay step.

## Directory removal belongs in the durability reference

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "MemFs directory removal in the
durability reference". The reference's first form had only an empty-directory removal, and that
action left carried snapshots in place, while `MemFs` drops them on both removals. Recursive
removal was covered by `MemFs` unit tests and by the differential oracle against full directory
reconciliation, but both of those share `MemFs`'s own bookkeeping, so neither could say
independently that a directory recreated after removal starts empty.

Modeling recursive removal as repeated empty-directory removals was rejected: it would hide the
single-step contract that `remove_dir_all` provides and would need a deeper step bound to reach the
same states. The scenarios recreate a directory that arrived by a move because a move is what
leaves a carried snapshot at that path; a directory created in place has none, and its
recreation could not tell whether removal drops snapshots.

## Abandonment waits for a change in progress rather than racing it

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Review fixes: stale staging names
and abandonment during a change". A code review found that `FileCoordination::abandon` released the
participant's locks while one of its operations could still be writing records, so a simulated
death could keep writing after another participant took exclusion -- a state no real crash
produces. Checking liveness before each write was rejected: the check and the write would still
race the abandonment. Holding the table of holds across each individual change, as the shared
implementation holds its guard, closes the window at the cost that an abandonment waits for a
change already running; a replay harness that parks such a change now fails it before abandoning.

Removing a stale staging sibling in `publish_new` was rejected for retrying under a fresh name:
the process id in the name identifies a process only within one process-id namespace, and two
namespaces sharing a filesystem could have a live publisher behind the same name.

## Lock-free sampling beside a serialized abandonment, and MemFs naming fixes

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Second review fixes: lock-free
sampling, MemFs naming and lock checks". A second code review found that serializing abandonment on
the table of holds made `alive()` -- and so every sample -- wait for a change in progress on the
same participant, breaking the sampling contract. Moving the flag out of the table into an atomic
that is written only under the table keeps both properties: checks that must be serialized with
an abandonment read it under the table, and checks that must not wait read it without.

The two MemFs naming defects were found by tracing against `MemFsDurability.tla` and reproduced
before fixing; the model already expressed both shapes, but no scenario or replay exercised them.
Each is now a scenario with a witness and a replay. The MemFs lock-check fix keeps the table for
the operation's duration rather than re-checking after the change, because a check after the
change could only report a violation, not prevent one.

## Unnamed marks carried with a snapshot rather than copied at rename

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "MemFs marks travel with what they
describe; empty lock ranges; the poller retires outside its lock". The second review's fix copied a
moved directory's marks to the destination path when the rename happened. A third review found that
this applies them too early: the destination's durable directory still holds its old contents until
the parent is synced, and a copied mark naming one of those entries unnamed deletes it on a crash
before that sync. A replay of exactly that crash failed against the copying code. Keeping the marks
beside the carried snapshot and adopting them only when the snapshot replaces the durable directory
makes the marks describe the same tree the durable directory does at every step.

A sync that keeps marks only beneath live directories was preferred over clearing marks when a
path is overwritten, because every operation that can put a file where a directory stood would
have to remember to clear them, while the sync that names the new entry already walks the children.

## Finding the store is a success of create

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "An identical creator finds the store
it asked for". Refusing an identical creator with `AlreadyCreated` made every caller that wants a
store to exist treat that error as success, and made the answer depend on timing: a creator that
looked while another was still building got a resume and then the refusal, one that looked a
moment later got the refusal directly. A distinct success outcome lets the caller tell whether it
made the store, which a caller that seeds a new store needs, without either of those costs.

Returning `Existing` from the first reading without waiting for the creation exclusion was chosen
over always waiting. Waiting would serialize every creator behind a slow creation that has already
finished for all a reader of the root can tell, and the marker's being last, with each artifact
durable before the next, is what makes the store complete; the sync of the marker is what protects
a caller from a creator that died before making it durable.

## Recognizing a publication's leftovers

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "A creation's own publication leftovers
are its artifacts'". Canopy chose, with the engineer, a strict root for creation -- nothing but
creation's own entries -- over letting the caller name entries to ignore, which would have added a
predicate to `CreationPlan` and an exception to the model's notion of foreign content. The strict
rule leaves one gap: the staging sibling a crash can leave beside an artifact. Removing leftovers
before reading the root was rejected because the reading is what decides whether the root may be
touched at all; recognizing them as the artifact's keeps the reading read-only.

## Moving rather than linking a publication on Windows

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "A publication is a move on Windows".
Canopy's cross-process creation tests failed in about half of runs once its marker was published
with `publish_new`: the next agent stamp could not rename over it. The engineer's first suspicion
was an open without `FILE_SHARE_DELETE`; none of ours lacks it, and a reproduction with `std::fs`
alone showed the hard link, with its staging name removed, as the trigger. Retrying the replacing
rename was rejected: it would put an unbounded wait into every replacement of a published file to
paper over the publication's shape. The hard link stays on Unix, where the link and unlink are what a
no-replace rename would be and nothing outside the process blocks them;
`renameat2` with `RENAME_NOREPLACE` is the alternative if that changes, and is not portable to
every filesystem a root may live on.

## The fourth 2026-11-17 review

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Fourth review fixes: markers, plan
names, recovery order, foreign roots and MemFs locks". The review was run by a second model over the
whole crate, and the engineer asked for each finding to be verified before it was fixed, with those
that did not verify recorded rather than changed.

- **The marker's publication moved into coordination** rather than tightening the maker's contract
  to require atomic visibility. A maker that must make its own marker atomic must know to use
  `publish_new`, and the contract's reading of a staging sibling as its artifact already belongs to
  coordination; giving the marker its own method lets coordination own both, and a maker cannot get
  it wrong. Waiting for the creation exclusion before trusting a marker was rejected because the
  writer of a torn marker may have died, which no wait repairs.
- **Case folding is the policy for every implementation and backend**, not only on Windows roots. A
  plan is data that a caller may carry to a root on another filesystem, and a plan accepted on one
  platform and refused on another would make portability depend on where it was first tested.
  Simple uppercase folding was chosen over full case folding because it maps one character to one,
  as filesystems that fold case do.
- **The vouched count was made recovery's commit point for trust** rather than reordering
  recovery to publish it last. Publishing it last was tried first, setting the fence and vouching
  the boot record before it, and canopy's replay of
  [Coordination.tla](../canopy/spec/Coordination.tla) refused it: the model never sets a fence in
  recovery, and makes the state trusted and settled in the one step that publishes the count. A
  trust read from the count record gives exactly that step. Undoing the count when a later step
  fails was rejected because an undo can fail too.
- **File participants check the reservation's implementation when they take exclusion or attach**,
  rather than requiring every caller to have called `confirm_created`: the contract's promise of one
  writer per root holds only if no capability can be had without the check. The shared low-level
  constructor was made crate-private for the same reason rather than given a check of its own.
- **The first-sample exception was documented** rather than removed by an explicit initialization
  step, which would add a call every user must make before the contract's cheapest operation, and
  rather than by eager initialization, which would make a shared participant touch a root before it
  could refuse a foreign reservation.
- **MemFs locks were keyed by file identity** because native locks belong to the open file, and a
  reference that keys them by name makes a test pass on `MemFs` that fails on the OS after a rename.
  The cost is a change in what a crash does to a lock on an erased file, which now follows the same
  rule: the lock belongs to the file, and the file is gone.

## The fifth review, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Fifth review fixes: recovery's commit
point, restart repair, foreign roots and the marker under abandonment". Five of its findings showed
fixes of the fourth review that were incomplete, each because the fix had been reasoned about from
one starting state: recovery on a root a dead holder had already fenced, a shared participant
checked when it first opened its state, and a marker made atomic without asking who may still
publish it.

- **Recovery lost its boot-record write** rather than gaining a fence of its own. Setting a fence
  at the start of recovery, so that every later step was covered, was tried on 2026-11-17 and
  refused by canopy's replay of the model, which never sets a fence in recovery. The count record
  already carries the trust, so the boot record's vouching added nothing a reader relied on.
- **The restart repair was reordered** rather than given an explicit "repair in progress" record:
  the record from another boot already is that record, provided it outlives the repair.
- **The marker's publication was handed to each implementation** rather than given a liveness
  check before and after, because only the guard `abandon` itself takes can order a publication
  with it.
- **The simple uppercase mapping is supplied by a short table** rather than a dependency. The
  standard library gives only the full mapping, and the characters whose full mapping is several
  characters are a closed set in which only the Greek letters with ypogegrammeni simple-map to
  another character.
- **`metadata` keeps following links**, because the alternative would make a store root reached
  through a link unusable, which no caller has asked for and canopy relies on.

## The sixth review, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Sixth review fixes: an unidentified
boot, inspection under abandonment, the reported fence, joining an ending epoch".

- **An inspection holds off abandonment** rather than being told afterwards that its participant
  died. The review suggested a lease that abandonment drains; the guards the implementations
  already have for changes in progress are that lease, and reusing them keeps one rule: once
  `abandon` returns, the participant does and observes nothing more. Reporting `Abandoned` after
  the inspection was rejected because the caller has already read records no probe protected.
- **A joiner that finds the model's words gone opens again** rather than treating the root as
  damaged or making fresh words as a joiner. Opening again sends it through the one path that
  decides an epoch, and the attempt cannot repeat without bound, since it repeats only when the
  last participant left, after which the next attempt finds none.
- **A vouched count without a boot is refused at its publication** rather than at recovery's
  start, so every path that would publish one meets the same refusal.

## The seventh review, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Seventh review fixes: shared discovery
probes, the fence as reported, opening under abandonment, the notification poller".

- **A shared probe** was preferred over making a contended probe read the record anyway: a writer
  holding the lock and another discovery holding it are only told apart by a lock that one of them
  takes in another mode, and the writer's exclusive lock already is that mode.
- **The fence word was split from the snapshot's `fenced`** rather than changing the snapshot,
  because the snapshot's meaning -- untrusted state counts as fenced -- is what refuses ordinary
  work, and changing it would have moved that refusal.
- **An opening mutex taken after the setup wait** keeps the design's rule that abandonment never
  waits behind a blocked initializer, which a mutex held across the wait would have broken.

## The eighth review, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Eighth review fixes: a hold from an
ended boot, the recovered count, a missing generation, mutexes free of filesystem calls".

- **The boot check on a file hold mirrors the shared participant's**, which already treats a
  participant over a simulated filesystem from an earlier boot as gone. Checking only the
  participant's abandonment flag would have left the simulated restart unable to end a holder.
- **Reading the boot outside the mutexes, not removing the mutexes,** keeps the rule that
  `abandon` waits for a change in progress; only observations, which need no serialization with
  abandonment, moved out.
- **The opening takes its mutex twice rather than across the setup wait**, which would have made
  `abandon` wait behind another participant's setup.
- **`wait_native` was narrowed rather than given a second, event-only function,** because no
  caller in the repository waits on a non-event handle through it, and keeping the wide form would
  have kept the misuse reachable from safe code.

## Three failures, two events, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Three failures, two events: a panic,
a process ending, a host restart"; the discussion is
[DESIGN-SESSION-2026-11-18-crash-kinds.md](design-sessions/DESIGN-SESSION-2026-11-18-crash-kinds.md).

- **A storage-only crash was never chosen.** "A crash loses storage and keeps locks" was a stance
  taken to make progress when `MemFs` gained locks; a day later the boot identity made the same
  crash a host restart. Together they described a host that restarts while its processes keep
  running, and three findings across the eighth and ninth reviews came from that state alone.
- **Boot checks in each coordination path** were the alternative to fixing `MemFs`. Each new path
  would have been another place to forget one, and the checks would have defended code that is
  correct on every real host against a state only the simulation could reach.
- **Keeping the crashing handle alive** would have spared the callers that reuse it, but would have
  left one view able to act across the restart.
- **Aborting in a guard dropped while unwinding** is literally a process ending, but a library would
  end its host application, and no in-process test could show the behaviour.
- **`std::panic::set_hook` and `compile_error!` under `panic = "unwind"`** were rejected: the hook is
  process-wide and not a library's to take, and the test harness needs unwinding.
- **Treating an unwinding drop as a clean release** would be safe only if every caller bracketed all
  of its work under a hold in modifications, which acadia cannot enforce.

## The ninth review, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Ninth review fixes: restart as process
end, handoff, reservations, carried sync, event and oplock capabilities". All ten findings held.

- **The first two findings were fixed at `MemFs`'s process boundary rather than in the file holds and
  registry.** A waiter and an attachment cross a restart only because the filesystem let a pre-restart
  process value act afterwards. Once `Vfs` says a restart ends that process, every consumer gets the
  rule from its backend and no path needs its own boot comparison.
- **The handoff is under the final opening guard rather than before that guard**, because the
  externally visible state is the `OnceLock`, not the local `SharedState` value. Publishing first
  makes abandonment wait for something it can then end.
- **Generation renewal compares the durable reservation with the mapped limit rather than merely
  accepting any well-formed record.** A valid lower value is still stale, and accepting it would
  reuse generations the running epoch has already handed out.
- **The carried-sync fix starts from the view an ancestor will materialize**, rather than adding a
  rollback around the durable mutation. Resolving the anchor first keeps a valid `fsync` from needing
  transactional cleanup.
- **`wait_native` is narrower than `wait_for_single_object`** by design. acadia's public name is an
  observation of a level signal; a consuming native wait remains an explicit capability in another
  layer.
- **Oplock teardown is provisioned before the oplock.** Starting a worker as a reaction to failure
  made failure of the worker itself part of the safety path. The process-wide worker removes that
  branch: if it cannot be provisioned, native registration is not used.
- **Fault replacement is bounded at the decorator boundary**, where the range's actual result length
  is known, rather than by changing whole-file `read` corruption or by making consumers defend
  against a backend that violates their contract.

## The tenth review, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Tenth review fixes: interrupted
renewal, participant boundaries and platform capabilities". All seven findings held.

- **The one-block-ahead generation reservation is recoverable because it is the only non-equal
  value that proves a completed rename.** Recovery repeats the directory sync and adopts it;
  missing, behind and arbitrary values still establish no such publication.
- **Poison handling moves outside the held-table scope rather than weakening abandonment.** A
  poisoned mutex still ends the participant, but the recovery path must not recursively acquire a
  mutex that belongs to the operation it is unwinding.
- **The participant abandonment flag is shared by the public facade and its shared state**, rather
  than copied or mirrored. This keeps creation authority, writer exclusion, registry and
  admission at one process-lifecycle boundary.
- **Teardown resources are transferred as one registration bundle**, so concurrent cancellation and
  callbacks cannot each believe they own part of the cleanup. A native registration that hands the
  callback to the poller preserves the callback rather than withdrawing it.
- **A sealed capability cannot infer Windows reset mode from the handle alone.** The safe wrapper
  therefore records provenance at construction, where `EventSignal` establishes manual reset.
- **Callback isolation follows the consumer's panic strategy.** `catch_unwind` cannot contain an
  abort, and a library must not replace the consumer's process policy with an unimplementable
  promise.
- **A crash-ended prefix represents an invariant of monotonically allocated process identities.**
  It keeps crash cost independent of how many boots preceded this one; a per-identity set is still
  needed only for death within the current boot.

## The eleventh review, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Eleventh review fixes: immutable
process views and unwind-safe native boundaries". All nine findings held.

- **A crash controller cannot change identity in place.** A mutable identity made correctness depend
  on how many `Arc` aliases happened to exist. Returning a distinct successor makes the process
  boundary explicit at the type boundary and leaves every pre-crash capability refused.
- **Fault decorators restart as decorated process views.** A fresh `MemFs` alone would lose the
  policy state that a test or witness uses, so the specialized successor copies the immutable rules
  and deterministic counters before changing only the wrapped process value.
- **Shared-state abandonment owns creation cleanup through a weak edge.** A strong edge from state
  to participant would create a cycle; a weak edge is enough because an active state always has a
  live public participant while it can own creation resources.
- **Panic cleanup lives at the guard that owns the numeric claim.** A local cleanup catch would
  serialize only one operation and still leave another path possible; the guard makes every unwind
  through registry inspection obey one release/abandonment rule.
- **Callback containment is at the native boundary.** The dependency's foreign trampoline cannot
  let user unwinds cross it, so acadia must both catch at its own scope and treat sample failure as
  a notification outcome. Merely catching the ring callback is too late.
- **A thread-pool wait remains a capability operation on the sealed value.** Exposing a raw or
  reset-mode-blind conversion would preserve less information than acadia owns. Consuming the
  sealed value into a sealed dependency target preserves its manual-reset provenance.
- **Bounded range results come from a bounded slice, not a post-allocation truncation.** Borrowing
  the selected rule removes the policy clone, and slicing the replacement before `to_vec` makes
  the result's allocation proportional to the bytes requested by the undecorated read.
- **Concurrency tests synchronize on entry, not elapsed silence.** A timeout can bound failure;
  it cannot prove that a worker crossed the contested branch whose post-wake behavior is under
  test.
- **Retry arithmetic is identical to publication arithmetic.** The retry path recognizes durable
  evidence using the same saturating operation, so the final representable block is not a special
  malformed state.

## The eleventh review follow-up, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Eleventh review follow-up: teardown
ownership and unwind containment". All eight findings held.

- **Cleanup ownership precedes user destruction.** A native callback cannot safely drop or wait on
  its own I/O object; transferring the complete bundle first removes that stack from the callback
  before a destructor can panic.
- **Claim ownership is operation-wide, not list-wide.** Registration, update and unregistration call
  the same user-controlled VFS boundary as listing, so the guard belongs to acquisition rather
  than to one caller.
- **Poller survival includes destructors, not only callbacks and samples.** The participant is an
  arbitrary dependency capability, and its `Drop` is part of the worker's reachable user state.
- **Shared opening avoids an interval with live liveness but no discoverable words.** The owner
  fields are ordered so the last state releases live liveness before its model can disappear to a
  weak-cache lookup.
- **Every native sample failure uses one policy.** Initial validation and settling are the same
  logical decision: a count that cannot be sampled must ring through a fallback path, never unwind
  through the caller or callback.
- **A test of native behavior needs a native-only observation.** A deadline or panic result alone is
  shared with the poller; the count-read absence and thread source distinguish the backend.
- **A synchronization test fails by releasing resources, not by timing out inside a scope.** The
  positive signal remains the proof, while cleanup makes a missing signal diagnostic.
- **A test fixture's reachability edge is weak when it exists only during a call.** This preserves
  re-entrancy while allowing the test to prove destruction rather than retaining it permanently.

## The final eleventh review follow-up, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Final eleventh review fixes:
temporary ownership and layer precedence". All five findings held.

- **Temporary ownership is treated as user code, even on success paths.** Moving destruction to
  a helper used only for retired registrations would have preserved the remaining early-exit
  routes. The per-value discard operation belongs to every worker-owned clone and is applied at
  each ownership transfer, rather than inferred from which branch is currently visible.
- **Destruction is sequenced after lock ownership, not wrapped around it.** Catching a panic would
  not solve a destructor that blocks trying to re-enter the same mutex. Taking values and dropping
  the guard first preserves abandonment's state transition while separating arbitrary destruction
  from participant synchronization.
- **Fault precedence belongs at the wrapped capability boundary.** A loop that replicated
  `ProcessEnded` checks in every faulted method would drift as operations were added. One hook on
  `Vfs` lets `MemFs` own its process identity and makes every decorator ask that owner before
  applying behavior of its own.
- **Native teardown and user teardown are one ordered transaction.** Running a withdrawn ring
  under native ownership relied on the publisher not meeting the registration it is withdrawing.
  Releasing native resources first lets normal and callback teardown share the same safety
  property, with callback work still transferred off its own thread.
- **Post-install sample failure is distinct from install failure.** Treating both as polling
  fallback loses the notification fact when a transient failure occurs before a one-shot fault is
  consumed. Returning to the poller is still correct when no native registration was established;
  once one exists, cleanup followed by one ring preserves the count-sampling contract and cannot
  wait silently until the deadline.

## The final review follow-up, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Final review fixes: asynchronous
delivery, complete cleanup and restart ordering". All seven findings held.

- **A completion result is not permission to run on the caller.** Native validation finishing on
  the registering stack is an installation fact only; the notification contract still owns the
  callback thread. Transferring the complete cleanup object retains one owner and chooses the
  callback environment where both native and user cleanup happen in the required order.
- **Abandonment detaches before it destructs, including during another unwind.** Catching one
  panic is insufficient if the value is still under a mutex. Ownership is first moved to detached
  locals, then each destructor runs inside an independent `catch_unwind` on the current thread.
  Rust contains that nested panic before it can escape the original unwind, so cleanup needs no
  executor startup and no ordinary ownership value is leaked.
- **A hostile panic payload belongs to the failed closure.** Rust's panic machinery owns the
  payload, but its `Drop` is arbitrary user code. Forgetting it after the invocation is caught is
  a bounded leak at a fault boundary and is preferable to letting payload destruction kill a
  process-wide worker or cross a native callback boundary.
- **Process precedence is a backend fact before argument precedence.** Raw operations check the
  process first because `FaultyFs` cannot be the first place the contract becomes true. The
  backend performs its ordinary validation afterward, so an ended view is indistinguishable to a
  caller whether it is decorated or not.
- **A restart retains one logical fault schedule, not a copied schedule.** Counters shared by the
  predecessor and successor have one total ordering. A second liveness check after arbitrary
  matching makes the counter consumption the fault operation's linearization point; an old view
  paused before it returns `ProcessEnded` and leaves the successor budget untouched.
- **A concurrency regression must clean up before proving it failed.** Waiting on a condition with
  the blocker still in scope turns a diagnostic timeout into a test deadlock. Error branches
  release that blocker first, while positive entry signals remain the proof that the tested wait
  was reached.

## Teardown availability and restart commit ordering, 2026-11-18

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Teardown availability and restart
commit ordering". All four findings held.

- **A detached map still has to be destroyed value by value.** The file registry's normal
  participant abandonment is entered from a guard's `Drop`, and may already be unwinding. Dropping
  the whole map as one expression couples all values' destructor glue. The contained per-value
  path preserves the participant's original unwind and prevents any one backend destructor from
  skipping the remaining resources.
- **A cleanup dependency discovered during unwind is already too late.** Ordinary lock values
  cannot be forgotten: they are live process-exclusion capabilities. A per-destructor
  `catch_unwind` on the existing thread handles the nested panic without allocating a new
  execution resource, so release availability is independent of thread limits and runtime
  shutdown state.
- **A liveness check is not a commit.** Running it before acquiring the fault counter permits
  restart in the gap. `commit_process` gives the backend the narrow critical section needed to
  validate process identity and consume the scheduled count as one ordered decision, while the
  predicate and observer remain outside it to preserve re-entrancy.
- **A teardown test must observe the side effect it promises.** Empty tables are not evidence
  that destructors ran, because a faulty implementation can leak values while emptying maps.
  Per-category counters prove cleanup was attempted even when each destructor then panics, and
  the test's final scrub exists only to make a failure diagnostic clean.

## The merge review follow-up, 2026-11-19

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Merge review fixes: ordinary
release and native notification hints". Both findings held.

- **Abandonment is selected by destructor failure, not only by a prior unwind.** A `VfsLock`
  destructor can be the operation that starts a panic. Its guard must therefore remove the lock,
  contain destruction, and abandon from that result; checking `thread::panicking` before the
  destructor cannot detect it. Central release helpers keep file and shared participants on the
  same rule and let multi-lock owners continue until every detached value has been attempted.
- **Native hints cannot be tested as poller deadlines.** A Windows oplock is broken by access to
  the native file, not only by the publication this crate performs. The public notification
  contract deliberately makes the notification a hint, so the native test resamples and verifies
  the count did not move. The non-early deadline assertion belongs to the polled path, whose
  only wake sources are its sample result and deadline.

## The full review follow-up, 2026-11-19

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Full review fixes: admission
liveness and worker-owned capture destruction". Both findings held.

- **A release is part of the capability transition.** Removing a gate from a participant table
  and containing its destructor can still abandon that participant. Returning the attachment or
  upgrade before observing that state would publish a capability whose claimed tiers no longer
  exist. Every transition therefore checks liveness after releasing temporary or retiring state,
  while initial acquisition errors and waiting behavior remain unchanged.
- **Callback-return containment ends at callback-owned state destruction.** A Linux worker retains
  the final entry Arc after the callback returns. If self-withdrawal removed the service's other
  references, dropping that Arc destroys user captures on the worker. Containing that final drop
  preserves the service worker; only a panic from the callback body itself follows Signal's
  existing abort policy.

## The corrected full-review follow-up, 2026-11-19

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "Corrected full review fixes:
durability, cleanup and contract alignment".

- **Reviewer error and contract drift are different.** An early count hint is explicitly permitted,
  so its appearance in a review as an implementation defect was wrong. The first sentence of the
  public trigger text still implied exact causation; clarifying that sentence prevents the same
  correlation failure without changing the behavior.
- **Implicit-root file sync is a name exception with no caller action.** The model's ordinary
  file-sync rule stores bytes, while directory sync publishes names. The implicit root is special
  because the API refuses syncing it; direct children must therefore be named by their own file
  fsync.
- **A current liveness observation owns its dependent decision.** Reading boot identity and then
  consulting captured opening state splits one question into two different moments. Sampling uses
  the boot read it just made, just as snapshot does.
- **Cleanup outcome is part of registry and admission success.** A contained destructor panic is
  still abandonment. Capability-returning operations must observe it before returning, and
  destructor paths that cannot return errors must continue releasing every remaining owned lock.
- **Allocation policy is part of resource good citizenship.** `read_at` returns one owned buffer;
  reserving a file-sized sparse range through allocator overcommit is not a dependable error path.
  A stated ceiling gives callers a deterministic chunking rule.
- **A public ordering promise must be provable on every platform.** `ResetEvent` guarantees the
  reset but not a general acquire/release RMW for arbitrary consumers. The contract narrows to
  raise/wait synchronization and requires the protocol's own atomic edge where one is needed.

## UTF-8 is Acadia's logical component boundary, 2026-11-19

The canonical decision is [DESIGN-NOTES.md](DESIGN-NOTES.md) "UTF-8 logical path components".
Passing `Path` or `OsStr` values directly through the contract was rejected because Unix can hold
non-UTF-8 bytes and Windows uses WTF-16, so one operation could succeed or fail according to a
backend's native string representation. That variability is implementation detail, not a storage
feature a dependent should depend on.

Changing `Vfs` path arguments themselves into a Canopy-like path type was unnecessary for this
boundary: the locator syntax is owned by the backend and is useful for absolute roots, while every
normal component can be validated identically on entry and represented losslessly as UTF-8. Keeping
`Path` also avoids coupling Acadia to one caller's hierarchy or character grammar. Creation artifact
names and directory-entry results, where Acadia owns the component abstraction directly, therefore
use `str` and `String` without loss.

Validation is performed before fault matching, so a hostile component cannot be hidden behind an
injected I/O result or consume a deterministic fault schedule. A backend must also refuse a native
directory entry it cannot encode rather than silently replace bytes. This preserves one observable
contract on MemFs and OsFs while leaving any shortening, hashing or authoritative per-directory
mapping policy to a deliberate higher layer.
