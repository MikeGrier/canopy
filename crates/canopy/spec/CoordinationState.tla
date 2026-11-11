\* Copyright (c) Michael Grier.
\* Split from Coordination.tla at 70668aa.
------------------------- MODULE CoordinationState --------------------------
(***************************************************************************)
(* Writer exclusion across the processes that open one store root, and the *)
(* counters that let a process holding nothing tell a settled store from   *)
(* one a writer is changing, as the coordination contract specifies them   *)
(* (crates/canopy/DESIGN-NOTES.md, "Several processes may open one store", *)
(* "Exclusion and recovery", "Counters"). Processes interact only through  *)
(* the contract: nothing here knows how an implementation provides the     *)
(* lock, the ownership record or the count.                                *)
(*                                                                         *)
(* A process attaches, then repeatedly requests writer exclusion, holds    *)
(* it while it writes, and releases it, or reads while holding nothing; it *)
(* may withdraw a request, have its request refused, or die at any step. A *)
(* process is used once: attaching starts it and exiting or dying ends it, *)
(* and one that has not attached yet stands for a process started later,   *)
(* after a host restart. Attaching is itself a protocol (below).           *)
(*                                                                         *)
(* Writer exclusion is two things: an OS lock, released when its holder    *)
(* dies, and an ownership record in the store root, which names its        *)
(* holder and survives the holder's death. A clean release clears the      *)
(* record and then drops the lock, so an acquirer that finds a record      *)
(* under the lock knows its holder died holding exclusion. It decides that *)
(* from the record alone: the contract gives it no way to ask whether a    *)
(* process is alive. Acquiring is three steps, each of which a process can *)
(* die between: take the lock, set the fence if a record was left behind,  *)
(* then replace the record, which also advances the generation.            *)
(*                                                                         *)
(* A write is one logical operation on two cells, a and b, standing for a  *)
(* transaction's two swaps: its first effect changes a and its last        *)
(* changes b, so a # b is a store half applied. The modification count     *)
(* brackets the operation: the opening raise is published before the first *)
(* effect and the closing raise after the last. An operation nested under  *)
(* the hold changes nothing: only the outermost raises the count. A        *)
(* publication is two steps, prepare and publish, and only the second is   *)
(* visible, so a writer that dies or fails between them leaves the         *)
(* previous value published. A write that fails after its first effect, or *)
(* whose closing publication fails, sets the fence and leaves the count    *)
(* odd. Nothing here clears the fence or restores trust, so once either is *)
(* set every later acquirer is told recovery is required, and a told       *)
(* holder writes nothing.                                                  *)
(*                                                                         *)
(* The generation advances on every acquisition. A holder relies on its    *)
(* cache of shared, durable state any holder may change, without reading   *)
(* it again, and keeps that cache only if the generation it found is the   *)
(* one it last advanced to; otherwise it reloads.                          *)
(*                                                                         *)
(* A reader takes a sample of the count, observes a and then b in two      *)
(* separate steps, takes a second sample, and classifies the read with     *)
(* Store::undisturbed: the samples are equal and even. A sample is one     *)
(* atomic read of the published value, and may be served from a cache that *)
(* is invalidated synchronously with every publication. State that cannot  *)
(* be vouched for reads as odd.                                            *)
(*                                                                         *)
(* A process's death loses only that process: the live filesystem stays    *)
(* as it was and the OS releases its lock. A host crash kills every        *)
(* process and leaves the coordination state untrusted, because nothing    *)
(* shows it was maintained across the restart; it may also lose the        *)
(* ownership record and the fence, and roll the count back over a writer   *)
(* that was mid-operation. Whether store data is durable is                *)
(* PointerPublication.tla's concern, so a crash here leaves it alone.      *)
(*                                                                         *)
(* Waiting. A request is an individual object that ends in exactly one of  *)
(* four outcomes: served, withdrawn, dead with its process, or refused as  *)
(* ineligible (req). There is no queue or ticket in the state: whenever    *)
(* the lock is free, acquisition is enabled for every eligible waiting     *)
(* process and the checker chooses among them, so every invariant is       *)
(* checked against every service order. "eligible" stands for the          *)
(* admission rules and only ever goes from true to false.                  *)
(*                                                                         *)
(* Attachment. A process is a reader, a writer, or an upgrader whose       *)
(* upgrade is safe or unsafe for readers (Scenario), and speaks one schema *)
(* in the layer that can be upgraded. It attaches through an admission     *)
(* gate and two tiers, locks that ordinary handles hold shared and an      *)
(* upgrade holds exclusively, taken in the order gate, reader tier, writer *)
(* tier, writer exclusion. A handle holds the gate only until it holds the *)
(* reader tier, and no process waits for the gate while holding a tier.    *)
(* Holding every tier it needs, a handle is checked against the admission  *)
(* rules over the store's oldest and current schemas and an upgrade in     *)
(* progress, and is attached or refused. A reader that cannot be left      *)
(* attached through an unsafe upgrade is kept out by the exclusive reader  *)
(* tier; a writer by the exclusive writer tier. Who holds what follows     *)
(* from where each process is (att), so a death releases all of it. An     *)
(* upgrade moves the layer to its target in one step when it ends, and an  *)
(* upgrader that dies in its body leaves the upgrade recorded in progress; *)
(* how an upgrade moves the schemas is not modelled further. Escapes is    *)
(* whether a waiting process may give up or be refused; with it false and  *)
(* nothing dying, the only state without a successor in a deadlock-free    *)
(* model is every process ended.                                           *)
(*                                                                         *)
(* Clones and leases. A handle's clones share its attachment, and every    *)
(* operation on any of them holds a lease on it until the operation ends.  *)
(* A clone's operation is abstract -- it holds a lease and does nothing    *)
(* else -- and the process's own reads and writes are operations too. An   *)
(* upgrader first drains: new operations on every clone are refused and it *)
(* waits for the leases to end while it still holds its tiers, and only    *)
(* then releases them and converts. Called from inside an operation it     *)
(* would wait for that operation, which cannot end while the call is       *)
(* blocked, so the call is refused. A drain can be cancelled, which leaves *)
(* the handle attached; a conversion cancelled or failed after the drain   *)
(* leaves it detached, holding nothing and refusing every operation, and   *)
(* it may attach again as an ordinary admission or be dropped. A death     *)
(* ends every lease with its process.                                      *)
(*                                                                         *)
(* The registry (NSlots entries; none when it is 0). An entry has a claim  *)
(* lock and a liveness lock, both OS locks that a death releases. A handle *)
(* registers after its check and before its attach returns: it claims an   *)
(* entry without waiting, tries the liveness lock without waiting, which   *)
(* succeeds only if no handle holds the entry, writes its token, and lets  *)
(* go of the claim lock last, keeping the liveness lock for as long as it  *)
(* stays attached. If every entry is held it attaches without one and the  *)
(* registry records that. A handle unregisters before it lets go of its    *)
(* tiers, whether it is dropped or its upgrade converts it, and updates    *)
(* its entry under the claim lock when it starts or stops draining. Every  *)
(* change to an entry and every inspection of one is made under its claim  *)
(* lock, which is only tried. A listing runs on a thread of a process that *)
(* need not be attached, visits the entries in turn, reports one live only *)
(* if the liveness lock was held when it probed, and says whether it may   *)
(* be incomplete. A host crash discards the registry.                      *)
(*                                                                         *)
(* Steps are sequentially consistent. The contract's ordering rules -- a   *)
(* raise is ordered with the holder's filesystem operations, and a sample  *)
(* with the sampler's, so a sample taken after observing a modification's  *)
(* effect reflects its opening raise -- hold here by construction. They    *)
(* are requirements on an implementation that this model assumes.          *)
(*                                                                         *)
(* Control selects a fault to confirm the properties detect it:            *)
(*   "none"                     the contract.                              *)
(*   "replace-without-fence"    an acquirer that finds an abandoned record *)
(*                              replaces it without setting the fence.     *)
(*   "unlock-before-clear"      a clean release drops the lock, then       *)
(*                              clears the record.                         *)
(*   "ignore-restart"           an acquirer does not consult whether the   *)
(*                              coordination state survived a restart.     *)
(*   "assembled-sample"         a sample is two parts read in two steps.   *)
(*   "late-invalidation"        a cached sample is invalidated some time   *)
(*                              after the publication that outdates it.    *)
(*   "read-untrusted-as-stored" state that cannot be vouched for is read   *)
(*                              as it is stored.                           *)
(*   "cache-always-kept"        a handle keeps its cache whatever the      *)
(*                              generation it finds.                       *)
(*   "inner-restores-parity"    a nested operation, when it ends, raises   *)
(*                              the count as an outermost one does.        *)
(*   "upgrader-keeps-tiers"     an upgrader keeps its shared tiers while   *)
(*                              it waits for the gate exclusively.         *)
(*   "upgrader-holds-exclusion" an upgrader begins while it holds writer   *)
(*                              exclusion.                                 *)
(*   "unsafe-writer-tier-first" an upgrade unsafe for readers takes the    *)
(*                              writer tier before the reader tier.        *)
(*   "gate-held-through-writer-tier"                                       *)
(*                              a handle keeps the gate while it waits for *)
(*                              the writer tier.                           *)
(*   "unsafe-upgrade-leaves-readers"                                       *)
(*                              an upgrade unsafe for readers does not     *)
(*                              take the reader tier exclusively.          *)
(*   "check-before-writer-tier" a handle is checked before it waits for    *)
(*                              the writer tier and not again.             *)
(*   "upgrade-from-inside-an-operation"                                    *)
(*                              an upgrade is started from inside an       *)
(*                              operation of its own handle.               *)
(*   "drain-ignores-leases"     a conversion begins without waiting for    *)
(*                              the leases to end.                         *)
(*   "clone-starts-while-draining"                                         *)
(*                              a clone starts an operation on a handle    *)
(*                              that is draining.                          *)
(*   "cancelled-drain-detaches" cancelling a drain detaches the handle.    *)
(*   "cancelled-conversion-stays-usable"                                   *)
(*                              a conversion cancelled after the drain     *)
(*                              leaves the handle usable and detached.     *)
(*   "list-without-claim"       a listing inspects an entry without        *)
(*                              claiming it.                               *)
(*   "register-releases-claim-early"                                       *)
(*                              a handle lets go of the claim lock before  *)
(*                              it has written its token.                  *)
(*   "entry-follows-the-handles-life"                                      *)
(*                              an upgrader keeps its entry through the    *)
(*                              conversion that detaches it.               *)
(*   "register-without-claim"   a handle writes an entry without claiming  *)
(*                              it.                                        *)
(*   "reuse-without-liveness-probe"                                        *)
(*                              a handle takes an entry without trying its *)
(*                              liveness lock.                             *)
(*   "listing-infers-from-fullness"                                        *)
(*                              a listing says it may be incomplete only   *)
(*                              when it found no free entry.               *)
(*                                                                         *)
(* The Witness_ properties are false on purpose: a run that checks one     *)
(* must report it violated, which shows the state or step it names is      *)
(* reachable and so that the properties are not holding for want of        *)
(* anything to check.                                                      *)
(***************************************************************************)
EXTENDS Naturals, Integers, FiniteSets, Sequences, TLC

CONSTANTS
    Procs,        \* the processes that open the root, 1..Len(RoleOf)
    MaxRequests,  \* bound on exclusion requests, to keep the model finite
    MaxDeaths,    \* bound on process deaths
    MaxCrashes,   \* bound on host crashes
    MaxReads,     \* bound on reads
    MaxLeases,    \* bound on the operations one handle's clones have in flight
    NSlots,       \* how many entries the registry has; 0 leaves it out
    MaxListings,  \* bound on registry listings
    Escapes,      \* whether a waiter may give up or be refused; FALSE makes a wait
                  \* that never ends a deadlock instead of a request that ends
    Scenario,     \* which of the Scenarios the processes are
    Control       \* the fault under test

None == 0   \* a number, since TLC compares only values of one kind
NoVal == -1

\* A scenario fixes what each process is, which is a reader, a writer, or an
\* upgrader whose upgrade is safe or unsafe for readers, and the schema each
\* speaks in the one layer that can be upgraded, with the oldest and current
\* schemas the store starts with and the one an upgrade moves to. A
\* configuration names one because a configuration file cannot state a
\* sequence.
Scen(roles, speaks, o, c, t) ==
    [roles   |-> roles,
     speaks  |-> [p \in 1..Len(roles) |-> <<speaks[p]>>],
     oldest  |-> <<o>>,
     current |-> <<c>>,
     target  |-> <<t>>]

Scenarios ==
       "Plain2" :> Scen(<<"writer", "writer">>, <<1, 1>>, 1, 1, 1)
    @@ "Plain3" :> Scen(<<"writer", "writer", "writer">>, <<1, 1, 1>>, 1, 1, 1)
    @@ "ReaderAndWriter" :> Scen(<<"reader", "writer">>, <<1, 2>>, 1, 2, 3)
    @@ "ReaderAndWriterTooNew" :> Scen(<<"reader", "writer">>, <<3, 3>>, 1, 2, 3)
    @@ "SafeAndReader" :> Scen(<<"safeUpgrader", "reader">>, <<2, 1>>, 1, 2, 3)
    @@ "SafeAndTargetReader" :> Scen(<<"safeUpgrader", "reader">>, <<2, 3>>, 1, 2, 3)
    @@ "UnsafeAndReader" :> Scen(<<"unsafeUpgrader", "reader">>, <<2, 1>>, 1, 2, 3)
    @@ "SafeAndWriter" :> Scen(<<"safeUpgrader", "writer">>, <<2, 2>>, 1, 2, 3)
    @@ "SafeAndTargetWriter" :> Scen(<<"safeUpgrader", "writer">>, <<2, 3>>, 1, 2, 3)
    @@ "UnsafeAndWriter" :> Scen(<<"unsafeUpgrader", "writer">>, <<2, 2>>, 1, 2, 3)
    @@ "TwoSafe" :> Scen(<<"safeUpgrader", "safeUpgrader">>, <<2, 2>>, 1, 2, 3)
    @@ "SafeWriterReader" :>
          Scen(<<"safeUpgrader", "writer", "reader">>, <<2, 2, 1>>, 1, 2, 3)
    @@ "UnsafeWriterReader" :>
          Scen(<<"unsafeUpgrader", "writer", "reader">>, <<2, 2, 1>>, 1, 2, 3)
    @@ "SafeUnsafeWriter" :>
          Scen(<<"safeUpgrader", "unsafeUpgrader", "writer">>, <<2, 2, 2>>, 1, 2, 3)
    @@ "TwoSafeWriter" :>
          Scen(<<"safeUpgrader", "safeUpgrader", "writer">>, <<2, 2, 2>>, 1, 2, 3)

RoleOf == Scenarios[Scenario].roles
SpeaksOf == Scenarios[Scenario].speaks
Oldest == Scenarios[Scenario].oldest
Current == Scenarios[Scenario].current
Target == Scenarios[Scenario].target
UpLayer == 1

ASSUME Scenario \in DOMAIN Scenarios
ASSUME Procs # {} /\ Procs = 1..Len(RoleOf)
ASSUME MaxRequests \in Nat /\ MaxDeaths \in Nat /\ MaxCrashes \in Nat
ASSUME MaxReads \in Nat /\ MaxLeases \in Nat /\ Escapes \in BOOLEAN
ASSUME NSlots \in Nat /\ MaxListings \in Nat
ASSUME \A p \in Procs :
          RoleOf[p] \in {"reader", "writer", "safeUpgrader", "unsafeUpgrader"}
ASSUME \A l \in 1..Len(Oldest) :
          Oldest[l] <= Current[l] /\ Current[l] <= Target[l]
ASSUME Control \in {"none", "replace-without-fence", "unlock-before-clear",
                    "ignore-restart", "assembled-sample", "late-invalidation",
                    "read-untrusted-as-stored", "cache-always-kept",
                    "inner-restores-parity", "upgrader-keeps-tiers",
                    "upgrader-holds-exclusion", "unsafe-writer-tier-first",
                    "gate-held-through-writer-tier",
                    "unsafe-upgrade-leaves-readers", "check-before-writer-tier",
                    "upgrade-from-inside-an-operation", "drain-ignores-leases",
                    "clone-starts-while-draining", "cancelled-drain-detaches",
                    "cancelled-conversion-stays-usable",
                    "list-without-claim", "register-releases-claim-early",
                    "entry-follows-the-handles-life", "register-without-claim",
                    "reuse-without-liveness-probe",
                    "listing-infers-from-fullness"}

Layers == 1..Len(Oldest)

\* The most any count or sample can reach: two raises per operation, one more
\* for each operation's nested end under a control, and one for a sample of
\* state that cannot be vouched for.
MaxCount == 3 * MaxRequests + 1

VARIABLES
    pc,          \* [Procs -> process states]: each process's execution state
    req,         \* [Procs -> request states]: the state of its latest request
    eligible,    \* [Procs -> BOOLEAN]: whether the admission rules admit it
    lockHolder,  \* the process holding the OS lock, or None
    owner,       \* the process the ownership record names, or None
    fence,       \* recovery is required until it is cleared; survives a death
    trusted,     \* the coordination state is known to have been maintained
    count,       \* the published modification count
    gen,         \* the generation, advanced on every acquisition of exclusion
    a, b,        \* the two cells of a write; a # b is a store half applied
    shared,      \* durable state a holder may cache
    mygen,       \* [Procs -> gen or NoVal]: what each handle last advanced to
    cache,       \* [Procs -> shared or NoVal]: each handle's knowledge of it
    rphase,      \* [Procs -> reader phase]: where each reader is in its read
    rbefore,     \* [Procs -> sample or NoVal]: a reader's first sample
    rseenA,      \* [Procs -> a or NoVal]: what a reader observed of a
    rseenB,      \* [Procs -> b or NoVal]: what a reader observed of b
    rpart,       \* [Procs -> part or NoVal]: half an assembled sample
    scache,      \* [Procs -> sample or NoVal]: each reader's cached sample
    att,         \* [Procs -> attachment states]: where each is in attaching or
                 \* converting, which fixes the gate and tiers it holds
    areq,        \* [Procs -> request states]: the state of its latest attach or
                 \* upgrade request
    lease,       \* [Procs -> 0..MaxLeases]: operations its clones have in flight
    cl,          \* [Slots -> Procs or None]: who holds each entry's claim lock
    ll,          \* [Slots -> Procs or None]: who holds each entry's liveness lock
    etok,        \* [Slots -> Procs or None]: the token written in each entry
    edr,         \* [Slots -> BOOLEAN]: whether the entry says its handle drains
    ovf,         \* some handle attached unregistered because no entry was free
    rst,         \* [Procs -> registry state]: where each is in registering,
                 \* updating or unregistering its entry
    rscan,       \* [Procs -> SUBSET Slots]: the entries a registrant has not tried
    rcur,        \* [Procs -> Slots or None]: the entry a registrant works on
    lph,         \* [Procs -> listing phase]: where each one's listing thread is
    lcur,        \* [Procs -> 1..NSlots+1]: the next entry its listing visits
    ltok,        \* [Procs -> token or None]: the token its listing has read
    lrep,        \* [Procs -> report]: the entry its listing last reported on
    lunk,        \* [Procs -> BOOLEAN]: its listing met an entry it could not claim
    lfree,       \* [Procs -> BOOLEAN]: its listing met an entry that was not live
    lres,        \* [Procs -> listing result]: what its last listing said
    lists,       \* how many listings have been started
    oldest,      \* [Layers -> schema]: the oldest admissible reader schema
    current,     \* [Layers -> schema]: the current schema
    upState,     \* "ordinary", or an upgrade of UpLayer in progress, "safe" or
                 \* "unsafe" for readers; it survives its upgrader's death
    diedIn,      \* ghost: where the latest holder died with the record present
    requests,
    deaths,
    crashes,
    reads

vars == <<pc, req, eligible, lockHolder, owner, fence, trusted, count, gen,
          a, b, shared, mygen, cache, rphase, rbefore, rseenA, rseenB, rpart,
          scache, att, areq, lease, cl, ll, etok, edr, ovf, rst, rscan, rcur,
          lph, lcur, ltok, lrep, lunk, lfree, lres, lists, oldest, current,
          upState, diedIn, requests, deaths, crashes, reads>>

coord  == <<lockHolder, owner, fence, trusted, count, gen>>
store  == <<a, b, shared>>
handle == <<mygen, cache>>
reader == <<rphase, rbefore, rseenA, rseenB, rpart, scache>>
counts == <<requests, deaths, crashes, reads>>
schema == <<oldest, current, upState>>
\* The registry: the entries' locks and data, each handle's place in keeping its
\* entry, and each listing's.
rlock  == <<cl, ll>>
rdata  == <<etok, edr>>
rown   == <<rst, rscan, rcur>>
rlist  == <<lph, lcur, ltok, lrep, lunk, lfree, lres, lists>>
reg    == <<rlock, rdata, ovf, rown, rlist>>
adm    == <<att, areq, lease, reg, schema>>
\* Everything but the process state, the attachment state, its request, the
\* leases, the registry and the schema state.
bystanders == <<req, eligible, coord, store, handle, reader, diedIn, counts>>
rest   == <<lease, reg, bystanders>>
\* Everything but the registry.
core   == <<pc, att, areq, lease, schema, bystanders>>

\* Process states. "new" has not started; "attaching" is passing the gate and
\* taking its tiers, and "converting" is an upgrader doing the same exclusively;
\* "convertingX" is one doing it while holding writer exclusion, only under
\* Control = "upgrader-holds-exclusion". "locked" holds the lock and has not yet
\* replaced the record.
\* A holder is then "held" before an operation; "opening" while the opening
\* raise is prepared and not yet published; "open" once it is published and
\* before the first effect; "writing" between the effects, or "nested" while an
\* operation nested under the hold runs there; "wrote" after the last;
\* "closing" while the closing raise is prepared and not yet published; and
\* "closed" once the operation is over. "told" holds exclusion having been
\* told recovery is required, or having failed unresolved, and may only
\* release it. "releasing" has cleared the record and still holds the lock;
\* "unlocked" is only reached under Control = "unlock-before-clear", and has
\* dropped the lock but not cleared the record. "detached" is a handle whose
\* conversion was cancelled or failed: it holds nothing, refuses every
\* operation, and may attach again or be dropped. "detaching" is a handle that
\* is being dropped and is leaving the registry before it lets go of its tiers.
LiveStates == {"attaching", "converting", "convertingX", "idle", "waiting",
               "locked", "held", "opening", "open", "writing", "nested",
               "wrote", "closing", "closed", "told", "releasing",
               "unlocked", "detached", "detaching"}
\* Where a process is inside an operation of its own: a write from its request
\* to its release. A read is inside one while rphase is not "none".
InOperationStates == LiveStates \ {"attaching", "converting", "convertingX",
                                   "idle", "detached", "detaching"}
States == {"new", "exited", "dead"} \cup LiveStates
OpenStates == {"open", "writing", "nested", "wrote", "closing"}
OwnsRecord == OpenStates \cup {"held", "opening", "closed", "told", "convertingX"}
HoldsLock == OwnsRecord \cup {"locked", "releasing"}
Outcomes == {"served", "withdrawn", "dead", "refused"}
DiedKinds == {"none", "crash", "unlocked"} \cup OwnsRecord
ReaderPhases == {"none", "before2", "obsA", "obsB", "after1", "after2"}

\* Attachment states. An ordinary attach goes wGate (asks for the gate shared),
\* hasGate (holds it, asks for the reader tier), gateRd (holds both, about to
\* drop the gate), rd (holds the reader tier, asks for the writer tier), tiers
\* (holds every tier it needs, check pending), registering (checked and
\* holding its tiers, writing its entry in the registry, which is skipped when
\* there is none), then attached, or refused and gone. A handle leaves its
\* attachment through unregistering, which clears its entry while it still holds
\* its tiers. "gateWr" and "rdChecked" are only reached under a control. An
\* upgrade goes draining (still attached and holding its tiers, refusing new
\* operations and waiting for the leases to end), uWGate (holding nothing,
\* asks for the gate exclusively; uWGateT holds its shared tiers, only under a
\* control), uGate (holds the gate), uRd or uWr (also one tier), uTiers (every
\* tier it needs), then uBody (the upgrade is in progress).
AttStates == {"off", "wGate", "hasGate", "gateRd", "gateWr", "rd", "rdChecked",
              "tiers", "registering", "attached", "unregistering", "draining",
              "uWGate", "uWGateT", "uGate", "uRd", "uWr", "uTiers", "uBody"}

\* The registry's entries, and what a registrant, an owner and a listing can be
\* doing. A registrant is "scan"ning for an entry, then has "claimed" one,
\* holds its "locked" liveness lock, has "written" its token, and is "in" once it
\* has let go of the claim lock; "unreg" is a handle that found every entry held
\* and attached without one. An owner "upd"ates its entry under its claim lock,
\* and unregisters by claiming ("uc"), clearing ("ucl") and releasing.
Slots == 1..NSlots
RegistryOn == NSlots > 0
RegStates == {"out", "scan", "claimed", "locked", "written", "in", "upd", "uc",
              "ucl", "unreg"}
ListPhases == {"none", "next", "held", "read"}
ListStatuses == {"free", "dead", "live", "unknown"}
NoRep == <<0, 0, "none">>
HoldsAttachment == {"registering", "attached", "unregistering", "draining"}

Set(f, k, v) == [f EXCEPT ![k] = v]
Odd(n) == n % 2 = 1
Even(n) == n % 2 = 0

CanWrite(p) == RoleOf[p] # "reader"
Upgrader(p) == RoleOf[p] \in {"safeUpgrader", "unsafeUpgrader"}
\* Whether an upgrade takes the reader tier exclusively, which drains every
\* reader. An upgrade that is unsafe for readers does, unless a control says
\* otherwise.
TakesReaderTier(p) ==
    RoleOf[p] = "unsafeUpgrader" /\ Control # "unsafe-upgrade-leaves-readers"

\* The gate and the tiers each process holds, fixed by where it is.
HoldsGateS(p) == att[p] \in {"hasGate", "gateRd", "gateWr"}
HoldsGateX(p) ==
    \/ att[p] \in {"uGate", "uRd", "uWr", "uTiers"}
    \/ att[p] = "uBody" /\ TakesReaderTier(p)
HoldsReaderS(p) ==
    \/ att[p] \in {"gateRd", "gateWr", "rd", "rdChecked", "tiers", "registering",
                   "attached", "unregistering", "draining"}
    \/ att[p] = "uWGateT"
HoldsWriterS(p) ==
    /\ CanWrite(p)
    /\ att[p] \in {"tiers", "registering", "attached", "unregistering",
                   "draining", "uWGateT"}
HoldsReaderX(p) ==
    \/ att[p] = "uRd"
    \/ att[p] \in {"uTiers", "uBody"} /\ TakesReaderTier(p)
HoldsWriterX(p) == att[p] \in {"uWr", "uTiers", "uBody"}

\* The operations in flight on a handle: its clones' and its own read. A write
\* of its own is inside one while pc says so.
Leases(p) == lease[p] + (IF rphase[p] # "none" THEN 1 ELSE 0)
InsideOperation(p) == rphase[p] # "none" \/ pc[p] \in InOperationStates

GateSHeld == \E q \in Procs : HoldsGateS(q)
GateXHeld == \E q \in Procs : HoldsGateX(q)
ReaderSHeld == \E q \in Procs : HoldsReaderS(q)
ReaderXHeld == \E q \in Procs : HoldsReaderX(q)
WriterSHeld == \E q \in Procs : HoldsWriterS(q)
WriterXHeld == \E q \in Procs : HoldsWriterX(q)

\* Whether an upgrade of layer l is in progress, which changes what it admits.
InProgress(l) == upState # "ordinary" /\ l = UpLayer

\* The admission rules, per layer, over what the store records (DESIGN-NOTES
\* "Schemas and attachment").
ReaderOK(l, s) ==
    IF InProgress(l)
       THEN upState = "safe"
            /\ ((oldest[l] <= s /\ s <= current[l]) \/ s = Target[l])
       ELSE oldest[l] <= s /\ s <= current[l]
WriterOK(l, s) ==
    IF InProgress(l) THEN s = Target[l] ELSE s = current[l]
Admissible(p) ==
    \A l \in Layers :
        IF CanWrite(p) THEN WriterOK(l, SpeaksOf[p][l])
                       ELSE ReaderOK(l, SpeaksOf[p][l])

\* A store with its write half applied.
Partial == a # b

\* What a sampler finds when it reads the published count. State that cannot be
\* vouched for reads as odd, whatever is stored.
Visible ==
    IF trusted \/ Control = "read-untrusted-as-stored"
       THEN count
       ELSE IF Odd(count) THEN count ELSE count + 1

\* The value a reader's next single-step sample returns: its cached sample if
\* it holds one, otherwise what it reads.
Sampled(p) == IF scache[p] # NoVal THEN scache[p] ELSE Visible

\* Store::undisturbed: two samples bracket a span in which nothing was modified.
Undisturbed(x, y) == x = y /\ Even(x)

-----------------------------------------------------------------------------
TypeOK ==
    /\ pc \in [Procs -> States]
    /\ req \in [Procs -> {"none", "pending"} \cup Outcomes]
    /\ eligible \in [Procs -> BOOLEAN]
    /\ lockHolder \in Procs \cup {None}
    /\ owner \in Procs \cup {None}
    /\ fence \in BOOLEAN
    /\ trusted \in BOOLEAN
    /\ count \in 0..MaxCount
    /\ gen \in 0..MaxRequests
    /\ a \in 0..MaxRequests
    /\ b \in 0..MaxRequests
    /\ shared \in 0..MaxRequests
    /\ mygen \in [Procs -> NoVal..MaxRequests]
    /\ cache \in [Procs -> NoVal..MaxRequests]
    /\ rphase \in [Procs -> ReaderPhases]
    /\ rbefore \in [Procs -> NoVal..MaxCount]
    /\ rseenA \in [Procs -> NoVal..MaxRequests]
    /\ rseenB \in [Procs -> NoVal..MaxRequests]
    /\ rpart \in [Procs -> NoVal..MaxCount]
    /\ scache \in [Procs -> NoVal..MaxCount]
    /\ att \in [Procs -> AttStates]
    /\ areq \in [Procs -> {"none", "pending"} \cup Outcomes]
    /\ lease \in [Procs -> 0..MaxLeases]
    /\ cl \in [Slots -> Procs \cup {None}]
    /\ ll \in [Slots -> Procs \cup {None}]
    /\ etok \in [Slots -> Procs \cup {None}]
    /\ edr \in [Slots -> BOOLEAN]
    /\ ovf \in BOOLEAN
    /\ rst \in [Procs -> RegStates]
    /\ rscan \in [Procs -> SUBSET Slots]
    /\ rcur \in [Procs -> 0..NSlots]
    /\ lph \in [Procs -> ListPhases]
    /\ lcur \in [Procs -> 1..(NSlots + 1)]
    /\ ltok \in [Procs -> Procs \cup {None}]
    /\ lrep \in [Procs -> {NoRep} \cup (Slots \X (Procs \cup {None}) \X ListStatuses)]
    /\ lunk \in [Procs -> BOOLEAN]
    /\ lfree \in [Procs -> BOOLEAN]
    /\ lres \in [Procs -> {"none", "complete", "incomplete"}]
    /\ lists \in 0..MaxListings
    /\ oldest \in [Layers -> Nat]
    /\ current \in [Layers -> Nat]
    /\ upState \in {"ordinary", "safe", "unsafe"}
    /\ diedIn \in DiedKinds
    /\ requests \in 0..MaxRequests
    /\ deaths \in 0..MaxDeaths
    /\ crashes \in 0..MaxCrashes
    /\ reads \in 0..MaxReads

Init ==
    /\ pc = [p \in Procs |-> "new"]
    /\ req = [p \in Procs |-> "none"]
    /\ eligible = [p \in Procs |-> TRUE]
    /\ lockHolder = None
    /\ owner = None
    /\ fence = FALSE
    /\ trusted = TRUE
    /\ count = 0
    /\ gen = 0
    /\ a = 0
    /\ b = 0
    /\ shared = 0
    /\ mygen = [p \in Procs |-> NoVal]
    /\ cache = [p \in Procs |-> NoVal]
    /\ rphase = [p \in Procs |-> "none"]
    /\ rbefore = [p \in Procs |-> NoVal]
    /\ rseenA = [p \in Procs |-> NoVal]
    /\ rseenB = [p \in Procs |-> NoVal]
    /\ rpart = [p \in Procs |-> NoVal]
    /\ scache = [p \in Procs |-> NoVal]
    /\ att = [p \in Procs |-> "off"]
    /\ areq = [p \in Procs |-> "none"]
    /\ lease = [p \in Procs |-> 0]
    /\ cl = [s \in Slots |-> None]
    /\ ll = [s \in Slots |-> None]
    /\ etok = [s \in Slots |-> None]
    /\ edr = [s \in Slots |-> FALSE]
    /\ ovf = FALSE
    /\ rst = [p \in Procs |-> "out"]
    /\ rscan = [p \in Procs |-> {}]
    /\ rcur = [p \in Procs |-> 0]
    /\ lph = [p \in Procs |-> "none"]
    /\ lcur = [p \in Procs |-> 1]
    /\ ltok = [p \in Procs |-> None]
    /\ lrep = [p \in Procs |-> NoRep]
    /\ lunk = [p \in Procs |-> FALSE]
    /\ lfree = [p \in Procs |-> FALSE]
    /\ lres = [p \in Procs |-> "none"]
    /\ lists = 0
    /\ oldest = [l \in Layers |-> Oldest[l]]
    /\ current = [l \in Layers |-> Current[l]]
    /\ upState = "ordinary"
    /\ diedIn = "none"
    /\ requests = 0
    /\ deaths = 0
    /\ crashes = 0
    /\ reads = 0

\* What a process forgets when it ends: its handle's memory and any read.
Forget(p) ==
    /\ mygen' = Set(mygen, p, NoVal)
    /\ cache' = Set(cache, p, NoVal)
    /\ rphase' = Set(rphase, p, "none")
    /\ rbefore' = Set(rbefore, p, NoVal)
    /\ rseenA' = Set(rseenA, p, NoVal)
    /\ rseenB' = Set(rseenB, p, NoVal)
    /\ rpart' = Set(rpart, p, NoVal)
    /\ scache' = Set(scache, p, NoVal)

=============================================================================
