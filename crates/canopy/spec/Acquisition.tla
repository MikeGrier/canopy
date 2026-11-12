\* Copyright (c) Michael Grier.
---------------------------- MODULE Acquisition ----------------------------
(***************************************************************************)
(* The acquisition layer of the coordination contract, on its own: writer  *)
(* exclusion, the admission gate and the two attachment tiers, taken and   *)
(* released by processes that go on attaching, writing, upgrading and      *)
(* detaching forever. What a process does while it holds something is      *)
(* abstract -- it holds, then releases -- so the model is small, and the   *)
(* results about who is served are established here once, under the        *)
(* assumption that every hold ends (DESIGN-NOTES "The acquisition layer is *)
(* a model of its own"). A model that takes and releases with these steps  *)
(* inherits them, and owes this layer only that assumption.                *)
(***************************************************************************)
EXTENDS Naturals, Sequences, TLC

CONSTANTS
    Scenario,        \* names the role of each process, from Scenarios
    MaxUpgrades,     \* how many upgrades may start, in all
    ExclusiveFirst   \* no handle passes the gate shared while an upgrader waits for it

\* A configuration cannot state a sequence, so it names one. An upgrader unsafe for
\* readers takes the reader tier exclusively; a safe one leaves readers attached.
Scenarios ==
       "Writers2" :> <<"writer", "writer">>
    @@ "Writers3" :> <<"writer", "writer", "writer">>
    @@ "UnsafeAndReader" :> <<"unsafeUpgrader", "reader">>
    @@ "SafeAndReader" :> <<"safeUpgrader", "reader">>
    @@ "SafeAndWriter" :> <<"safeUpgrader", "writer">>
    @@ "UnsafeAndTwoReaders" :> <<"unsafeUpgrader", "reader", "reader">>
    @@ "UnsafeWriterReader" :> <<"unsafeUpgrader", "writer", "reader">>

ASSUME Scenario \in DOMAIN Scenarios /\ MaxUpgrades \in Nat /\ ExclusiveFirst \in BOOLEAN

RoleOf == Scenarios[Scenario]
Procs == 1..Len(RoleOf)
None == 0

CanWrite(p) == RoleOf[p] # "reader"
Upgrader(p) == RoleOf[p] \in {"safeUpgrader", "unsafeUpgrader"}
TakesReaderTier(p) == RoleOf[p] = "unsafeUpgrader"

VARIABLES
    att,         \* where each process is in attaching, and what it holds by it
    pc,          \* where each process is in writing
    lockHolder,  \* the holder of writer exclusion, or None
    upgrades     \* how many upgrades have started

vars == <<att, pc, lockHolder, upgrades>>

Set(f, k, v) == [f EXCEPT ![k] = v]

AttStates == {"off", "wGate", "hasGate", "gateRd", "rd", "tiers", "attached", "draining",
              "uWGate", "uGate", "uRd", "uTiers", "uBody"}
PcStates == {"new", "attaching", "idle", "waiting", "held", "converting", "exited"}

\* The gate and the tiers each process holds, fixed by where it is.
HoldsGateS(p) == att[p] \in {"hasGate", "gateRd"}
HoldsGateX(p) ==
    \/ att[p] \in {"uGate", "uRd", "uTiers"}
    \/ att[p] = "uBody" /\ TakesReaderTier(p)
HoldsReaderS(p) == att[p] \in {"gateRd", "rd", "tiers", "attached", "draining"}
HoldsWriterS(p) == CanWrite(p) /\ att[p] \in {"tiers", "attached", "draining"}
HoldsReaderX(p) ==
    \/ att[p] = "uRd"
    \/ att[p] \in {"uTiers", "uBody"} /\ TakesReaderTier(p)
HoldsWriterX(p) == att[p] \in {"uTiers", "uBody"}

GateSHeld == \E q \in Procs : HoldsGateS(q)
GateXHeld == \E q \in Procs : HoldsGateX(q)
ReaderSHeld == \E q \in Procs : HoldsReaderS(q)
ReaderXHeld == \E q \in Procs : HoldsReaderX(q)
WriterSHeld == \E q \in Procs : HoldsWriterS(q)
WriterXHeld == \E q \in Procs : HoldsWriterX(q)

\* An upgrader waits for the gate exclusively.
ExclusiveWaiter == \E q \in Procs : att[q] = "uWGate"

Init ==
    /\ att = [p \in Procs |-> "off"]
    /\ pc = [p \in Procs |-> "new"]
    /\ lockHolder = None
    /\ upgrades = 0

-----------------------------------------------------------------------------
(* Attaching. A handle passes the gate shared, takes the reader tier,      *)
(* drops the gate, takes the writer tier if it may write, and is admitted. *)
(* One that has detached may attach again.                                 *)

BeginAttach(p) ==
    /\ pc[p] \in {"new", "exited"}
    /\ pc' = Set(pc, p, "attaching")
    /\ att' = Set(att, p, "wGate")
    /\ UNCHANGED <<lockHolder, upgrades>>

TakeGateS(p) ==
    /\ att[p] = "wGate" /\ ~GateXHeld
    /\ ~(ExclusiveFirst /\ ExclusiveWaiter)
    /\ att' = Set(att, p, "hasGate")
    /\ UNCHANGED <<pc, lockHolder, upgrades>>

TakeReaderS(p) ==
    /\ att[p] = "hasGate" /\ ~ReaderXHeld
    /\ att' = Set(att, p, "gateRd")
    /\ UNCHANGED <<pc, lockHolder, upgrades>>

DropGate(p) ==
    /\ att[p] = "gateRd"
    /\ att' = Set(att, p, IF CanWrite(p) THEN "rd" ELSE "tiers")
    /\ UNCHANGED <<pc, lockHolder, upgrades>>

TakeWriterS(p) ==
    /\ att[p] = "rd" /\ ~WriterXHeld
    /\ att' = Set(att, p, "tiers")
    /\ UNCHANGED <<pc, lockHolder, upgrades>>

Admit(p) ==
    /\ att[p] = "tiers"
    /\ att' = Set(att, p, "attached")
    /\ pc' = Set(pc, p, "idle")
    /\ UNCHANGED <<lockHolder, upgrades>>

Exit(p) ==
    /\ pc[p] = "idle" /\ att[p] = "attached"
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p, "exited")
    /\ UNCHANGED <<lockHolder, upgrades>>

-----------------------------------------------------------------------------
(* Writer exclusion. What the holder does under it is abstract.            *)

Request(p) ==
    /\ CanWrite(p) /\ pc[p] = "idle" /\ att[p] = "attached"
    /\ pc' = Set(pc, p, "waiting")
    /\ UNCHANGED <<att, lockHolder, upgrades>>

Lock(p) ==
    /\ pc[p] = "waiting" /\ lockHolder = None
    /\ lockHolder' = p
    /\ pc' = Set(pc, p, "held")
    /\ UNCHANGED <<att, upgrades>>

Release(p) ==
    /\ pc[p] = "held"
    /\ lockHolder' = None
    /\ pc' = Set(pc, p, "idle")
    /\ UNCHANGED <<att, upgrades>>

-----------------------------------------------------------------------------
(* Upgrading. The upgrader drains while it holds its shared tiers, gives   *)
(* them up before it waits for the gate exclusively, takes the gate, then  *)
(* the reader tier if the upgrade is unsafe for readers, then the writer   *)
(* tier; what it does holding them is abstract. A safe upgrade lets the    *)
(* gate reopen once it holds its tiers.                                    *)

StartUpgrade(p) ==
    /\ Upgrader(p) /\ pc[p] = "idle" /\ att[p] = "attached"
    /\ upgrades < MaxUpgrades
    /\ upgrades' = upgrades + 1
    /\ pc' = Set(pc, p, "converting")
    /\ att' = Set(att, p, "draining")
    /\ UNCHANGED lockHolder

DrainDone(p) ==
    /\ att[p] = "draining"
    /\ att' = Set(att, p, "uWGate")
    /\ UNCHANGED <<pc, lockHolder, upgrades>>

TakeGateX(p) ==
    /\ att[p] = "uWGate" /\ ~GateSHeld /\ ~GateXHeld
    /\ att' = Set(att, p, "uGate")
    /\ UNCHANGED <<pc, lockHolder, upgrades>>

TakeFirstTier(p) ==
    /\ att[p] = "uGate"
    /\ IF TakesReaderTier(p)
          THEN /\ ~ReaderSHeld /\ ~ReaderXHeld
               /\ att' = Set(att, p, "uRd")
          ELSE /\ ~WriterSHeld /\ ~WriterXHeld
               /\ att' = Set(att, p, "uTiers")
    /\ UNCHANGED <<pc, lockHolder, upgrades>>

TakeSecondTier(p) ==
    /\ att[p] = "uRd" /\ ~WriterSHeld /\ ~WriterXHeld
    /\ att' = Set(att, p, "uTiers")
    /\ UNCHANGED <<pc, lockHolder, upgrades>>

BeginBody(p) ==
    /\ att[p] = "uTiers"
    /\ att' = Set(att, p, "uBody")
    /\ UNCHANGED <<pc, lockHolder, upgrades>>

\* The upgrade ends and its handle with it.
EndUpgrade(p) ==
    /\ att[p] = "uBody"
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p, "exited")
    /\ UNCHANGED <<lockHolder, upgrades>>

-----------------------------------------------------------------------------

Next ==
    \E p \in Procs :
        \/ BeginAttach(p) \/ TakeGateS(p) \/ TakeReaderS(p) \/ DropGate(p)
        \/ TakeWriterS(p) \/ Admit(p) \/ Exit(p)
        \/ Request(p) \/ Lock(p) \/ Release(p)
        \/ StartUpgrade(p) \/ DrainDone(p) \/ TakeGateX(p) \/ TakeFirstTier(p)
        \/ TakeSecondTier(p) \/ BeginBody(p) \/ EndUpgrade(p)

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Fairness.                                                               *)

\* A process's take of each resource. Each process waits for at most one thing at a
\* time and leaves a wait only by taking it, so fairness on the disjunction of a
\* process's takes is fairness on each, and one condition per process keeps the
\* formula small enough for TLC.
Resources == {"lock", "gateS", "gateX", "readerTier", "writerTier"}
Takes(p, rs) ==
    \/ "lock" \in rs /\ Lock(p)
    \/ "gateS" \in rs /\ TakeGateS(p)
    \/ "gateX" \in rs /\ TakeGateX(p)
    \/ "readerTier" \in rs /\ (TakeReaderS(p) \/ (TakeFirstTier(p) /\ TakesReaderTier(p)))
    \/ "writerTier" \in rs
       /\ \/ TakeWriterS(p)
          \/ TakeFirstTier(p) /\ ~TakesReaderTier(p)
          \/ TakeSecondTier(p)

\* The steps that are no one's choice once under way, each an I/O the contract
\* assumes completes; each is enabled from its own state alone, so fairness on the
\* disjunction is fairness on each.
Completes(p) == DropGate(p) \/ Admit(p) \/ DrainDone(p) \/ BeginBody(p) \/ EndUpgrade(p)

\* Every hold ends and every handle eventually detaches: the assumption this layer
\* makes of the one above it. Detaching is strongly fair because a request between
\* attachments interrupts it.
HoldsEnd == \A p \in Procs : WF_vars(Release(p)) /\ SF_vars(Exit(p))
FairIO == \A p \in Procs : WF_vars(Completes(p))

\* The contract's fairness: a take whose resource stays grantable to that process is
\* made, since a lock call whose lock stays grantable returns. Waiters are unordered.
GrantedTakes(rs) == \A p \in Procs : WF_vars(Takes(p, rs))

ContractSpec == Spec /\ HoldsEnd /\ FairIO /\ GrantedTakes(Resources)

\* Each condition dropped, as a control.
ContractSpecWithoutRelease ==
    Spec /\ (\A p \in Procs : SF_vars(Exit(p))) /\ FairIO /\ GrantedTakes(Resources)
ContractSpecWithoutIO == Spec /\ HoldsEnd /\ GrantedTakes(Resources)
ContractSpecWithoutLock == Spec /\ HoldsEnd /\ FairIO /\ GrantedTakes(Resources \ {"lock"})
ContractSpecWithoutGateS == Spec /\ HoldsEnd /\ FairIO /\ GrantedTakes(Resources \ {"gateS"})
ContractSpecWithoutGateX == Spec /\ HoldsEnd /\ FairIO /\ GrantedTakes(Resources \ {"gateX"})
ContractSpecWithoutReaderTier ==
    Spec /\ HoldsEnd /\ FairIO /\ GrantedTakes(Resources \ {"readerTier"})
ContractSpecWithoutWriterTier ==
    Spec /\ HoldsEnd /\ FairIO /\ GrantedTakes(Resources \ {"writerTier"})

\* Strong fairness on each acquisition: a take that is grantable again and again is
\* made. It implies the contract's weak fairness on each.
FairSpec ==
    Spec /\ HoldsEnd /\ FairIO /\ \A p \in Procs : SF_vars(Takes(p, Resources))

-----------------------------------------------------------------------------
(* Invariants.                                                             *)

\* Audience: internal.
TypeOK ==
    /\ att \in [Procs -> AttStates]
    /\ pc \in [Procs -> PcStates]
    /\ lockHolder \in Procs \cup {None}
    /\ upgrades \in 0..MaxUpgrades

\* Audience: client. At most one process writes at a time. Shows through: no two
\* writes overlap.
OneWriterAtATime == \A p \in Procs : (pc[p] = "held") <=> (lockHolder = p)

\* Audience: internal.
\* The gate is held exclusively by at most one process, and then by no one shared.
GateIsExclusive ==
    /\ \A p, q \in Procs : HoldsGateX(p) /\ HoldsGateX(q) => p = q
    /\ GateXHeld => ~GateSHeld

\* Audience: internal.
\* Each tier is held exclusively by at most one process, and then by no one shared.
TiersAreExclusive ==
    /\ \A p, q \in Procs : HoldsReaderX(p) /\ HoldsReaderX(q) => p = q
    /\ \A p, q \in Procs : HoldsWriterX(p) /\ HoldsWriterX(q) => p = q
    /\ ReaderXHeld => ~ReaderSHeld
    /\ WriterXHeld => ~WriterSHeld

-----------------------------------------------------------------------------
(* Progress.                                                               *)

WaitingAtt == {"wGate", "hasGate", "rd", "uWGate", "uGate", "uRd"}
NobodyWaits == \A p \in Procs : pc[p] # "waiting" /\ att[p] \notin WaitingAtt
AnyAcq == \E p \in Procs : Takes(p, Resources)

\* Audience: internal.
\* The store as a whole makes progress: while anyone waits for a resource, some
\* resource is acquired again and again.
StoreMakesProgress == []<>NobodyWaits \/ []<><<AnyAcq>>_vars

\* Audience: client. An upgrade that holds its tiers completes. Shows through: the
\* return of upgrade_schema.
AnUpgradeHoldingItsTiersCompletes ==
    \A p \in Procs : (att[p] = "uTiers") ~> (att[p] \notin {"uTiers", "uBody"})

\* Audience: client. A writer that waits for exclusion is eventually served. Shows
\* through: the return of a write.
AWaitingWriterIsServed ==
    \A p \in Procs : (pc[p] = "waiting") ~> (pc[p] # "waiting")

\* Audience: client. An upgrader that waits for the gate or its tiers is eventually
\* served. Shows through: the return of upgrade_schema.
AWaitingUpgraderIsServed ==
    \A p \in Procs :
        (att[p] \in {"uWGate", "uGate", "uRd"}) ~> (att[p] \notin {"uWGate", "uGate", "uRd"})

\* Audience: client. A handle that waits to attach is eventually admitted. Shows
\* through: the return of an attach.
AnAttachingHandleIsServed ==
    \A p \in Procs :
        (att[p] \in {"wGate", "hasGate", "rd"}) ~> (att[p] \notin {"wGate", "hasGate", "rd"})

-----------------------------------------------------------------------------
(* Witnesses: each is violated by a behaviour that shows its state is      *)
(* reachable, or that it goes on forever.                                  *)

\* Grants of writer exclusion eventually stop, which a behaviour with a grant in a
\* cycle violates.
Witness_GrantsGoOnForever == <>[][~(\E p \in Procs : Lock(p))]_vars

\* Admissions eventually stop, which a behaviour with an admission in a cycle violates.
Witness_AdmissionsGoOnForever == <>[][~(\E p \in Procs : Admit(p))]_vars

\* No upgrade ever holds its tiers, which a behaviour with an upgrade body violates.
Witness_AnUpgradeHoldsItsTiers == \A p \in Procs : att[p] # "uBody"

=============================================================================
