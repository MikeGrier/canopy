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
