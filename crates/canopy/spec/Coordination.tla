\* Copyright (c) Michael Grier.
---------------------------- MODULE Coordination ----------------------------
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

-----------------------------------------------------------------------------
(* Attachment and requests.                                                *)

\* A clean detach, or the drop of a detached handle, once none of its clones
\* has an operation in flight and no listing of its own is running. A handle that
\* is attached unregisters before it lets go of its tiers.
Exit(p) ==
    /\ pc[p] \in {"idle", "detached"} /\ rphase[p] = "none" /\ lease[p] = 0
    /\ lph[p] = "none" /\ rst[p] \in {"out", "in", "unreg"}
    /\ IF pc[p] = "idle" /\ RegistryOn
          THEN /\ pc' = Set(pc, p, "detaching")
               /\ att' = Set(att, p, "unregistering")
               /\ UNCHANGED <<handle, reader>>
          ELSE /\ pc' = Set(pc, p, "exited")
               /\ att' = Set(att, p, "off")
               /\ Forget(p)
    /\ UNCHANGED <<areq, lease, schema, req, eligible, coord, store, diedIn,
                   counts, reg>>

\* EXTERNAL: the admission rules stop admitting p's handle.
Disqualify(p) ==
    /\ Escapes
    /\ pc[p] \in {"idle", "waiting"} /\ eligible[p]
    /\ eligible' = Set(eligible, p, FALSE)
    /\ UNCHANGED <<adm, pc, req, coord, store, handle, reader, diedIn, counts>>

Request(p) ==
    /\ CanWrite(p)
    /\ pc[p] = "idle" /\ rphase[p] = "none" /\ requests < MaxRequests
    /\ pc' = Set(pc, p, "waiting")
    /\ req' = Set(req, p, "pending")
    /\ requests' = requests + 1
    /\ UNCHANGED <<adm, eligible, coord, store, handle, reader, diedIn, deaths,
                   crashes, reads>>

\* The waiter gives up.
Withdraw(p) ==
    /\ Escapes
    /\ pc[p] = "waiting"
    /\ pc' = Set(pc, p, "idle")
    /\ req' = Set(req, p, "withdrawn")
    /\ UNCHANGED <<adm, eligible, coord, store, handle, reader, diedIn, counts>>

\* A request that is no longer eligible is refused rather than served.
Refuse(p) ==
    /\ Escapes
    /\ pc[p] = "waiting" /\ ~eligible[p]
    /\ pc' = Set(pc, p, "idle")
    /\ req' = Set(req, p, "refused")
    /\ UNCHANGED <<adm, eligible, coord, store, handle, reader, diedIn, counts>>

-----------------------------------------------------------------------------
(* The admission gate and the attachment tiers (DESIGN-NOTES "Schemas and *)
(* attachment"). The gate and each tier are locks that ordinary handles    *)
(* share and an upgrade takes exclusively. Who holds each follows from att, *)
(* so a process's death releases all of them.                              *)

\* An ordinary handle passes the gate shared, takes the reader tier, drops the
\* gate before it waits for anything else, takes the writer tier if it may
\* write, and only then is checked, so an upgrade cannot complete between the
\* check and the attachment. A detached handle attaches again the same way.
BeginAttach(p) ==
    /\ pc[p] \in {"new", "detached"} /\ rst[p] = "out"
    /\ pc' = Set(pc, p, "attaching")
    /\ att' = Set(att, p, "wGate")
    /\ areq' = Set(areq, p, "pending")
    /\ UNCHANGED <<schema, rest>>

TakeGateS(p) ==
    /\ att[p] = "wGate" /\ ~GateXHeld
    /\ att' = Set(att, p, "hasGate")
    /\ UNCHANGED <<pc, areq, schema, rest>>

TakeReaderS(p) ==
    /\ att[p] = "hasGate" /\ ~ReaderXHeld
    /\ att' = Set(att, p, "gateRd")
    /\ UNCHANGED <<pc, areq, schema, rest>>

DropGate(p) ==
    /\ att[p] = "gateRd"
    /\ ~(CanWrite(p) /\ Control = "gate-held-through-writer-tier")
    /\ att' = Set(att, p, IF CanWrite(p) THEN "rd" ELSE "tiers")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* Under Control = "gate-held-through-writer-tier" a handle that may write keeps
\* the gate while it waits for the writer tier.
KeepGate(p) ==
    /\ att[p] = "gateRd" /\ CanWrite(p)
    /\ Control = "gate-held-through-writer-tier"
    /\ att' = Set(att, p, "gateWr")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* Under Control = "check-before-writer-tier" the check is made before the
\* writer tier is waited for, and not again.
CheckEarly(p) ==
    /\ Control = "check-before-writer-tier" /\ att[p] = "rd"
    /\ IF Admissible(p)
          THEN /\ att' = Set(att, p, "rdChecked")
               /\ UNCHANGED <<pc, areq>>
          ELSE /\ lph[p] = "none"
               /\ att' = Set(att, p, "off")
               /\ pc' = Set(pc, p, "exited")
               /\ areq' = Set(areq, p, "refused")
    /\ UNCHANGED <<schema, rest>>

TakeWriterS(p) ==
    /\ \/ att[p] = "rd" /\ Control # "check-before-writer-tier"
       \/ att[p] \in {"gateWr", "rdChecked"}
    /\ ~WriterXHeld
    /\ IF att[p] = "rdChecked"
          THEN /\ att' = Set(att, p, IF RegistryOn THEN "registering" ELSE "attached")
               /\ pc' = IF RegistryOn THEN pc ELSE Set(pc, p, "idle")
               /\ areq' = IF RegistryOn THEN areq ELSE Set(areq, p, "served")
          ELSE /\ att' = Set(att, p, "tiers")
               /\ UNCHANGED <<pc, areq>>
    /\ UNCHANGED <<schema, rest>>

\* The check, under the shared tiers it needs: a handle the rules do not admit
\* is refused and releases them. One that is admitted goes on to register, and
\* its attach returns once it has.
Admit(p) ==
    /\ att[p] = "tiers"
    /\ IF Admissible(p)
          THEN /\ att' = Set(att, p, IF RegistryOn THEN "registering" ELSE "attached")
               /\ pc' = IF RegistryOn THEN pc ELSE Set(pc, p, "idle")
               /\ areq' = IF RegistryOn THEN areq ELSE Set(areq, p, "served")
          ELSE /\ lph[p] = "none"
               /\ att' = Set(att, p, "off")
               /\ pc' = Set(pc, p, "exited")
               /\ areq' = Set(areq, p, "refused")
    /\ UNCHANGED <<schema, rest>>

\* A handle that is waiting gives up, releasing whatever it holds. A conversion
\* that gives up after its drain leaves the handle detached, at once and with
\* nothing left to wait for.
GiveUp(p) ==
    /\ Escapes /\ lph[p] = "none"
    /\ \/ att[p] \in {"wGate", "hasGate", "gateWr", "rd", "rdChecked"}
       \/ att[p] \in {"uWGate", "uGate", "uRd", "uWr"} /\ pc[p] = "converting"
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p,
                 IF pc[p] # "converting" THEN "exited"
                 ELSE IF Control = "cancelled-conversion-stays-usable"
                         THEN "idle" ELSE "detached")
    /\ areq' = Set(areq, p, "withdrawn")
    /\ UNCHANGED <<schema, rest>>

\* A clone of an attached handle starts an operation, which holds a lease on the
\* attachment until it ends. A handle that is draining or converting refuses it.
BeginOp(p) ==
    /\ \/ att[p] = "attached"
       \/ att[p] = "draining" /\ Control = "clone-starts-while-draining"
    /\ lease[p] < MaxLeases
    /\ lease' = Set(lease, p, lease[p] + 1)
    /\ UNCHANGED <<pc, att, areq, schema, reg, bystanders>>

EndOp(p) ==
    /\ lease[p] > 0
    /\ lease' = Set(lease, p, lease[p] - 1)
    /\ UNCHANGED <<pc, att, areq, schema, reg, bystanders>>

\* An upgrader first drains: new operations on every clone are refused and it
\* waits for the leases to end while it still holds its tiers. Called from inside
\* an operation it would wait for that operation, which cannot end while the
\* call is blocked, so the call is refused; under Control =
\* "upgrade-from-inside-an-operation" it starts and waits for itself.
StartUpgrade(p) ==
    /\ Upgrader(p) /\ pc[p] = "idle" /\ att[p] = "attached"
    /\ (rphase[p] = "none" \/ Control = "upgrade-from-inside-an-operation")
    /\ pc' = Set(pc, p, "converting")
    /\ att' = Set(att, p, "draining")
    /\ areq' = Set(areq, p, "pending")
    /\ UNCHANGED <<schema, rest>>

RefuseUpgrade(p) ==
    /\ Upgrader(p) /\ att[p] = "attached" /\ InsideOperation(p)
    /\ areq' = Set(areq, p, "refused")
    /\ UNCHANGED <<pc, att, schema, rest>>

\* Once no lease is outstanding the drain ends and the conversion begins: the
\* upgrader releases its shared tiers before it asks for the gate exclusively,
\* so it never waits for the gate while holding a tier. Under Control =
\* "upgrader-keeps-tiers" it keeps them until it has the gate, and under
\* "drain-ignores-leases" it does not wait for the leases. The handle leaves the
\* registry first, since its entry follows its attachment and not its life, which
\* Control = "entry-follows-the-handles-life" reverses.
DrainDone(p) ==
    /\ att[p] = "draining"
    /\ rst[p] \in {"out", "in", "unreg"}
    /\ (Leases(p) = 0 \/ Control = "drain-ignores-leases")
    /\ att' = Set(att, p,
                  IF RegistryOn /\ Control # "entry-follows-the-handles-life"
                     THEN "unregistering"
                     ELSE IF Control = "upgrader-keeps-tiers" THEN "uWGateT"
                                                              ELSE "uWGate")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* A drain is cancelled by its caller; the handle stays attached and usable.
CancelDrain(p) ==
    /\ Escapes /\ att[p] = "draining"
    /\ IF Control = "cancelled-drain-detaches"
          THEN /\ att' = Set(att, p, "off")
               /\ pc' = Set(pc, p, "detached")
          ELSE /\ att' = Set(att, p, "attached")
               /\ pc' = Set(pc, p, "idle")
    /\ areq' = Set(areq, p, "withdrawn")
    /\ UNCHANGED <<schema, rest>>

\* Under Control = "upgrader-holds-exclusion" an upgrader starts while it holds
\* writer exclusion, without draining.
StartUpgradeHolding(p) ==
    /\ Control = "upgrader-holds-exclusion"
    /\ Upgrader(p) /\ pc[p] = "held" /\ att[p] = "attached"
    /\ pc' = Set(pc, p, "convertingX")
    /\ att' = Set(att, p, "uWGate")
    /\ areq' = Set(areq, p, "pending")
    /\ UNCHANGED <<schema, rest>>

TakeGateX(p) ==
    /\ att[p] \in {"uWGate", "uWGateT"}
    /\ ~GateSHeld /\ ~GateXHeld
    /\ att' = Set(att, p, "uGate")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* The exclusive tiers follow the order of the gate, then the reader tier, then
\* the writer tier: an upgrade that drains readers takes the reader tier first.
\* Under Control = "unsafe-writer-tier-first" it takes the writer tier first.
TakeFirstTier(p) ==
    /\ att[p] = "uGate"
    /\ IF TakesReaderTier(p) /\ Control # "unsafe-writer-tier-first"
          THEN /\ ~ReaderSHeld /\ ~ReaderXHeld
               /\ att' = Set(att, p, "uRd")
          ELSE /\ ~WriterSHeld /\ ~WriterXHeld
               /\ att' = Set(att, p, IF TakesReaderTier(p) THEN "uWr"
                                                           ELSE "uTiers")
    /\ UNCHANGED <<pc, areq, schema, rest>>

TakeSecondTier(p) ==
    /\ \/ att[p] = "uRd" /\ ~WriterSHeld /\ ~WriterXHeld
       \/ att[p] = "uWr" /\ ~ReaderSHeld /\ ~ReaderXHeld
    /\ att' = Set(att, p, "uTiers")
    /\ UNCHANGED <<pc, areq, schema, rest>>

\* Holding its tiers, an upgrader checks the schemas again. If they are as it
\* found them and no upgrade is under way, the upgrade begins; one that leaves
\* readers attached lets the gate reopen to them.
BeginUpgrade(p) ==
    /\ att[p] = "uTiers"
    /\ current[UpLayer] = SpeaksOf[p][UpLayer] /\ upState = "ordinary"
    /\ upState' = IF RoleOf[p] = "unsafeUpgrader" THEN "unsafe" ELSE "safe"
    /\ att' = Set(att, p, "uBody")
    /\ UNCHANGED <<oldest, current, pc, areq, rest>>

\* Otherwise it gives up, and its handle is left detached.
UpgradeMoved(p) ==
    /\ att[p] = "uTiers"
    /\ ~(current[UpLayer] = SpeaksOf[p][UpLayer] /\ upState = "ordinary")
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p, IF pc[p] = "convertingX" THEN "held" ELSE "detached")
    /\ areq' = Set(areq, p, "refused")
    /\ UNCHANGED <<schema, rest>>

\* The upgrade completes: the layer moves to its target, and an upgrade unsafe
\* for readers raises the oldest reader schema with it.
EndUpgrade(p) ==
    /\ att[p] = "uBody"
    /\ (pc[p] = "convertingX" \/ lph[p] = "none")
    /\ upState' = "ordinary"
    /\ current' = Set(current, UpLayer, Target[UpLayer])
    /\ oldest' = IF RoleOf[p] = "unsafeUpgrader"
                    THEN Set(oldest, UpLayer, Target[UpLayer]) ELSE oldest
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p, IF pc[p] = "convertingX" THEN "held" ELSE "exited")
    /\ areq' = Set(areq, p, "served")
    /\ UNCHANGED rest

-----------------------------------------------------------------------------
(* The registry of attached handles (DESIGN-NOTES "A registry lists the   *)
(* attached handles"). Each entry has a claim lock and a liveness lock,    *)
(* both OS locks that a death releases. Every change to an entry and every *)
(* inspection of one is made under its claim lock, which is only ever     *)
(* tried, never waited for, and a handle keeps its entry's liveness lock   *)
(* for as long as it stays attached.                                       *)

Unclaim(s, p) == IF cl[s] = p THEN Set(cl, s, None) ELSE cl

\* After its check a handle writes an entry before its attach returns. It tries
\* the entries one at a time: it claims one without waiting, tries the liveness
\* lock exclusively without waiting, which succeeds only if no handle holds the
\* entry, writes its token, and lets go of the claim lock last. Holding the claim
\* lock from before the liveness lock until after the token is written is what
\* keeps a listing from seeing a new owner's lock beside an old owner's token.
RegStart(p) ==
    /\ att[p] = "registering" /\ rst[p] = "out"
    /\ rst' = Set(rst, p, "scan")
    /\ rscan' = Set(rscan, p, Slots)
    /\ UNCHANGED <<core, rlock, rdata, ovf, rcur, rlist>>

\* A claim lock that is held moves the handle on to the next entry.
RegClaim(p, s) ==
    /\ rst[p] = "scan" /\ s \in rscan[p]
    /\ rscan' = Set(rscan, p, rscan[p] \ {s})
    /\ IF cl[s] = None \/ Control = "register-without-claim"
          THEN /\ cl' = IF Control = "register-without-claim" THEN cl
                                                             ELSE Set(cl, s, p)
               /\ rst' = Set(rst, p, "claimed")
               /\ rcur' = Set(rcur, p, s)
          ELSE UNCHANGED <<cl, rst, rcur>>
    /\ UNCHANGED <<core, ll, rdata, ovf, rlist>>

\* A liveness lock that is held means a live handle owns the entry, and the
\* handle moves on. Under Control = "reuse-without-liveness-probe" it takes the
\* entry regardless, and under "register-releases-claim-early" it lets go of the
\* claim lock before it has written.
RegLive(p) ==
    /\ rst[p] = "claimed"
    /\ IF ll[rcur[p]] = None \/ Control = "reuse-without-liveness-probe"
          THEN /\ ll' = IF ll[rcur[p]] = None THEN Set(ll, rcur[p], p) ELSE ll
               /\ rst' = Set(rst, p, "locked")
               /\ cl' = IF Control = "register-releases-claim-early"
                           THEN Unclaim(rcur[p], p) ELSE cl
               /\ UNCHANGED rcur
          ELSE /\ cl' = Unclaim(rcur[p], p)
               /\ rst' = Set(rst, p, "scan")
               /\ rcur' = Set(rcur, p, 0)
               /\ UNCHANGED ll
    /\ UNCHANGED <<core, rdata, ovf, rscan, rlist>>

RegWrite(p) ==
    /\ rst[p] = "locked"
    /\ etok' = Set(etok, rcur[p], p)
    /\ edr' = Set(edr, rcur[p], FALSE)
    /\ rst' = Set(rst, p, "written")
    /\ UNCHANGED <<core, rlock, ovf, rscan, rcur, rlist>>

RegRelease(p) ==
    /\ rst[p] = "written"
    /\ cl' = Unclaim(rcur[p], p)
    /\ rst' = Set(rst, p, "in")
    /\ att' = Set(att, p, "attached")
    /\ pc' = Set(pc, p, "idle")
    /\ areq' = Set(areq, p, "served")
    /\ UNCHANGED <<lease, schema, bystanders, ll, rdata, ovf, rscan, rcur, rlist>>

\* A handle that found every entry held attaches without one, and the registry
\* records that some handle is missing from it.
RegOverflow(p) ==
    /\ rst[p] = "scan" /\ rscan[p] = {}
    /\ ovf' = TRUE
    /\ rst' = Set(rst, p, "unreg")
    /\ att' = Set(att, p, "attached")
    /\ pc' = Set(pc, p, "idle")
    /\ areq' = Set(areq, p, "served")
    /\ UNCHANGED <<lease, schema, bystanders, rlock, rdata, rscan, rcur, rlist>>

\* An owner updates its entry when its draining state changes, under the claim
\* lock; if the claim lock is held it does not wait and tries again later.
UpdTake(p) ==
    /\ rst[p] = "in" /\ att[p] \in {"attached", "draining"}
    /\ edr[rcur[p]] # (att[p] = "draining") /\ cl[rcur[p]] = None
    /\ cl' = Set(cl, rcur[p], p)
    /\ rst' = Set(rst, p, "upd")
    /\ UNCHANGED <<core, ll, rdata, ovf, rscan, rcur, rlist>>

UpdDone(p) ==
    /\ rst[p] = "upd"
    /\ edr' = Set(edr, rcur[p], att[p] = "draining")
    /\ cl' = Unclaim(rcur[p], p)
    /\ rst' = Set(rst, p, "in")
    /\ UNCHANGED <<core, ll, etok, ovf, rscan, rcur, rlist>>

\* A handle leaves the registry before it lets go of its tiers: it claims its
\* entry without waiting, clears it, and releases the liveness lock and then the
\* claim lock. If the claim lock is held it does not wait: it releases the
\* liveness lock alone, which leaves the entry to read as unused.
UnregNone(p) ==
    /\ att[p] = "unregistering" /\ rst[p] = "unreg"
    /\ rst' = Set(rst, p, "out")
    /\ UNCHANGED <<core, rlock, rdata, ovf, rscan, rcur, rlist>>

UnregClaim(p) ==
    /\ att[p] = "unregistering" /\ rst[p] = "in" /\ cl[rcur[p]] = None
    /\ cl' = Set(cl, rcur[p], p)
    /\ rst' = Set(rst, p, "uc")
    /\ UNCHANGED <<core, ll, rdata, ovf, rscan, rcur, rlist>>

UnregSkip(p) ==
    /\ att[p] = "unregistering" /\ rst[p] = "in" /\ cl[rcur[p]] # None
    /\ ll' = Set(ll, rcur[p], None)
    /\ rst' = Set(rst, p, "out")
    /\ rcur' = Set(rcur, p, 0)
    /\ UNCHANGED <<core, cl, rdata, ovf, rscan, rlist>>

UnregClear(p) ==
    /\ rst[p] = "uc"
    /\ etok' = Set(etok, rcur[p], None)
    /\ edr' = Set(edr, rcur[p], FALSE)
    /\ rst' = Set(rst, p, "ucl")
    /\ UNCHANGED <<core, rlock, ovf, rscan, rcur, rlist>>

UnregRelease(p) ==
    /\ rst[p] = "ucl"
    /\ ll' = Set(ll, rcur[p], None)
    /\ cl' = Unclaim(rcur[p], p)
    /\ rst' = Set(rst, p, "out")
    /\ rcur' = Set(rcur, p, 0)
    /\ UNCHANGED <<core, rdata, ovf, rscan, rlist>>

\* Out of the registry, the handle lets go of its tiers: a handle that is being
\* dropped ends, and an upgrader goes on to ask for the gate.
Detach(p) ==
    /\ att[p] = "unregistering" /\ rst[p] = "out"
    /\ IF pc[p] = "detaching"
          THEN /\ lph[p] = "none"
               /\ pc' = Set(pc, p, "exited")
               /\ att' = Set(att, p, "off")
               /\ Forget(p)
          ELSE /\ att' = Set(att, p, IF Control = "upgrader-keeps-tiers"
                                        THEN "uWGateT" ELSE "uWGate")
               /\ UNCHANGED <<pc, handle, reader>>
    /\ UNCHANGED <<areq, lease, schema, req, eligible, coord, store, diedIn,
                   counts, reg>>

\* A listing runs on a thread of its own, on a process that need not be
\* attached, and visits the entries in turn: it claims an entry without
\* waiting, reporting it as unknown if it cannot, reads its token, and tries
\* the liveness lock in shared mode, which is released at once, reporting the
\* entry live only if the attempt was refused. It says at the end whether it
\* may be incomplete.
ListStart(p) ==
    /\ RegistryOn /\ lph[p] = "none" /\ lists < MaxListings
    /\ pc[p] \notin {"exited", "dead"}
    /\ lph' = Set(lph, p, "next")
    /\ lcur' = Set(lcur, p, 1)
    /\ lunk' = Set(lunk, p, FALSE)
    /\ lfree' = Set(lfree, p, FALSE)
    /\ lists' = lists + 1
    /\ UNCHANGED <<core, rlock, rdata, ovf, rown, ltok, lrep, lres>>

\* Under Control = "list-without-claim" it does not claim the entry at all.
ListClaim(p) ==
    /\ lph[p] = "next" /\ lcur[p] <= NSlots
    /\ IF Control = "list-without-claim"
          THEN /\ lph' = Set(lph, p, "held")
               /\ UNCHANGED <<cl, lcur, lunk, lrep>>
          ELSE IF cl[lcur[p]] = None
          THEN /\ cl' = Set(cl, lcur[p], p)
               /\ lph' = Set(lph, p, "held")
               /\ UNCHANGED <<lcur, lunk, lrep>>
          ELSE /\ lrep' = Set(lrep, p, <<lcur[p], None, "unknown">>)
               /\ lunk' = Set(lunk, p, TRUE)
               /\ lcur' = Set(lcur, p, lcur[p] + 1)
               /\ UNCHANGED <<cl, lph>>
    /\ UNCHANGED <<core, ll, rdata, ovf, rown, ltok, lfree, lres, lists>>

ListRead(p) ==
    /\ lph[p] = "held"
    /\ ltok' = Set(ltok, p, etok[lcur[p]])
    /\ lph' = Set(lph, p, "read")
    /\ UNCHANGED <<core, rlock, rdata, ovf, rown, lcur, lrep, lunk, lfree, lres,
                   lists>>

ListProbe(p) ==
    /\ lph[p] = "read"
    /\ lrep' = Set(lrep, p,
                   <<lcur[p], ltok[p],
                     IF ltok[p] = None THEN "free"
                     ELSE IF ll[lcur[p]] = None THEN "dead" ELSE "live">>)
    /\ lfree' = Set(lfree, p, lfree[p] \/ ltok[p] = None \/ ll[lcur[p]] = None)
    /\ cl' = Unclaim(lcur[p], p)
    /\ lcur' = Set(lcur, p, lcur[p] + 1)
    /\ lph' = Set(lph, p, "next")
    /\ UNCHANGED <<core, ll, rdata, ovf, rown, ltok, lunk, lres, lists>>

\* The listing says it may be incomplete when it could not claim an entry or when
\* the registry records that a handle attached without one; under Control =
\* "listing-infers-from-fullness" it says so only when no entry was free.
ListDone(p) ==
    /\ lph[p] = "next" /\ lcur[p] > NSlots
    /\ lres' = Set(lres, p,
                   IF lunk[p] \/ (IF Control = "listing-infers-from-fullness"
                                     THEN ~lfree[p] ELSE ovf)
                      THEN "incomplete" ELSE "complete")
    /\ lph' = Set(lph, p, "none")
    /\ UNCHANGED <<core, rlock, rdata, ovf, rown, lcur, ltok, lrep, lunk, lfree,
                   lists>>

-----------------------------------------------------------------------------
(* Writer exclusion (DESIGN-NOTES "Writer exclusion", "Ownership").        *)

\* The request is served when the lock is granted. Whenever it is free this is
\* enabled for every eligible waiter.
Lock(p) ==
    /\ pc[p] = "waiting" /\ eligible[p]
    /\ lockHolder = None
    /\ lockHolder' = p
    /\ pc' = Set(pc, p, "locked")
    /\ req' = Set(req, p, "served")
    /\ UNCHANGED <<adm, eligible, owner, fence, trusted, count, gen, store, handle,
                   reader, diedIn, counts>>

\* Under the lock the ownership record, the fence and the trust flag cannot
\* change. A record found here was left by a holder that died, because a clean
\* release clears the record before it drops the lock.

\* The fence is set before the abandoned record is replaced, so the evidence
\* outlives the acquirer's own clean release.
SetFence(p) ==
    /\ pc[p] = "locked" /\ owner # None /\ ~fence
    /\ Control # "replace-without-fence"
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, pc, req, eligible, lockHolder, owner, trusted, count, gen,
                   store, handle, reader, diedIn, counts>>

\* The acquirer records itself as holder, advances the generation, and is told
\* that recovery is required when it found a record, the fence set, or state it
\* cannot vouch for. It keeps its cache of shared only if the generation it
\* found is the one it last advanced to. It recovers nothing, and a told holder
\* writes nothing.
Record(p) ==
    /\ pc[p] = "locked"
    /\ (owner = None \/ fence \/ Control = "replace-without-fence")
    /\ owner' = p
    /\ gen' = gen + 1
    /\ mygen' = Set(mygen, p, gen + 1)
    /\ cache' = Set(cache, p,
                    IF cache[p] # NoVal
                          /\ (mygen[p] = gen \/ Control = "cache-always-kept")
                       THEN cache[p] ELSE shared)
    /\ pc' = Set(pc, p,
                 IF owner # None \/ fence
                       \/ (~trusted /\ Control # "ignore-restart")
                    THEN "told" ELSE "held")
    /\ UNCHANGED <<adm, req, eligible, lockHolder, fence, trusted, count, store,
                   reader, diedIn, counts>>

\* The opening raise is prepared, then published.
BeginOpen(p) ==
    /\ pc[p] = "held"
    /\ pc' = Set(pc, p, "opening")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

\* Publishing makes the new count visible at once, and any cached sample of the
\* old one is invalidated before the publication takes effect.
Cleared == IF Control = "late-invalidation" THEN scache
           ELSE [q \in Procs |-> NoVal]

EndOpen(p) ==
    /\ pc[p] = "opening"
    /\ count' = count + 1
    /\ pc' = Set(pc, p, "open")
    /\ scache' = Cleared
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, fence, trusted, gen, store,
                   handle, rphase, rbefore, rseenA, rseenB, rpart, diedIn,
                   counts>>

\* A failed opening publication makes no effect and leaves the count as it was.
OpenFails(p) ==
    /\ pc[p] = "opening"
    /\ pc' = Set(pc, p, "closed")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

\* The write's first and last effects. The first changes shared, and the
\* holder's cache with it.
FirstEffect(p) ==
    /\ pc[p] = "open"
    /\ pc' = Set(pc, p, "writing")
    /\ a' = a + 1
    /\ shared' = shared + 1
    /\ cache' = Set(cache, p, shared + 1)
    /\ UNCHANGED <<adm, req, eligible, coord, b, mygen, reader, diedIn, counts>>

LastEffect(p) ==
    /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, "wrote")
    /\ b' = b + 1
    /\ UNCHANGED <<adm, req, eligible, coord, a, shared, handle, reader, diedIn,
                   counts>>

\* An operation that finds nothing to change ends without an effect, and one
\* that fails after its first effect but can be rolled back undoes it; either
\* then publishes the closing raise, so the count is even again.
EndEmpty(p) ==
    /\ pc[p] = "open"
    /\ pc' = Set(pc, p, "wrote")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

Rollback(p) ==
    /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, "wrote")
    /\ a' = b
    /\ shared' = shared - 1
    /\ cache' = Set(cache, p, shared - 1)
    /\ UNCHANGED <<adm, req, eligible, coord, b, mygen, reader, diedIn, counts>>

\* An operation that fails after its first effect and cannot be rolled back sets
\* the fence and leaves the count odd.
FailUnresolved(p) ==
    /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

\* An operation nested under the hold changes nothing: the depth is tracked, so
\* an inner operation never restores even parity early.
EnterInner(p) ==
    /\ pc[p] = "writing"
    /\ pc' = Set(pc, p, "nested")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

LeaveInner(p) ==
    /\ pc[p] = "nested"
    /\ pc' = Set(pc, p, "writing")
    /\ count' = IF Control = "inner-restores-parity" THEN count + 1 ELSE count
    /\ scache' = IF Control = "inner-restores-parity" THEN Cleared ELSE scache
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, fence, trusted, gen, store,
                   handle, rphase, rbefore, rseenA, rseenB, rpart, diedIn,
                   counts>>

\* The closing raise is prepared, then published.
BeginClose(p) ==
    /\ pc[p] = "wrote"
    /\ pc' = Set(pc, p, "closing")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>

EndClose(p) ==
    /\ pc[p] = "closing"
    /\ count' = count + 1
    /\ pc' = Set(pc, p, "closed")
    /\ scache' = Cleared
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, fence, trusted, gen, store,
                   handle, rphase, rbefore, rseenA, rseenB, rpart, diedIn,
                   counts>>

\* A failed closing publication leaves the count odd and sets the fence, so
\* recovery, which republishes it, is required.
CloseFails(p) ==
    /\ pc[p] = "closing"
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

\* A clean release clears the ownership record while still holding the lock,
\* then drops the lock. A holder with an unresolved operation cannot release
\* cleanly.
Release(p) ==
    /\ pc[p] \in {"held", "closed", "told"}
    /\ IF Control = "unlock-before-clear"
          THEN /\ lockHolder' = None
               /\ owner' = owner
               /\ pc' = Set(pc, p, "unlocked")
          ELSE /\ lockHolder' = lockHolder
               /\ owner' = None
               /\ pc' = Set(pc, p, "releasing")
    /\ UNCHANGED <<adm, req, eligible, fence, trusted, count, gen, store, handle,
                   reader, diedIn, counts>>

Unlock(p) ==
    /\ pc[p] = "releasing"
    /\ lockHolder' = None
    /\ pc' = Set(pc, p, "idle")
    /\ UNCHANGED <<adm, req, eligible, owner, fence, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

\* Only under Control = "unlock-before-clear": removes whatever record is
\* there, which may by now be another process's.
ClearRecord(p) ==
    /\ pc[p] = "unlocked"
    /\ owner' = None
    /\ pc' = Set(pc, p, "idle")
    /\ UNCHANGED <<adm, req, eligible, lockHolder, fence, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>

-----------------------------------------------------------------------------
(* Reads (DESIGN-NOTES "Counters"). A reader holds nothing.                *)

\* The reader's last sample ends the read.
FinishRead(p, v) ==
    /\ rphase' = Set(rphase, p, "none")
    /\ rbefore' = Set(rbefore, p, NoVal)
    /\ rseenA' = Set(rseenA, p, NoVal)
    /\ rseenB' = Set(rseenB, p, NoVal)
    /\ rpart' = Set(rpart, p, NoVal)
    /\ scache' = Set(scache, p, IF Control = "assembled-sample" THEN NoVal
                                ELSE v)

\* The first sample, in one step; as two parts, high half first, under
\* Control = "assembled-sample".
ReadBefore(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "none" /\ reads < MaxReads
    /\ reads' = reads + 1
    /\ IF Control = "assembled-sample"
          THEN /\ rpart' = Set(rpart, p, count \div 2)
               /\ rphase' = Set(rphase, p, "before2")
               /\ UNCHANGED <<adm, rbefore, scache>>
          ELSE /\ rbefore' = Set(rbefore, p, Sampled(p))
               /\ scache' = Set(scache, p, Sampled(p))
               /\ rphase' = Set(rphase, p, "obsA")
               /\ UNCHANGED rpart
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rseenA, rseenB,
                   diedIn, requests, deaths, crashes>>

ReadBefore2(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "before2"
    /\ rbefore' = Set(rbefore, p, 2 * rpart[p] + (count % 2))
    /\ rphase' = Set(rphase, p, "obsA")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rseenA, rseenB,
                   rpart, scache, diedIn, counts>>

ObserveA(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "obsA"
    /\ rseenA' = Set(rseenA, p, a)
    /\ rphase' = Set(rphase, p, "obsB")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rbefore, rseenB,
                   rpart, scache, diedIn, counts>>

ObserveB(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "obsB"
    /\ rseenB' = Set(rseenB, p, b)
    /\ rphase' = Set(rphase, p, "after1")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rbefore, rseenA,
                   rpart, scache, diedIn, counts>>

\* The second sample ends the read: it is settled when the two samples are
\* equal and even, and contended otherwise.
ReadAfter(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "after1" /\ Control # "assembled-sample"
    /\ FinishRead(p, Sampled(p))
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, diedIn, counts>>

ReadAfter1(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "after1" /\ Control = "assembled-sample"
    /\ rpart' = Set(rpart, p, count \div 2)
    /\ rphase' = Set(rphase, p, "after2")
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rbefore, rseenA,
                   rseenB, scache, diedIn, counts>>

ReadAfter2(p) ==
    /\ pc[p] = "idle" /\ rphase[p] = "after2"
    /\ FinishRead(p, 2 * rpart[p] + (count % 2))
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, diedIn, counts>>

\* A cached sample may be dropped at any time.
Invalidate(p) ==
    /\ scache[p] # NoVal
    /\ scache' = Set(scache, p, NoVal)
    /\ UNCHANGED <<adm, pc, req, eligible, coord, store, handle, rphase, rbefore,
                   rseenA, rseenB, rpart, diedIn, counts>>

-----------------------------------------------------------------------------
(* Death and crash.                                                        *)

\* EXTERNAL: one process dies at any step. The OS releases its lock; nothing
\* it left in the filesystem changes, and the ownership record and the count
\* stay as published.
Die(p) ==
    /\ deaths < MaxDeaths
    /\ pc[p] \in LiveStates
    /\ pc' = Set(pc, p, "dead")
    /\ lockHolder' = IF lockHolder = p THEN None ELSE lockHolder
    /\ req' = IF pc[p] = "waiting" THEN Set(req, p, "dead") ELSE req
    /\ diedIn' = IF owner = p THEN pc[p] ELSE diedIn
    /\ deaths' = deaths + 1
    /\ att' = Set(att, p, "off")
    /\ areq' = IF areq[p] = "pending" THEN Set(areq, p, "dead") ELSE areq
    /\ lease' = Set(lease, p, 0)
    /\ Forget(p)
    /\ cl' = [s \in Slots |-> IF cl[s] = p THEN None ELSE cl[s]]
    /\ ll' = [s \in Slots |-> IF ll[s] = p THEN None ELSE ll[s]]
    /\ rst' = Set(rst, p, "out")
    /\ rscan' = Set(rscan, p, {})
    /\ rcur' = Set(rcur, p, 0)
    /\ lph' = Set(lph, p, "none")
    /\ lcur' = Set(lcur, p, 1)
    /\ ltok' = Set(ltok, p, None)
    /\ lrep' = Set(lrep, p, NoRep)
    /\ lunk' = Set(lunk, p, FALSE)
    /\ lfree' = Set(lfree, p, FALSE)
    /\ lres' = Set(lres, p, "none")
    /\ UNCHANGED <<rdata, ovf, lists>>
    /\ UNCHANGED <<schema, eligible, owner, fence, trusted, count, gen, store,
                   requests, crashes, reads>>

\* EXTERNAL: power loss. Every process dies and every lock is released. The
\* coordination state cannot be vouched for afterwards, and the record, the
\* fence and the count may or may not have survived: the count may roll back
\* over a writer that was mid-operation.
Crash ==
    /\ crashes < MaxCrashes
    /\ pc' = [p \in Procs |-> IF pc[p] \in LiveStates THEN "dead" ELSE pc[p]]
    /\ req' = [p \in Procs |-> IF pc[p] = "waiting" THEN "dead" ELSE req[p]]
    /\ lockHolder' = None
    /\ owner' \in {owner, None}
    /\ fence' \in {fence, FALSE}
    /\ trusted' = FALSE
    /\ count' \in {count, IF Odd(count) THEN count - 1 ELSE count}
    /\ diedIn' = IF owner # None THEN "crash" ELSE diedIn
    /\ crashes' = crashes + 1
    /\ mygen' = [p \in Procs |-> NoVal]
    /\ cache' = [p \in Procs |-> NoVal]
    /\ rphase' = [p \in Procs |-> "none"]
    /\ rbefore' = [p \in Procs |-> NoVal]
    /\ rseenA' = [p \in Procs |-> NoVal]
    /\ rseenB' = [p \in Procs |-> NoVal]
    /\ rpart' = [p \in Procs |-> NoVal]
    /\ scache' = [p \in Procs |-> NoVal]
    /\ att' = [p \in Procs |-> "off"]
    /\ areq' = [p \in Procs |-> IF areq[p] = "pending" THEN "dead" ELSE areq[p]]
    /\ lease' = [p \in Procs |-> 0]
    /\ cl' = [s \in Slots |-> None]
    /\ ll' = [s \in Slots |-> None]
    /\ etok' = [s \in Slots |-> None]
    /\ edr' = [s \in Slots |-> FALSE]
    /\ ovf' = FALSE
    /\ rst' = [p \in Procs |-> "out"]
    /\ rscan' = [p \in Procs |-> {}]
    /\ rcur' = [p \in Procs |-> 0]
    /\ lph' = [p \in Procs |-> "none"]
    /\ lcur' = [p \in Procs |-> 1]
    /\ ltok' = [p \in Procs |-> None]
    /\ lrep' = [p \in Procs |-> NoRep]
    /\ lunk' = [p \in Procs |-> FALSE]
    /\ lfree' = [p \in Procs |-> FALSE]
    /\ lres' = [p \in Procs |-> "none"]
    /\ UNCHANGED <<schema, eligible, gen, store, requests, deaths, reads, lists>>

\* Every process has ended. It is the only step then, so a state in which a
\* process is stuck waiting is the only kind of deadlock.
Terminal ==
    /\ \A p \in Procs : pc[p] \in {"exited", "dead"}
    /\ UNCHANGED vars

Next ==
    \/ \E p \in Procs :
          \/ Exit(p) \/ Disqualify(p)
          \/ BeginAttach(p) \/ TakeGateS(p) \/ TakeReaderS(p) \/ DropGate(p)
          \/ KeepGate(p) \/ CheckEarly(p) \/ TakeWriterS(p) \/ Admit(p)
          \/ GiveUp(p) \/ BeginOp(p) \/ EndOp(p)
          \/ StartUpgrade(p) \/ RefuseUpgrade(p) \/ DrainDone(p)
          \/ CancelDrain(p) \/ StartUpgradeHolding(p) \/ TakeGateX(p)
          \/ RegStart(p) \/ (\E s \in Slots : RegClaim(p, s)) \/ RegLive(p)
          \/ RegWrite(p) \/ RegRelease(p) \/ RegOverflow(p)
          \/ UpdTake(p) \/ UpdDone(p) \/ UnregNone(p) \/ UnregClaim(p)
          \/ UnregSkip(p) \/ UnregClear(p) \/ UnregRelease(p) \/ Detach(p)
          \/ ListStart(p) \/ ListClaim(p) \/ ListRead(p) \/ ListProbe(p)
          \/ ListDone(p)
          \/ TakeFirstTier(p) \/ TakeSecondTier(p) \/ BeginUpgrade(p)
          \/ UpgradeMoved(p) \/ EndUpgrade(p)
          \/ Request(p) \/ Withdraw(p) \/ Refuse(p)
          \/ Lock(p) \/ SetFence(p) \/ Record(p)
          \/ BeginOpen(p) \/ EndOpen(p) \/ OpenFails(p)
          \/ FirstEffect(p) \/ LastEffect(p) \/ FailUnresolved(p)
          \/ EndEmpty(p) \/ Rollback(p)
          \/ EnterInner(p) \/ LeaveInner(p)
          \/ BeginClose(p) \/ EndClose(p) \/ CloseFails(p)
          \/ Release(p) \/ Unlock(p) \/ ClearRecord(p)
          \/ ReadBefore(p) \/ ReadBefore2(p) \/ ObserveA(p) \/ ObserveB(p)
          \/ ReadAfter(p) \/ ReadAfter1(p) \/ ReadAfter2(p) \/ Invalidate(p)
          \/ Die(p)
    \/ Crash
    \/ Terminal

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties.                                                             *)

\* Something an acquirer can see that tells it a holder died or the state is
\* not to be trusted.
Evidence == fence \/ ~trusted \/ (owner # None /\ pc[owner] = "dead")

\* At most one live process holds writer exclusion, and the lock names it.
AtMostOneHolder ==
    Cardinality({p \in Procs : pc[p] \in HoldsLock}) <= 1

LockMatchesHolders ==
    \A p \in Procs : (pc[p] \in HoldsLock) <=> (lockHolder = p)

\* The record names the process that holds exclusion, or one that died; never
\* a process that is alive and not holding.
OwnerRecordNamesItsHolder ==
    /\ \A p \in Procs : pc[p] \in OwnsRecord => owner = p
    /\ owner # None => pc[owner] \in OwnsRecord \cup {"dead"}

\* A holder that died holding exclusion stays reported until recovery, even
\* when it died before its first effect or after its last.
AbandonmentIsNeverForgotten == diedIn # "none" => Evidence

\* Half-applied data is either its live writer's or visible as unresolved.
UnresolvedWorkIsNeverInvisible ==
    Partial => (\E p \in Procs : pc[p] \in {"writing", "nested"}) \/ Evidence

\* An abandoned record is replaced only after the fence (or the untrusted
\* state) carries what it said.
AbandonedRecordNeverReplacedUnfenced ==
    [][(owner # None /\ pc[owner] = "dead" /\ owner' # owner)
          => (fence' \/ ~trusted')]_vars

\* A holder that is not told recovery is required finds a store with nothing
\* to recover.
NormalAcquireFindsAHealthyStore ==
    [][\A p \in Procs :
          (pc[p] = "locked" /\ pc'[p] = "held")
             => (owner = None /\ ~fence /\ trusted /\ ~Partial /\ Even(count))
      ]_vars

\* An acquirer is told recovery is required only with cause, and told of an
\* abandoned record only when the record's holder really died.
ToldOnlyWithCause ==
    [][\A p \in Procs :
          (pc[p] = "locked" /\ pc'[p] = "told")
             => (owner # None \/ fence \/ ~trusted)]_vars

NoFalseAbandonmentReport ==
    [][\A p \in Procs :
          (pc[p] = "locked" /\ pc'[p] = "told" /\ owner # None)
             => pc[owner] = "dead"]_vars

\* Attaching, acquiring and a process's death change no store data: only a
\* healthy holder in the middle of a write does.
StoreDataChangesOnlyByAHealthyHolder ==
    [][store' # store =>
          \E p \in Procs : pc[p] \in {"open", "writing"} /\ lockHolder = p
                           /\ ~fence /\ trusted]_vars

\* A process's death changes none of the coordination files, only its own
\* state and the OS lock it held.
DeathLeavesTheFilesystemAsItWas ==
    [][((\E p \in Procs : pc[p] \in LiveStates /\ pc'[p] = "dead")
            /\ crashes' = crashes)
          => UNCHANGED <<schema, store, rdata, ovf, owner, fence, trusted, count,
                         gen>>]_vars

\* A host crash leaves no live process.
CrashLeavesNoLiveProcess ==
    [][crashes' # crashes => \A p \in Procs : pc'[p] \notin LiveStates]_vars

\* A request is pending exactly while its process waits.
RequestStateMatchesProcess ==
    \A p \in Procs : (req[p] = "pending") <=> (pc[p] = "waiting")

OutcomeIsJustified ==
    \A p \in Procs :
        /\ req[p] = "dead" => pc[p] = "dead"
        /\ req[p] = "refused" => ~eligible[p]

\* A request leaves "pending" in exactly one of the four outcomes, which then
\* does not change until the process makes another request, and each outcome
\* comes with the process state that explains it.
EveryRequestEndsInExactlyOneOutcome ==
    [][\A p \in Procs :
          /\ req[p] = "none" => req'[p] \in {"none", "pending"}
          /\ req[p] \in Outcomes => req'[p] \in {req[p], "pending"}
          /\ (pc[p] = "waiting" /\ pc'[p] # "waiting") =>
                \/ pc'[p] = "locked" /\ req'[p] = "served" /\ eligible[p]
                \/ pc'[p] = "dead" /\ req'[p] = "dead"
                \/ pc'[p] = "idle" /\ req'[p] = "withdrawn"
                \/ pc'[p] = "idle" /\ req'[p] = "refused" /\ ~eligible[p]
      ]_vars

\* The count is odd whenever an operation is unresolved: while one is open, and
\* for as long as its data is half applied, whether or not its holder lives.
CountIsOddWhileAnOperationIsUnresolved ==
    /\ Partial => Odd(Visible)
    /\ \A p \in Procs : pc[p] \in OpenStates => Odd(Visible)

\* The count is raised only by the holder publishing it, and by nothing else
\* but a host crash.
CountIsRaisedOnlyByItsHolder ==
    [][(count' # count /\ crashes' = crashes) =>
          \E p \in Procs :
              /\ lockHolder = p
              /\ \/ pc[p] = "opening" /\ pc'[p] = "open"
                 \/ pc[p] = "closing" /\ pc'[p] = "closed"]_vars

\* Every effect is ordered after the opening raise, and before the closing one.
EffectsAreBracketedByTheCount ==
    [][store' # store => Odd(count)]_vars

\* A failed opening publication has no effect.
FailedOpeningPublicationHasNoEffect ==
    [][\A p \in Procs :
          (pc[p] = "opening" /\ pc'[p] = "closed")
             => (count' = count /\ store' = store)]_vars

\* A failed closing publication, or an operation that fails unresolved, leaves
\* the count odd and sets the fence.
FailedOperationsLeaveTheCountOddAndFence ==
    [][\A p \in Procs :
          (pc[p] \in {"closing", "writing"} /\ pc'[p] = "told")
             => (Odd(count') /\ fence')]_vars

\* A host crash is the only thing that changes the published count other than
\* its holder; and no step but an acquisition changes the generation.
GenerationAdvancesOnEveryAcquisitionAndOnlyThen ==
    [][/\ \A p \in Procs : (pc[p] = "locked" /\ pc'[p] \in {"held", "told"})
                              => gen' = gen + 1
       /\ gen' # gen => \E p \in Procs :
                           pc[p] = "locked" /\ pc'[p] \in {"held", "told"}
      ]_vars

\* A handle whose remembered generation is the current one has a cache that is
\* accurate, so it may use it without reading shared again.
GenerationMatchImpliesAnAccurateCache ==
    \A p \in Procs : (mygen[p] = gen /\ cache[p] # NoVal) => cache[p] = shared

\* A holder's cache of shared is accurate.
AHolderCacheIsAccurate ==
    \A p \in Procs : pc[p] \in OwnsRecord => cache[p] = shared

\* A cached sample is never stale.
ACachedSampleIsNeverStale ==
    \A p \in Procs : scache[p] # NoVal => scache[p] = Visible

\* The reader's bookkeeping is consistent with where it is.
ReadersAreConsistent ==
    \A p \in Procs :
        /\ rphase[p] = "none" =>
              (rbefore[p] = NoVal /\ rseenA[p] = NoVal /\ rseenB[p] = NoVal
               /\ rpart[p] = NoVal)
        /\ rphase[p] \in {"obsA", "obsB", "after1", "after2"} =>
              rbefore[p] # NoVal
        /\ rphase[p] \in {"obsB", "after1", "after2"} => rseenA[p] # NoVal
        /\ rphase[p] \in {"after1", "after2"} => rseenB[p] # NoVal
        /\ rphase[p] # "none" => pc[p] = "idle"

\* An operation that has finished its effects, by completing or by rolling
\* back, leaves the store settled.
AFinishedOperationLeavesTheStoreSettled ==
    \A p \in Procs : pc[p] \in {"wrote", "closing", "closed"} => a = b

\* A read whose last step this is, and the value of its second sample.
Finishing(p) ==
    /\ rphase[p] \in {"after1", "after2"}
    /\ rphase'[p] = "none" /\ pc'[p] = "idle"
FinalSample(p) ==
    IF rphase[p] = "after2" THEN 2 * rpart[p] + (count % 2) ELSE Sampled(p)
Accepted(p) == Finishing(p) /\ Undisturbed(rbefore[p], FinalSample(p))

\* The read classification: a read classified as settled saw the whole of one
\* settled store, with no part of an operation in it, and nothing has changed
\* since. This is what makes a snapshot, and a verdict that a disagreement is
\* real rather than raced, sound.
SettledReadSawNoPartOfAnOperation ==
    [][\A p \in Procs :
          Accepted(p) => (rseenA[p] = a /\ rseenB[p] = b /\ a = b)]_vars

-----------------------------------------------------------------------------
(* The gate and the tiers.                                                 *)

\* An exclusive gate holder coexists with no other admission: nobody else holds
\* the gate in either mode.
AnExclusiveGateHolderCoexistsWithNoOtherAdmission ==
    /\ Cardinality({p \in Procs : HoldsGateX(p)}) <= 1
    /\ GateXHeld => ~GateSHeld

\* Tiers held exclusively are held by one handle and shared by none.
TierHoldersAreCompatible ==
    /\ Cardinality({p \in Procs : HoldsReaderX(p)}) <= 1
    /\ ReaderXHeld => ~ReaderSHeld
    /\ Cardinality({p \in Procs : HoldsWriterX(p)}) <= 1
    /\ WriterXHeld => ~WriterSHeld

\* Holders of the gate in its shared mode never block one another, and hold it
\* only until they hold their reader tier.
OrdinaryAdmissionHoldsTheGateOnlyUntilItHoldsTheReaderTier ==
    \A p \in Procs : HoldsGateS(p) => att[p] \in {"hasGate", "gateRd"}

NoHandleWaitsForTheGateWhileHoldingATier ==
    \A p \in Procs :
        att[p] \in {"wGate", "uWGate", "uWGateT"} =>
            ~(HoldsReaderS(p) \/ HoldsWriterS(p)
              \/ HoldsReaderX(p) \/ HoldsWriterX(p))

\* The resources are ranked gate, reader tier, writer tier, writer exclusion,
\* and a process waits only for one ranked above every one it holds.
HeldRanks(p) ==
    {r \in 1..4 :
        \/ r = 1 /\ (HoldsGateS(p) \/ HoldsGateX(p))
        \/ r = 2 /\ (HoldsReaderS(p) \/ HoldsReaderX(p))
        \/ r = 3 /\ (HoldsWriterS(p) \/ HoldsWriterX(p))
        \/ r = 4 /\ lockHolder = p}

WaitRank(p) ==
    CASE att[p] \in {"wGate", "uWGate", "uWGateT"} -> 1
      [] att[p] = "hasGate" -> 2
      [] att[p] = "uGate" ->
            IF TakesReaderTier(p) /\ Control # "unsafe-writer-tier-first"
               THEN 2 ELSE 3
      [] att[p] = "uWr" -> 2
      [] att[p] \in {"rd", "gateWr", "rdChecked", "uRd"} -> 3
      [] pc[p] = "waiting" -> 4
      [] OTHER -> 0

NothingIsAcquiredAgainstTheOrder ==
    \A p \in Procs :
        WaitRank(p) > 0 => \A r \in HeldRanks(p) : r < WaitRank(p)

\* Writer exclusion is held only by an attached handle that may write.
ExclusionIsHeldOnlyByAttachedWriters ==
    lockHolder = None \/ (att[lockHolder] = "attached" /\ CanWrite(lockHolder))

\* No handle is admitted against the rules in force at the time, and none stays
\* attached against them as the store changes beneath it.
AdmittedHandlesAreAdmissible ==
    [][\A p \in Procs :
          (att[p] # "attached" /\ att'[p] = "attached") => Admissible(p)]_vars

AttachedHandlesStayAdmissible ==
    \A p \in Procs : att[p] = "attached" => Admissible(p)

\* An upgrade begins only from the schema its upgrader found, with no other
\* upgrade recorded in progress.
AnUpgradeBeginsFromTheSchemaItExpects ==
    [][\A p \in Procs :
          (att[p] = "uTiers" /\ att'[p] = "uBody") =>
              (current[UpLayer] = SpeaksOf[p][UpLayer] /\ upState = "ordinary")]_vars

\* The current schema moves exactly when an upgrade completes, and the oldest
\* reader schema exactly when one that is unsafe for readers does.
SchemasMoveOnlyWhenAnUpgradeCompletes ==
    [][/\ (current' # current) <=> (upState # "ordinary" /\ upState' = "ordinary")
       /\ (oldest' # oldest) <=> (upState = "unsafe" /\ upState' = "ordinary")]_vars

\* What each process's attachment state says about where it is.
AttachmentMatchesTheProcess ==
    \A p \in Procs :
        /\ pc[p] = "new" => att[p] = "off"
        /\ pc[p] \in {"exited", "dead"} => att[p] = "off" /\ lease[p] = 0
        /\ pc[p] = "detached" => att[p] = "off"
        /\ pc[p] \in LiveStates \ {"attaching", "converting", "convertingX",
                                  "detached", "detaching"}
              => att[p] = "attached"
        /\ pc[p] = "detaching" => att[p] = "unregistering"
        /\ pc[p] = "attaching" =>
              att[p] \in {"wGate", "hasGate", "gateRd", "gateWr", "rd",
                          "rdChecked", "tiers", "registering"}
        /\ pc[p] = "converting" =>
              att[p] \in {"draining", "unregistering", "uWGate", "uWGateT", "uGate",
                          "uRd", "uWr", "uTiers", "uBody"}

\* An attach or upgrade request is pending exactly while its process is
\* attaching or converting, and ends in exactly one of the four outcomes. A
\* call refused at the door is an outcome that leaves its process where it was.
AttachRequestStateMatchesProcess ==
    \A p \in Procs :
        (areq[p] = "pending")
            <=> (pc[p] \in {"attaching", "converting", "convertingX"})

AttachRequestsEndInExactlyOneOutcome ==
    [][\A p \in Procs :
          /\ areq[p] = "none" => areq'[p] \in {"none", "pending", "refused"}
          /\ areq[p] \in Outcomes =>
                areq'[p] \in {areq[p], "pending", "refused"}
          /\ (areq[p] # "pending" /\ areq'[p] \notin {areq[p], "pending"}) =>
                (areq'[p] = "refused" /\ pc'[p] = pc[p])
          /\ (areq[p] = "pending" /\ areq'[p] # "pending") =>
                \/ areq'[p] = "served" /\ pc'[p] \in {"idle", "exited", "held"}
                \/ areq'[p] = "dead" /\ pc'[p] = "dead"
                \/ areq'[p] = "withdrawn"
                      /\ pc'[p] \in {"idle", "exited", "detached"}
                \/ areq'[p] = "refused"
                      /\ pc'[p] \in {"exited", "held", "detached"}
      ]_vars

-----------------------------------------------------------------------------
(* The registry.                                                           *)

\* An entry's liveness lock is held only by a handle that holds an attachment,
\* so an entry exists exactly while its handle is attached: not before it is
\* admitted, and not once its attachment is given up, including by a conversion.
EntriesFollowAttachment ==
    \A s \in Slots : ll[s] # None => att[ll[s]] \in HoldsAttachment

\* A handle that is attached has an entry it holds, or the registry says that
\* some handle attached without one.
AttachedHandlesAreRegisteredOrAnnounced ==
    RegistryOn =>
        \A p \in Procs :
            att[p] \in {"attached", "draining"} =>
                \/ rst[p] \in {"in", "upd"}
                \/ rst[p] = "unreg" /\ ovf

ARegisteredHandleOwnsItsEntry ==
    \A p \in Procs :
        rst[p] \in {"in", "upd"} =>
            (ll[rcur[p]] = p /\ etok[rcur[p]] = p)

AnUnregisteredHandleIsAnnounced ==
    (\E p \in Procs : rst[p] = "unreg") => ovf

\* With no claim lock held, an entry's token names the handle that holds its
\* liveness lock: no one can see a new owner's lock beside an old owner's token.
AFreeClaimShowsTheTokenOfTheLivenessHolder ==
    \A s \in Slots : (cl[s] = None /\ ll[s] # None) => etok[s] = ll[s]

\* A death releases the locks of the process that died, and a clean exit has
\* already released its own.
RegistryLocksAreHeldOnlyByLiveProcesses ==
    \A s \in Slots :
        /\ cl[s] # None => pc[cl[s]] \notin {"dead", "exited"}
        /\ ll[s] # None => pc[ll[s]] \notin {"dead", "exited"}

\* Every change to an entry is made by the handle that holds its claim lock, and
\* what it writes is its own token, or a clearing. A crash discards the entries
\* with the rest of the coordination state.
EntryChangesAreMadeUnderTheClaimLock ==
    [][\A s \in Slots :
          ((etok'[s] # etok[s] \/ edr'[s] # edr[s]) /\ crashes' = crashes) =>
              (/\ cl[s] # None
               /\ (etok'[s] = etok[s] \/ etok'[s] \in {None, cl[s]}))]_vars

ACrashDiscardsTheRegistry ==
    [][crashes' # crashes =>
          (\A s \in Slots : etok'[s] = None /\ ~edr'[s]) /\ ~ovf']_vars

\* Every inspection of an entry is made under its claim lock.
EntryInspectionsAreMadeUnderTheClaimLock ==
    [][\A p \in Procs :
          ((lph[p] = "held" /\ lph'[p] = "read")
           \/ (lph[p] = "read" /\ lph'[p] = "next")) =>
              cl[lcur[p]] = p]_vars

\* A listing reports an entry live only if the liveness lock was held, as of its
\* probe, by the handle the token names, and that handle holds an attachment.
LiveReportIsJustified ==
    [][\A p \in Procs :
          (lph[p] = "read" /\ lph'[p] = "next" /\ lrep'[p][3] = "live") =>
              (ll[lrep'[p][1]] = lrep'[p][2]
               /\ att[lrep'[p][2]] \in HoldsAttachment)]_vars

\* A listing says it may be incomplete when it met an entry it could not claim,
\* and when some handle is attached without an entry.
AListingSaysWhenItMayBeIncomplete ==
    [][\A p \in Procs :
          (lph[p] = "next" /\ lph'[p] = "none" /\ pc'[p] # "dead"
           /\ crashes' = crashes) =>
              /\ lunk[p] => lres'[p] = "incomplete"
              /\ (\E q \in Procs : att[q] \in {"attached", "draining"}
                                  /\ rst[q] = "unreg") =>
                    lres'[p] = "incomplete"]_vars

-----------------------------------------------------------------------------
(* Clones, leases and the drain.                                           *)

ConversionWaits == {"uWGate", "uWGateT", "uGate", "uRd", "uWr", "uTiers"}

\* No operation is in flight beneath a conversion: the drain waited for every
\* lease before the upgrader released its tiers.
NoLeaseIsHeldOnceAConversionBegins ==
    \A p \in Procs :
        att[p] \in ConversionWaits \cup {"uBody"} =>
            (lease[p] = 0 /\ rphase[p] = "none")

\* An operation is in flight only while its handle holds the tiers it was
\* admitted under, so no upgrade completes beneath it.
AnOperationInFlightHoldsTheTiers ==
    \A p \in Procs :
        (lease[p] > 0 \/ rphase[p] # "none") =>
            (HoldsReaderS(p) /\ (CanWrite(p) => HoldsWriterS(p)))

\* An upgrade never waits for an operation that its own call is blocking.
NoDrainWaitsOnItsOwnCaller ==
    \A p \in Procs : att[p] = "draining" => rphase[p] = "none"

\* An upgrade is never started from inside an operation of its handle.
AnUpgradeNeverStartsFromInsideAnOperation ==
    [][\A p \in Procs :
          (pc[p] \notin {"converting", "convertingX"}
           /\ pc'[p] \in {"converting", "convertingX"}) => ~InsideOperation(p)]_vars

OperationStarts(p) ==
    \/ lease'[p] > lease[p]
    \/ rphase[p] = "none" /\ rphase'[p] # "none"
    \/ pc[p] = "idle" /\ pc'[p] = "waiting"

\* Neither a clone nor the handle starts an operation while its handle drains,
\* converts or is detached.
NoOperationStartsWhileTheHandleConverts ==
    [][\A p \in Procs : OperationStarts(p) => att[p] = "attached"]_vars

\* Cancelling the drain ends it and leaves the handle attached and usable.
ACancelledDrainLeavesTheHandleAttached ==
    [][\A p \in Procs :
          (att[p] = "draining" /\ areq'[p] = "withdrawn") =>
              (att'[p] = "attached" /\ pc'[p] = "idle")]_vars

\* Once the drain has finished the attachment is the upgrader's to give up, so a
\* conversion that is cancelled or fails leaves the handle detached.
ACancelledOrFailedConversionLeavesTheHandleDetached ==
    [][\A p \in Procs :
          (att[p] \in ConversionWaits /\ pc[p] = "converting"
           /\ areq'[p] \in {"withdrawn", "refused"}) =>
              (att'[p] = "off" /\ pc'[p] = "detached")]_vars

-----------------------------------------------------------------------------
(* Witnesses: each is violated by a reachable state or step it names.      *)

HoldsTold == \E p \in Procs : pc[p] = "told"

Witness_AllThreeUnservedOutcomes ==
    ~( (\E p \in Procs : req[p] = "refused")
       /\ (\E p \in Procs : req[p] = "withdrawn")
       /\ (\E p \in Procs : req[p] = "dead") )

Witness_ToldAfterDeathBeforeFirstEffect ==
    ~(diedIn \in {"held", "opening", "open"} /\ HoldsTold)

Witness_ToldAfterDeathWhileOpeningIsPublished ==
    ~(diedIn = "opening" /\ HoldsTold)

Witness_ToldAfterDeathMidWrite ==
    ~(diedIn = "writing" /\ Partial /\ HoldsTold)

Witness_ToldAfterDeathAfterLastEffect ==
    ~(diedIn \in {"wrote", "closing", "closed"} /\ HoldsTold)

Witness_ToldAfterDeathWhileClosingIsPublished ==
    ~(diedIn = "closing" /\ HoldsTold)

Witness_ToldAfterRestartAlone ==
    ~(diedIn = "none" /\ ~fence /\ ~trusted /\ HoldsTold)

Witness_OpeningPublicationFails ==
    [][~(\E p \in Procs : pc[p] = "opening" /\ pc'[p] = "closed")]_vars

Witness_ClosingPublicationFails ==
    [][~(\E p \in Procs : pc[p] = "closing" /\ pc'[p] = "told")]_vars

Witness_AnOperationFailsUnresolved ==
    [][~(\E p \in Procs : pc[p] = "writing" /\ pc'[p] = "told")]_vars

Witness_AnOperationIsRolledBack ==
    [][~(\E p \in Procs : pc[p] = "writing" /\ pc'[p] = "wrote" /\ a' < a)]_vars

Witness_AnOperationEndsWithoutAnEffect ==
    [][~(\E p \in Procs : pc[p] = "open" /\ pc'[p] = "wrote")]_vars

Witness_AnInnerOperationRuns ==
    [][~(\E p \in Procs : pc[p] = "nested" /\ pc'[p] = "writing")]_vars

Witness_ASettledReadIsAccepted ==
    [][~(\E p \in Procs : Accepted(p))]_vars

\* A snapshot taken between the two swaps of one transaction.
Witness_ATornReadIsRejected ==
    [][~(\E p \in Procs : Finishing(p) /\ rseenA[p] # rseenB[p])]_vars

\* A snapshot taken after a transaction failed part-way.
Witness_AReadAfterAnUnresolvedFailureIsRejected ==
    [][~(\E p \in Procs : Finishing(p) /\ Partial /\ fence /\ trusted)]_vars

Witness_AReadMeetsUntrustedState ==
    [][~(\E p \in Procs : Finishing(p) /\ ~trusted)]_vars

Witness_AReadSpansACompletedOperation ==
    [][~(\E p \in Procs :
            Finishing(p) /\ Even(rbefore[p]) /\ Even(FinalSample(p))
            /\ rbefore[p] # FinalSample(p))]_vars

Witness_ACacheIsKept ==
    [][~(\E p \in Procs :
            pc[p] = "locked" /\ pc'[p] \in {"held", "told"}
            /\ cache[p] # NoVal /\ mygen[p] = gen)]_vars

Witness_ACacheIsReloaded ==
    [][~(\E p \in Procs :
            pc[p] = "locked" /\ pc'[p] \in {"held", "told"}
            /\ cache[p] # NoVal /\ mygen[p] # gen)]_vars

\* Attachment.
Admitted(p) == att[p] \notin {"attached", "draining"} /\ att'[p] = "attached"
RefusedAtTheGate(p) ==
    pc[p] = "attaching" /\ areq'[p] = "refused"

Witness_AReaderIsAdmitted ==
    [][~(\E p \in Procs : ~CanWrite(p) /\ Admitted(p))]_vars

Witness_AWriterIsAdmitted ==
    [][~(\E p \in Procs : CanWrite(p) /\ Admitted(p))]_vars

Witness_AReaderIsRefused ==
    [][~(\E p \in Procs : ~CanWrite(p) /\ RefusedAtTheGate(p))]_vars

Witness_AWriterIsRefused ==
    [][~(\E p \in Procs : CanWrite(p) /\ RefusedAtTheGate(p))]_vars

Witness_AReaderIsAdmittedDuringASafeUpgrade ==
    [][~(\E p \in Procs : ~CanWrite(p) /\ Admitted(p) /\ upState = "safe")]_vars

Witness_AReaderSpeakingTheTargetIsAdmittedDuringASafeUpgrade ==
    [][~(\E p \in Procs :
            ~CanWrite(p) /\ Admitted(p) /\ upState = "safe"
            /\ SpeaksOf[p][UpLayer] = Target[UpLayer])]_vars

Witness_AReaderIsRefusedDuringAnUnsafeUpgrade ==
    [][~(\E p \in Procs :
            ~CanWrite(p) /\ RefusedAtTheGate(p) /\ upState = "unsafe")]_vars

\* An upgrader died in its body; a writer speaking the target resumes.
Witness_AWriterIsAdmittedToResumeAnInterruptedUpgrade ==
    [][~(\E p \in Procs :
            CanWrite(p) /\ Admitted(p) /\ upState # "ordinary"
            /\ \A q \in Procs : att[q] # "uBody")]_vars

Witness_ASafeUpgradeCompletes ==
    [][~(\E p \in Procs :
            att[p] = "uBody" /\ att'[p] = "off" /\ upState = "safe")]_vars

Witness_AnUnsafeUpgradeCompletes ==
    [][~(\E p \in Procs :
            att[p] = "uBody" /\ att'[p] = "off" /\ upState = "unsafe")]_vars

Witness_AnUpgraderGivesUpAtTheRecheck ==
    [][~(\E p \in Procs : att[p] = "uTiers" /\ att'[p] = "off")]_vars

Witness_AnAttachingHandleGivesUp ==
    [][~(\E p \in Procs : pc[p] = "attaching" /\ areq'[p] = "withdrawn")]_vars

Witness_AnUpgraderWaitsBehindAnAdmission ==
    ~(\E p, q \in Procs : att[p] = "uWGate" /\ HoldsGateS(q))

Witness_AWriterWaitsForTheWriterTierBehindAnUpgrade ==
    ~(\E p \in Procs : att[p] = "rd" /\ WriterXHeld)

Witness_AnUpgraderWaitsForAttachedWriters ==
    ~(\E p \in Procs : att[p] = "uGate" /\ WriterSHeld)

Witness_ReadersAttachWhileASafeUpgradeRuns ==
    ~(\E p, q \in Procs :
         att[p] = "uBody" /\ upState = "safe"
         /\ ~CanWrite(q) /\ att[q] = "attached")

\* Clones, leases and the drain.
Witness_ACloneOperationStarts ==
    [][~(\E p \in Procs : lease'[p] > lease[p])]_vars

Witness_AnUpgradeWaitsForACloneOperation ==
    ~(\E p \in Procs : att[p] = "draining" /\ lease[p] > 0)

Witness_AnOperationEndsWhileTheHandleDrains ==
    [][~(\E p \in Procs : att[p] = "draining" /\ lease'[p] < lease[p])]_vars

Witness_ADrainEndsAndTheConversionBegins ==
    [][~(\E p \in Procs : att[p] = "draining" /\ att'[p] = "uWGate")]_vars

Witness_ADrainIsCancelled ==
    [][~(\E p \in Procs : att[p] = "draining" /\ att'[p] = "attached")]_vars

Witness_ACancelledDrainLeavesOperationsRunning ==
    [][~(\E p \in Procs :
            att[p] = "draining" /\ att'[p] = "attached" /\ lease'[p] > 0)]_vars

Witness_AConversionIsCancelled ==
    [][~(\E p \in Procs :
            att[p] \in ConversionWaits /\ pc'[p] = "detached"
            /\ areq'[p] = "withdrawn")]_vars

Witness_AConversionFailsAtTheRecheck ==
    [][~(\E p \in Procs :
            att[p] = "uTiers" /\ pc'[p] = "detached" /\ areq'[p] = "refused")]_vars

Witness_ADetachedHandleAttachesAgain ==
    [][~(\E p \in Procs : pc[p] = "detached" /\ pc'[p] = "attaching")]_vars

Witness_ADetachedHandleIsDropped ==
    [][~(\E p \in Procs : pc[p] = "detached" /\ pc'[p] = "exited")]_vars

RefusedAtTheDoor(p) ==
    att[p] = "attached" /\ areq[p] # "refused" /\ areq'[p] = "refused"

Witness_AnUpgradeIsRefusedFromInsideARead ==
    [][~(\E p \in Procs : RefusedAtTheDoor(p) /\ rphase[p] # "none")]_vars

Witness_AnUpgradeIsRefusedFromInsideAWrite ==
    [][~(\E p \in Procs :
            RefusedAtTheDoor(p) /\ pc[p] \in InOperationStates)]_vars

Witness_AnUpgraderDiesWhileDraining ==
    [][~(\E p \in Procs : att[p] = "draining" /\ pc'[p] = "dead")]_vars

Witness_ADeathEndsTheLeases ==
    [][~(\E p \in Procs : pc'[p] = "dead" /\ lease[p] > 0)]_vars

\* The registry.
Witness_AHandleRegisters ==
    [][~(\E p \in Procs : rst[p] = "written" /\ rst'[p] = "in")]_vars

Witness_AHandleAttachesUnregistered ==
    [][~(\E p \in Procs : rst[p] = "scan" /\ rst'[p] = "unreg")]_vars

\* The entry of a handle that died is left as it was and is reused.
Witness_AnEntryOfADeadHandleIsReused ==
    [][~(\E p \in Procs :
            rst[p] = "claimed" /\ rst'[p] = "locked"
            /\ etok[rcur[p]] # None /\ pc[etok[rcur[p]]] = "dead")]_vars

Witness_ARegistrantMovesOnFromAHeldClaim ==
    [][~(\E p \in Procs :
            rst[p] = "scan" /\ rst'[p] = "scan" /\ rscan'[p] # rscan[p])]_vars

Witness_ARegistrantMovesOnFromALiveEntry ==
    [][~(\E p \in Procs : rst[p] = "claimed" /\ rst'[p] = "scan")]_vars

Witness_AnOwnerUpdatesItsEntry ==
    [][~(\E p \in Procs : rst[p] = "upd" /\ rst'[p] = "in")]_vars

Witness_AHandleUnregistersAtExit ==
    [][~(\E p \in Procs :
            rst[p] = "ucl" /\ rst'[p] = "out" /\ pc[p] = "detaching")]_vars

Witness_AHandleUnregistersAtConversion ==
    [][~(\E p \in Procs :
            rst[p] = "ucl" /\ rst'[p] = "out" /\ pc[p] = "converting")]_vars

Witness_AnUnregistrationSkipsAHeldClaim ==
    [][~(\E p \in Procs : rst[p] = "in" /\ rst'[p] = "out")]_vars

Witness_AListingSeesALiveEntry ==
    [][~(\E p \in Procs : lph[p] = "read" /\ lrep'[p][3] = "live")]_vars

Witness_AListingSeesADeadEntry ==
    [][~(\E p \in Procs : lph[p] = "read" /\ lrep'[p][3] = "dead")]_vars

Witness_AListingSeesAFreeEntry ==
    [][~(\E p \in Procs : lph[p] = "read" /\ lrep'[p][3] = "free")]_vars

Witness_ADeadHandlesEntryIsReportedDead ==
    [][~(\E p \in Procs :
            lph[p] = "read" /\ lrep'[p][3] = "dead"
            /\ ltok[p] # None /\ pc[ltok[p]] = "dead")]_vars

Witness_AListingMeetsAClaimedEntry ==
    [][~(\E p \in Procs : lph[p] = "next" /\ ~lunk[p] /\ lunk'[p])]_vars

Witness_AListingSaysItMayBeIncomplete ==
    [][~(\E p \in Procs :
            lph[p] = "next" /\ lph'[p] = "none" /\ lres'[p] = "incomplete")]_vars

Witness_AListingSaysItMayBeIncompleteAfterAnOverflow ==
    [][~(\E p \in Procs :
            lph[p] = "next" /\ lph'[p] = "none" /\ lres'[p] = "incomplete"
            /\ ~lunk[p])]_vars

Witness_AListingSaysItIsComplete ==
    [][~(\E p \in Procs :
            lph[p] = "next" /\ lph'[p] = "none" /\ lres'[p] = "complete")]_vars

Witness_ADeathBetweenTheClaimAndLivenessLocks ==
    [][~(\E p \in Procs : rst[p] = "claimed" /\ pc'[p] = "dead")]_vars

Witness_ADeathWhileRegistered ==
    [][~(\E p \in Procs : rst[p] = "in" /\ pc'[p] = "dead")]_vars
=============================================================================
