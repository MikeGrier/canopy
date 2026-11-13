\* Copyright (c) Michael Grier.
------------------------------ MODULE Creation ------------------------------
(***************************************************************************)
(* Creating a store root, as the coordination contract specifies it        *)
(* (crates/canopy/DESIGN-NOTES.md, "Several processes may open one store", *)
(* "Identity and creation"), across the death of a process and the loss of *)
(* power. It is a model of its own: creation ends at a marker that makes   *)
(* the root a store, and everything the rest of the contract does starts   *)
(* from it, so Coordination.tla begins with the marker in place.           *)
(*                                                                         *)
(* A process is a creator or an opener, uses one implementation of the     *)
(* coordination contract, and a creator carries creation inputs. Each      *)
(* implementation has a creation exclusion that excludes only the          *)
(* processes using that implementation, so two implementations cannot      *)
(* arbitrate between themselves and the root does it: a creator publishes  *)
(* a reservation naming its implementation and its inputs, with an         *)
(* exclusive publish-if-absent that leaves the complete file or none, and  *)
(* whoever publishes it owns the creation.                                 *)
(*                                                                         *)
(* A creator reads the root and decides. A marker means the root is a      *)
(* store and it stops. Something under the root that creation does not     *)
(* make means the root is not one to create in, and it stops. A            *)
(* reservation means a creation was begun: it stops if the reservation     *)
(* names another implementation or other inputs, and otherwise resumes it. *)
(* With no reservation it creates. Either way it then takes its own        *)
(* implementation's creation exclusion and reads the root again under it.  *)
(* A creator publishes the reservation; a resumer finds the marker still   *)
(* absent and the reservation unchanged; and each finds the root still     *)
(* unoccupied. It then makes the reservation durable and only then makes   *)
(* the artifacts, in order, making each durable before it makes the next.  *)
(* The last artifact is the marker. It releases the exclusion when the     *)
(* marker is durable.                                                      *)
(*                                                                         *)
(* Whether a file is durable is its own state, apart from whether it       *)
(* exists. A process's death changes no file and releases its exclusion.   *)
(* A host crash kills every process and keeps every durable file; each     *)
(* file that is not yet durable survives or is lost, independently.        *)
(* Making the reservation durable stands for making the file and its       *)
(* directory entry durable.                                                *)
(*                                                                         *)
(* An opener takes its implementation's creation exclusion, which a        *)
(* creator holds until its marker is in place, reads the root, and opens   *)
(* it if there is a marker and the reservation names the opener's          *)
(* implementation. It makes the marker durable before it relies on it,     *)
(* since the creator may have died between publishing the marker and       *)
(* making it durable.                                                      *)
(*                                                                         *)
(* Steps are sequentially consistent, and reading the root is one step.    *)
(*                                                                         *)
(* Control selects a fault to confirm the properties detect it:            *)
(*   "none"                          the contract.                         *)
(*   "reservation-in-two-writes"     the reservation is published as an    *)
(*                                   empty file and then filled in.        *)
(*   "reservation-not-synced"        the artifacts begin before the        *)
(*                                   reservation is durable.               *)
(*   "body-not-synced-before-marker" an artifact is not made durable       *)
(*                                   before the next is made.              *)
(*   "publish-replaces"              publishing replaces a reservation     *)
(*                                   that is there.                        *)
(*   "publish-without-exclusion"     a creator publishes without holding   *)
(*                                   its creation exclusion.               *)
(*   "resume-without-exclusion"      a resumer does not take the creation  *)
(*                                   exclusion.                            *)
(*   "resume-without-reread"         a resumer does not read the root      *)
(*                                   again under the exclusion.            *)
(*   "resume-ignores-inputs"         a resumer does not compare its inputs *)
(*                                   with the reservation's.               *)
(*   "open-ignores-implementation"   an open succeeds through any          *)
(*                                   implementation.                       *)
(*   "open-trusts-a-live-marker"     an open does not make the marker      *)
(*                                   durable before it relies on it.       *)
(*   "creation-ignores-foreign-content"                                    *)
(*                                   a creator does not refuse a root that *)
(*                                   holds something creation does not     *)
(*                                   make.                                 *)
(*                                                                         *)
(* The Witness_ properties are false on purpose: a run that checks one     *)
(* must report it violated, which shows the state or step it names is      *)
(* reachable and so that the properties are not holding for want of        *)
(* anything to check.                                                      *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences, TLC

CONSTANTS
    Procs,       \* the processes that create or open the root, 1..Len(RoleOf)
    NArt,        \* how many artifacts creation makes; the last is the marker
    MaxDeaths,   \* bound on process deaths
    MaxCrashes,  \* bound on host crashes
    Intrusion,   \* whether something creation does not make may appear under
                 \* the root while a creation is under way
    Scenario,    \* which of the Scenarios the processes are
    Control      \* the fault under test

None == 0   \* a number, since TLC compares only values of one kind

\* A scenario fixes what each process is, a creator or an opener, the
\* implementation it uses and the creation inputs it carries, which an opener
\* ignores. A configuration names one because a configuration file cannot
\* state a sequence.
Scen(roles, impls, ins) == [roles |-> roles, impls |-> impls, ins |-> ins]

Scenarios ==
       "OneCreator" :> Scen(<<"creator">>, <<1>>, <<1>>)
    @@ "TwoCreators" :> Scen(<<"creator", "creator">>, <<1, 1>>, <<1, 1>>)
    @@ "TwoImplementations" :> Scen(<<"creator", "creator">>, <<1, 2>>, <<1, 1>>)
    @@ "DifferentInputs" :> Scen(<<"creator", "creator">>, <<1, 1>>, <<1, 2>>)
    @@ "ThreeCreators" :>
          Scen(<<"creator", "creator", "creator">>, <<1, 1, 2>>, <<1, 1, 1>>)
    @@ "ThreeDifferent" :>
          Scen(<<"creator", "creator", "creator">>, <<1, 1, 2>>, <<1, 2, 1>>)
    @@ "FourCreators" :>
          Scen(<<"creator", "creator", "creator", "creator">>,
               <<1, 1, 1, 2>>, <<1, 1, 2, 1>>)
    @@ "CreatorAndOpeners" :>
          Scen(<<"creator", "opener", "opener">>, <<1, 1, 2>>, <<1, 1, 1>>)
    @@ "TwoCreatorsAndOpener" :>
          Scen(<<"creator", "creator", "opener">>, <<1, 1, 1>>, <<1, 1, 1>>)

RoleOf == Scenarios[Scenario].roles
ImplOf == Scenarios[Scenario].impls
InOf == Scenarios[Scenario].ins

Impls == {1, 2}
Inputs == {1, 2}
Arts == 1..NArt
Marker == NArt

ASSUME Scenario \in DOMAIN Scenarios
ASSUME Procs # {} /\ Procs = 1..Len(RoleOf)
ASSUME NArt \in Nat /\ NArt >= 1 /\ MaxDeaths \in Nat /\ MaxCrashes \in Nat
ASSUME Intrusion \in BOOLEAN
ASSUME \A p \in Procs : RoleOf[p] \in {"creator", "opener"}
ASSUME \A p \in Procs : ImplOf[p] \in Impls /\ InOf[p] \in Inputs
ASSUME Control \in {"none", "reservation-in-two-writes", "reservation-not-synced",
                    "body-not-synced-before-marker", "publish-replaces",
                    "publish-without-exclusion", "resume-without-exclusion",
                    "resume-without-reread", "resume-ignores-inputs",
                    "open-ignores-implementation", "open-trusts-a-live-marker",
                    "creation-ignores-foreign-content"}

VARIABLES
    pc,         \* [Procs -> process states]
    out,        \* [Procs -> how its attempt ended]
    cx,         \* [Impls -> Procs or None]: who holds each creation exclusion
    res,        \* the reservation as the root has it
    rdur,       \* whether the reservation is durable
    aliv,       \* [Arts -> BOOLEAN]: which artifacts exist
    ainp,       \* [Arts -> inputs or None]: the inputs each was made from
    adur,       \* [Arts -> BOOLEAN]: which artifacts are durable
    foreign,    \* something creation does not make is under the root
    seen,       \* [Procs -> reservation]: what a resumer read before it waited
    path,       \* [Procs -> "none", "fresh" or "resume"]: what a creator decided
    ak,         \* [Procs -> Arts or None]: the artifact a creator is making
    sawPartial, \* ghost: some process read a reservation that was not complete
    relied,     \* ghost: some open succeeded
    everPub,    \* ghost: a reservation has been published
    deaths,
    crashes

vars == <<pc, out, cx, res, rdur, aliv, ainp, adur, foreign, seen, path, ak,
          sawPartial, relied, everPub, deaths, crashes>>

root   == <<res, rdur, aliv, ainp, adur, foreign>>
mine   == <<seen, path, ak>>
ghost  == <<sawPartial, relied, everPub>>
counts == <<deaths, crashes>>

\* Process states. "new" has not started; a creator is "wantX" once it has
\* decided and waits for its creation exclusion, "holdsX" while it holds the
\* exclusion and has not yet published or confirmed, "partialPub" between the
\* two writes of a reservation (only under a control), "syncRes" while it makes
\* the reservation durable, then "writeArt" and "syncArt" for each artifact.
\* "done" has ended with an outcome.
CreatingStates == {"syncRes", "writeArt", "syncArt"}
HoldingStates == {"holdsX", "partialPub"} \cup CreatingStates
LiveStates == {"wantX"} \cup HoldingStates
States == {"new", "done", "dead"} \cup LiveStates
Outcomes == {"none", "created", "resumed", "exists", "occupied", "otherImpl",
             "differentInputs", "lostRace", "changed", "partial", "opened",
             "notAStore", "mismatch"}

NoRes == [st |-> "none", impl |-> 0, inp |-> 0, nonce |-> 0]
Half == [st |-> "partial", impl |-> 0, inp |-> 0, nonce |-> 0]
Whole(p) == [st |-> "whole", impl |-> ImplOf[p], inp |-> InOf[p], nonce |-> p]
Reservations == {NoRes, Half}
                \cup [st : {"whole"}, impl : Impls, inp : Inputs, nonce : Procs]

Set(f, k, v) == [f EXCEPT ![k] = v]
MarkerLive == aliv[Marker]
\* Whether a creator finds the root free of anything creation does not make.
Unoccupied == ~foreign \/ Control = "creation-ignores-foreign-content"
ReleaseX(p) == [i \in Impls |-> IF cx[i] = p THEN None ELSE cx[i]]

-----------------------------------------------------------------------------
\* Audience: internal.
TypeOK ==
    /\ pc \in [Procs -> States]
    /\ out \in [Procs -> Outcomes]
    /\ cx \in [Impls -> Procs \cup {None}]
    /\ res \in Reservations
    /\ rdur \in BOOLEAN
    /\ aliv \in [Arts -> BOOLEAN]
    /\ ainp \in [Arts -> Inputs \cup {None}]
    /\ adur \in [Arts -> BOOLEAN]
    /\ foreign \in BOOLEAN
    /\ seen \in [Procs -> Reservations]
    /\ path \in [Procs -> {"none", "fresh", "resume"}]
    /\ ak \in [Procs -> 0..NArt]
    /\ sawPartial \in BOOLEAN
    /\ relied \in BOOLEAN
    /\ everPub \in BOOLEAN
    /\ deaths \in 0..MaxDeaths
    /\ crashes \in 0..MaxCrashes

Init ==
    /\ pc = [p \in Procs |-> "new"]
    /\ out = [p \in Procs |-> "none"]
    /\ cx = [i \in Impls |-> None]
    /\ res = NoRes
    /\ rdur = FALSE
    /\ aliv = [k \in Arts |-> FALSE]
    /\ ainp = [k \in Arts |-> None]
    /\ adur = [k \in Arts |-> FALSE]
    /\ foreign = FALSE
    /\ seen = [p \in Procs |-> NoRes]
    /\ path = [p \in Procs |-> "none"]
    /\ ak = [p \in Procs |-> 0]
    /\ sawPartial = FALSE
    /\ relied = FALSE
    /\ everPub = FALSE
    /\ deaths = 0
    /\ crashes = 0

-----------------------------------------------------------------------------
(* A creator.                                                              *)

\* The attempt ends with an outcome.
EndAs(p, why) ==
    /\ pc' = Set(pc, p, "done")
    /\ out' = Set(out, p, why)

\* A creator reads the root, which is one step, and decides.
Look(p) ==
    /\ pc[p] = "new" /\ RoleOf[p] = "creator"
    /\ \/ /\ res.st = "partial"
          /\ EndAs(p, "partial")
          /\ sawPartial' = TRUE
          /\ UNCHANGED <<seen, path>>
       \/ /\ res.st # "partial" /\ MarkerLive
          /\ EndAs(p, "exists")
          /\ UNCHANGED <<sawPartial, seen, path>>
       \/ /\ res.st # "partial" /\ ~MarkerLive /\ ~Unoccupied
          /\ EndAs(p, "occupied")
          /\ UNCHANGED <<sawPartial, seen, path>>
       \/ /\ res.st = "whole" /\ ~MarkerLive /\ Unoccupied
          /\ res.impl # ImplOf[p]
          /\ EndAs(p, "otherImpl")
          /\ UNCHANGED <<sawPartial, seen, path>>
       \/ /\ res.st = "whole" /\ ~MarkerLive /\ Unoccupied
          /\ res.impl = ImplOf[p]
          /\ res.inp # InOf[p] /\ Control # "resume-ignores-inputs"
          /\ EndAs(p, "differentInputs")
          /\ UNCHANGED <<sawPartial, seen, path>>
       \/ /\ res.st = "whole" /\ ~MarkerLive /\ Unoccupied
          /\ res.impl = ImplOf[p]
          /\ (res.inp = InOf[p] \/ Control = "resume-ignores-inputs")
          /\ pc' = Set(pc, p, IF Control = "resume-without-exclusion"
                                 THEN "holdsX" ELSE "wantX")
          /\ seen' = Set(seen, p, res)
          /\ path' = Set(path, p, "resume")
          /\ UNCHANGED <<out, sawPartial>>
       \/ /\ res.st = "none" /\ ~MarkerLive /\ Unoccupied
          /\ pc' = Set(pc, p, IF Control = "publish-without-exclusion"
                                 THEN "holdsX" ELSE "wantX")
          /\ path' = Set(path, p, "fresh")
          /\ UNCHANGED <<out, sawPartial, seen>>
    /\ UNCHANGED <<cx, root, ak, relied, everPub, counts>>

\* The creation exclusion of the creator's own implementation, taken whenever
\* it is free, so every service order is checked.
Lock(p) ==
    /\ pc[p] = "wantX" /\ cx[ImplOf[p]] = None
    /\ cx' = [cx EXCEPT ![ImplOf[p]] = p]
    /\ pc' = Set(pc, p, "holdsX")
    /\ UNCHANGED <<out, root, mine, ghost, counts>>

\* Publish-if-absent publishes the complete reservation, or in two writes under
\* a control, and replaces one that is there under another.
CanPublish == res.st = "none" \/ Control = "publish-replaces"

Publish(p) ==
    /\ pc[p] = "holdsX" /\ path[p] = "fresh" /\ CanPublish /\ Unoccupied
    /\ IF Control = "reservation-in-two-writes"
          THEN /\ res' = Half
               /\ pc' = Set(pc, p, "partialPub")
          ELSE /\ res' = Whole(p)
               /\ pc' = Set(pc, p, "syncRes")
    /\ rdur' = FALSE
    /\ everPub' = TRUE
    /\ UNCHANGED <<cx, out, aliv, ainp, adur, foreign, mine, sawPartial, relied,
                   counts>>

\* Only under Control = "reservation-in-two-writes".
SecondWrite(p) ==
    /\ pc[p] = "partialPub"
    /\ res' = Whole(p)
    /\ pc' = Set(pc, p, "syncRes")
    /\ UNCHANGED <<cx, out, rdur, aliv, ainp, adur, foreign, mine, ghost, counts>>

\* A reservation was there: the creator stops.
LoseRace(p) ==
    /\ pc[p] = "holdsX" /\ path[p] = "fresh" /\ ~CanPublish
    /\ EndAs(p, "lostRace")
    /\ cx' = ReleaseX(p)
    /\ UNCHANGED <<root, mine, ghost, counts>>

\* The root was occupied when the creator read it again under the exclusion.
OccupiedUnderExclusion(p) ==
    /\ pc[p] = "holdsX" /\ path[p] = "fresh" /\ CanPublish /\ ~Unoccupied
    /\ EndAs(p, "occupied")
    /\ cx' = ReleaseX(p)
    /\ UNCHANGED <<root, mine, ghost, counts>>

\* A resumer reads the root again under the exclusion. The creation it read is
\* gone if the marker is in place, is not the one it read if the reservation
\* changed, and cannot be resumed if the root holds something else.
Reconfirm(p) ==
    /\ pc[p] = "holdsX" /\ path[p] = "resume"
    /\ \/ /\ MarkerLive /\ Control # "resume-without-reread"
          /\ EndAs(p, "exists")
          /\ cx' = ReleaseX(p)
       \/ /\ ~MarkerLive /\ res # seen[p] /\ Control # "resume-without-reread"
          /\ EndAs(p, "changed")
          /\ cx' = ReleaseX(p)
       \/ /\ ~MarkerLive /\ res = seen[p] /\ ~Unoccupied
          /\ Control # "resume-without-reread"
          /\ EndAs(p, "occupied")
          /\ cx' = ReleaseX(p)
       \/ /\ (Control = "resume-without-reread"
               \/ (~MarkerLive /\ res = seen[p] /\ Unoccupied))
          /\ pc' = Set(pc, p, "syncRes")
          /\ UNCHANGED <<out, cx>>
    /\ UNCHANGED <<root, mine, ghost, counts>>

\* The reservation, and with it its directory entry, is made durable before
\* any other artifact exists.
SyncRes(p) ==
    /\ pc[p] = "syncRes"
    /\ rdur' = IF Control = "reservation-not-synced" THEN rdur ELSE TRUE
    /\ pc' = Set(pc, p, "writeArt")
    /\ ak' = Set(ak, p, 1)
    /\ UNCHANGED <<cx, out, res, aliv, ainp, adur, foreign, seen, path, ghost,
                   counts>>

\* An artifact that is already there is the one a dead creator made, from the
\* same inputs, and is kept.
WriteArt(p) ==
    /\ pc[p] = "writeArt"
    /\ aliv' = Set(aliv, ak[p], TRUE)
    /\ ainp' = IF aliv[ak[p]] THEN ainp ELSE Set(ainp, ak[p], InOf[p])
    /\ pc' = Set(pc, p, "syncArt")
    /\ UNCHANGED <<cx, out, res, rdur, adur, foreign, mine, ghost, counts>>

\* Each artifact is made durable before the next is made, and the creator lets
\* go of its exclusion when the marker is durable.
SyncArt(p) ==
    /\ pc[p] = "syncArt"
    /\ adur' = IF Control = "body-not-synced-before-marker" /\ ak[p] < Marker
                  THEN adur ELSE Set(adur, ak[p], TRUE)
    /\ IF ak[p] < Marker
          THEN /\ pc' = Set(pc, p, "writeArt")
               /\ ak' = Set(ak, p, ak[p] + 1)
               /\ UNCHANGED <<cx, out>>
          ELSE /\ pc' = Set(pc, p, "done")
               /\ ak' = Set(ak, p, 0)
               /\ cx' = ReleaseX(p)
               /\ out' = Set(out, p, IF path[p] = "resume" THEN "resumed"
                                     ELSE "created")
    /\ UNCHANGED <<res, rdur, aliv, ainp, foreign, seen, path, ghost, counts>>

-----------------------------------------------------------------------------
(* An opener.                                                              *)

\* An opener reads the root while no creator of its implementation holds the
\* exclusion, and opens it if there is a marker and the reservation names its
\* implementation.
OpenLook(p) ==
    /\ pc[p] = "new" /\ RoleOf[p] = "opener" /\ cx[ImplOf[p]] = None
    /\ \/ /\ res.st = "partial"
          /\ EndAs(p, "partial")
          /\ sawPartial' = TRUE
          /\ UNCHANGED <<relied, adur>>
       \/ /\ res.st # "partial" /\ ~MarkerLive
          /\ EndAs(p, "notAStore")
          /\ UNCHANGED <<sawPartial, relied, adur>>
       \/ /\ res.st # "partial" /\ MarkerLive
          /\ res.impl # ImplOf[p] /\ Control # "open-ignores-implementation"
          /\ EndAs(p, "mismatch")
          /\ UNCHANGED <<sawPartial, relied, adur>>
       \/ /\ res.st # "partial" /\ MarkerLive
          /\ (res.impl = ImplOf[p] \/ Control = "open-ignores-implementation")
          /\ EndAs(p, "opened")
          /\ relied' = TRUE
          /\ adur' = IF Control = "open-trusts-a-live-marker" THEN adur
                     ELSE Set(adur, Marker, TRUE)
          /\ UNCHANGED sawPartial
    /\ UNCHANGED <<cx, res, rdur, aliv, ainp, foreign, mine, everPub, counts>>

-----------------------------------------------------------------------------
(* Death and crash.                                                        *)

\* EXTERNAL: one process dies at any step. The OS releases its exclusion;
\* nothing it left in the root changes.
Die(p) ==
    /\ deaths < MaxDeaths
    /\ pc[p] \in LiveStates
    /\ pc' = Set(pc, p, "dead")
    /\ cx' = ReleaseX(p)
    /\ deaths' = deaths + 1
    /\ seen' = Set(seen, p, NoRes)
    /\ path' = Set(path, p, "none")
    /\ ak' = Set(ak, p, 0)
    /\ UNCHANGED <<out, root, ghost, crashes>>

\* EXTERNAL: power loss. Every process dies and every exclusion is released.
\* Whatever is durable survives; a reservation or artifact that is not durable
\* survives or is lost, each on its own, and what survives is durable.
Crash ==
    /\ crashes < MaxCrashes
    /\ pc' = [p \in Procs |-> IF pc[p] \in LiveStates THEN "dead" ELSE pc[p]]
    /\ cx' = [i \in Impls |-> None]
    /\ res' \in IF rdur \/ res.st = "none" THEN {res} ELSE {res, NoRes}
    /\ rdur' = (res'.st # "none")
    /\ aliv' \in {f \in [Arts -> BOOLEAN] :
                     \A k \in Arts : (f[k] => aliv[k]) /\ (adur[k] => f[k])}
    /\ ainp' = [k \in Arts |-> IF aliv'[k] THEN ainp[k] ELSE None]
    /\ adur' = aliv'
    /\ seen' = [p \in Procs |-> NoRes]
    /\ path' = [p \in Procs |-> "none"]
    /\ ak' = [p \in Procs |-> 0]
    /\ crashes' = crashes + 1
    /\ UNCHANGED <<out, foreign, ghost, deaths>>

\* EXTERNAL: something creation does not make appears under the root, and
\* stays.
Intrude ==
    /\ Intrusion /\ ~foreign
    /\ foreign' = TRUE
    /\ UNCHANGED <<pc, out, cx, res, rdur, aliv, ainp, adur, mine, ghost, counts>>

\* Every process has ended. It is the only step then, so a state in which a
\* process is stuck waiting is the only kind of deadlock.
Terminal ==
    /\ \A p \in Procs : pc[p] \in {"done", "dead"}
    /\ UNCHANGED vars

Next ==
    \/ \E p \in Procs :
          \/ Look(p) \/ Lock(p) \/ Publish(p) \/ SecondWrite(p) \/ LoseRace(p)
          \/ Reconfirm(p) \/ OccupiedUnderExclusion(p) \/ SyncRes(p)
          \/ WriteArt(p) \/ SyncArt(p) \/ OpenLook(p) \/ Die(p)
    \/ Crash
    \/ Intrude
    \/ Terminal

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties.                                                             *)

\* Audience: internal.
\* No process reads a reservation that is not complete.
NeverObservedPartial == ~sawPartial

\* Audience: internal.
\* No other artifact exists, so none can be durable, unless the reservation is
\* complete and durable.
ReservationPrecedesEveryOtherArtifact ==
    (\E k \in Arts : aliv[k]) => (res.st = "whole" /\ rdur)

\* Audience: client. A root is a store only when every artifact is there and durable.
\* Shows through: the outcome of open and of create.
\* The marker exists only once every artifact before it is there and durable.
MarkerCompletesTheStore ==
    MarkerLive => \A k \in Arts \ {Marker} : aliv[k] /\ adur[k]

\* Audience: client. A created store is made from the inputs it was asked to be made from.
\* Shows through: the contents of a created store.
\* Every artifact was made from the inputs the reservation records.
ArtifactsCarryTheReservedInputs ==
    \A k \in Arts : aliv[k] => ainp[k] = res.inp

\* Audience: client. At most one creation proceeds in a root, whichever implementations are in
\* use. Shows through: the outcome of two creates that race.
\* At most one process is creating, whichever implementations the processes use.
OneCreationAtATime ==
    Cardinality({p \in Procs : pc[p] \in CreatingStates}) <= 1

\* Audience: internal.
\* A process holds its implementation's creation exclusion exactly while it
\* is publishing or creating.
ExclusionMatchesHolders ==
    /\ \A p \in Procs : pc[p] \in HoldingStates => cx[ImplOf[p]] = p
    /\ \A i \in Impls :
          cx[i] # None => (pc[cx[i]] \in HoldingStates /\ ImplOf[cx[i]] = i)

\* Audience: client. A creation continues only under the implementation and inputs its reservation
\* records. Shows through: the outcome of a create that finds another reservation.
\* A process that is creating does so for the creation the reservation records:
\* its implementation and its inputs.
ACreatorUsesTheReservedInputs ==
    \A p \in Procs :
        pc[p] \in CreatingStates =>
            (res.st = "whole" /\ res.impl = ImplOf[p] /\ res.inp = InOf[p])

\* Audience: client. Nothing is still creating once the store exists. Shows through: a create that
\* runs over a finished store.
\* Once the marker is there, the only process still creating is the one making
\* the marker.
NoCreationAfterTheMarker ==
    MarkerLive =>
        \A p \in Procs : pc[p] \in CreatingStates => ak[p] = Marker

\* Audience: client. A store that has been opened is still a store after power is lost.
\* Shows through: open after a crash.
\* A store that has been opened keeps its marker, however power is lost.
AnOpenedStoreKeepsItsMarker == relied => (MarkerLive /\ adur[Marker])

\* Audience: internal.
\* A request or attempt is over exactly when it has an outcome.
OutcomeMatchesProcess ==
    \A p \in Procs : (pc[p] = "done") <=> (out[p] # "none")

\* Audience: internal.
\* The reservation is never replaced once it is published. The only change to
\* one that is there is the second write of one published in two.
ReservationIsPublishedOnlyIfAbsent ==
    [][(res.st # "none" /\ res'.st # "none" /\ res' # res)
          => (res.st = "partial" /\ res'.st = "whole")]_vars

\* Audience: internal.
\* A reservation is published only by a creator that holds the creation
\* exclusion of its implementation, and names that creator.
ThePublisherHoldsItsExclusion ==
    [][(res'.st # "none" /\ res' # res) =>
          \E p \in Procs :
              /\ pc[p] \in {"holdsX", "partialPub"}
              /\ cx[ImplOf[p]] = p
              /\ res'.nonce = p]_vars

\* Audience: client. An open succeeds only through the implementation the root records, and only
\* where the store exists. Shows through: the outcome of open.
\* An open succeeds only through the implementation the reservation names, and
\* only where there is a marker.
AnOpenSucceedsOnlyThroughTheRecordedImplementation ==
    [][\A p \in Procs :
          (out[p] # "opened" /\ out'[p] = "opened")
             => (MarkerLive /\ res.impl = ImplOf[p])]_vars

\* Audience: client. A create or open that is refused changes nothing in the root. Shows through:
\* the outcome of create and of open.
\* An attempt that ends without creating or opening changes nothing in the
\* root.
ARefusalChangesNothing ==
    [][\A p \in Procs :
          (pc[p] # "done" /\ pc'[p] = "done"
           /\ out'[p] \notin {"created", "resumed", "opened"})
             => UNCHANGED root]_vars

\* Audience: client. A process dying changes nothing in the root, so its creation can be resumed.
\* Shows through: the outcome of a create that resumes.
\* A process's death changes nothing in the root.
DeathLeavesTheRootAsItWas ==
    [][deaths' # deaths => UNCHANGED root]_vars

\* Audience: client. A creation never begins in a root that holds something creation does not
\* make. Shows through: the outcome of create.
\* A creator publishes its reservation, or a resumer goes on to create, only
\* when the root holds nothing creation does not make.
CreationBeginsOnlyInAnUnoccupiedRoot ==
    [][\A p \in Procs :
          (pc[p] = "holdsX" /\ pc'[p] \in {"partialPub", "syncRes"})
             => ~foreign]_vars

\* Audience: internal.
\* A host crash leaves no live process, keeps everything durable, and leaves
\* everything that survives durable.
ACrashKeepsEverythingDurable ==
    [][crashes' # crashes =>
          /\ \A p \in Procs : pc'[p] \notin LiveStates
          /\ (rdur => res' = res)
          /\ \A k \in Arts : adur[k] => aliv'[k]
          /\ (res'.st # "none" => rdur')
          /\ \A k \in Arts : aliv'[k] => adur'[k]]_vars

-----------------------------------------------------------------------------
(* Witnesses: each is violated by a reachable state or step it names.      *)

Witness_ACreationCompletes ==
    ~(\E p \in Procs : out[p] = "created")

Witness_AnInterruptedCreationIsResumed ==
    ~(\E p \in Procs : out[p] = "resumed")

Witness_AResumeFollowsAProcessDeath ==
    ~((\E p \in Procs : out[p] = "resumed") /\ deaths > 0 /\ crashes = 0)

Witness_AResumeFollowsAHostCrash ==
    ~((\E p \in Procs : out[p] = "resumed") /\ crashes > 0)

Witness_ACrashLosesTheReservation ==
    ~(everPub /\ res.st = "none")

Witness_ACrashLeavesOnlyTheReservation ==
    ~(crashes > 0 /\ res.st = "whole" /\ \A k \in Arts : ~aliv[k])

Witness_ACrashLeavesTheArtifactsWithoutTheMarker ==
    ~(crashes > 0 /\ NArt > 1 /\ ~MarkerLive
      /\ \A k \in Arts \ {Marker} : aliv[k])

Witness_ACreatorWaitsForTheExclusion ==
    ~(\E p \in Procs : pc[p] = "wantX" /\ cx[ImplOf[p]] # None)

Witness_ACreatorStopsAtAnotherImplementationsReservation ==
    ~(\E p \in Procs : out[p] = "otherImpl")

Witness_ACreatorStopsAtDifferentInputs ==
    ~(\E p \in Procs : out[p] = "differentInputs")

Witness_ACreatorLosesTheRace ==
    ~(\E p \in Procs : out[p] = "lostRace")

Witness_ACreatorStopsAtAnOccupiedRoot ==
    ~(\E p \in Procs : out[p] = "occupied")

Witness_ACreatorFindsTheRootOccupiedUnderTheExclusion ==
    [][~(\E p \in Procs : pc[p] = "holdsX" /\ path[p] = "fresh"
                          /\ out'[p] = "occupied")]_vars

Witness_AResumerFindsTheRootOccupiedUnderTheExclusion ==
    [][~(\E p \in Procs : pc[p] = "holdsX" /\ path[p] = "resume"
                          /\ out'[p] = "occupied")]_vars

Witness_ACreatorFindsTheStoreCreated ==
    [][~(\E p \in Procs : pc[p] = "new" /\ out'[p] = "exists")]_vars

Witness_AResumerFindsTheStoreCreatedWhileItWaited ==
    [][~(\E p \in Procs : pc[p] = "holdsX" /\ out'[p] = "exists")]_vars

Witness_AnOpenOpens ==
    ~(\E p \in Procs : out[p] = "opened")

Witness_AnOpenIsRefusedForAnotherImplementation ==
    ~(\E p \in Procs : out[p] = "mismatch")

Witness_AnOpenFindsNoStore ==
    ~(\E p \in Procs : out[p] = "notAStore")

Witness_AnOpenFindsACreationInProgress ==
    ~(\E p \in Procs : out[p] = "notAStore" /\ res.st = "whole")

\* An opener meets a marker its creator died before making durable.
Witness_AnOpenMakesTheMarkerDurable ==
    [][~(\E p \in Procs : out[p] # "opened" /\ out'[p] = "opened"
                          /\ ~adur[Marker])]_vars

=============================================================================
