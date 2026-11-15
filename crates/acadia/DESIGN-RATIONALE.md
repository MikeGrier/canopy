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