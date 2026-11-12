\* Copyright (c) Michael Grier.
------------------------ MODULE CoordinationLiveness ------------------------
(***************************************************************************)
(* Every hold ends: what the coordination model owes the acquisition model *)
(* it refines (CoordinationAcquisition.tla), whose results about who is    *)
(* served assume it. A write releases writer exclusion; an attach drops    *)
(* the gate and is admitted or refused; a drain ends; a detach completes;  *)
(* an upgrade holding its tiers finishes or gives up; a recovery under way *)
(* ends. A process's death ends all of them. Each is a property of one     *)
(* operation, attachment or upgrade body, so it is checked on the bounded  *)
(* configurations, under HoldSpec: the fairness of a process's own steps,  *)
(* and none among waiters. Each HoldSpecWithout* drops one condition, so   *)
(* that a hold can last forever, as a control.                             *)
(***************************************************************************)
EXTENDS CoordinationWitnesses

-----------------------------------------------------------------------------
(* A process's own steps.                                                  *)

\* The steps of an operation, a read, a recovery, an attachment or an upgrade once it is
\* under way that are no one's choice, each an I/O the contract assumes completes.
Completes(p) ==
    \/ EndOpen(p) \/ FirstEffect(p) \/ LastEffect(p) \/ Observe(p) \/ BeginClose(p)
    \/ EndClose(p)
    \/ Ingest(p) \/ Prepare(p) \/ End(p) \/ Sweep(p) \/ Unlock(p)
    \/ RecWhole(p) \/ RecSwap1(p) \/ RecSwap2(p) \/ RecEnd(p) \/ RecReset(p)
    \/ RecSweep(p) \/ RecCount(p) \/ RecClear(p)
    \/ ReadBefore2(p) \/ ObserveA(p) \/ ObserveB(p) \/ ReadAfter(p) \/ ReadAfter1(p)
    \/ ReadAfter2(p)
    \/ DropGate(p) \/ Admit(p) \/ DrainDone(p) \/ BeginUpgrade(p) \/ EndUpgrade(p)
    \/ UpgradeMoved(p) \/ SetFence(p) \/ Record(p)

\* What a holder does with its operation once it has it: a transaction begins, or the
\* operation ends with nothing to change, and an upgrade body records its intent and
\* goes through its steps.
Advances(p) ==
    \/ Begin(p) \/ EndEmpty(p)
    \/ RecordIntent(p) \/ UpStep(p) \/ UpAdvance(p) \/ UpClear(p)

\* A handle's work in the registry, and its listing's, once begun.
Registers(p) ==
    \/ RegStart(p) \/ (\E s \in Slots : RegClaim(p, s)) \/ RegLive(p) \/ RegWrite(p)
    \/ RegRelease(p) \/ RegOverflow(p) \/ UpdTake(p) \/ UpdDone(p)
    \/ UnregNone(p) \/ UnregClaim(p) \/ UnregSkip(p) \/ UnregClear(p) \/ UnregRelease(p)
    \/ Detach(p)
    \/ ListClaim(p) \/ ListRead(p) \/ ListProbe(p) \/ ListDone(p)

\* Each leads its process out of where it is, apart from the loops a process's own
\* choices make, so one condition covers them all and keeps the formula small enough for
\* TLC.
OwnSteps(p) == Completes(p) \/ Advances(p) \/ Registers(p)

-----------------------------------------------------------------------------
(* Fairness.                                                               *)

WriterProcs == {p \in Procs : CanWrite(p)}

\* Strong fairness, since a holder's own choice interrupts it: a recovery stopped and
\* started again keeps a told holder from its release.
FairRelease == \A p \in WriterProcs : SF_vars(Release(p))
FairOwnSteps == \A p \in Procs : SF_vars(OwnSteps(p))
\* A clone's operation ends; while its handle drains no new one starts.
FairLeases == \A p \in Procs : WF_vars(EndOp(p))

HoldSpec == Spec /\ FairRelease /\ FairOwnSteps /\ FairLeases

\* Each condition dropped, as a control.
HoldSpecWithoutRelease == Spec /\ FairOwnSteps /\ FairLeases
HoldSpecWithoutOwnSteps == Spec /\ FairRelease /\ FairLeases
HoldSpecWithoutLeases == Spec /\ FairRelease /\ FairOwnSteps

-----------------------------------------------------------------------------
(* Every hold ends.                                                        *)

\* Audience: internal.
\* Writer exclusion, once taken, is released, by the holder or by its death.
AHoldOfExclusionEnds == \A p \in Procs : (lockHolder = p) ~> (lockHolder # p)

\* Audience: internal.
\* A handle holding the gate and the reader tier drops the gate.
AnAttachDropsTheGate == \A p \in Procs : (att[p] = "gateRd") ~> (att[p] # "gateRd")

\* Audience: client. A handle holding every tier it needs is admitted or refused. Shows
\* through: the return of an attach.
AnAdmissionCompletes ==
    \A p \in Procs :
        (att[p] \in {"tiers", "registering"}) ~> (att[p] \notin {"tiers", "registering"})

\* Audience: client. A drain ends, so an upgrade that has begun to drain goes on to wait
\* for the gate, or is cancelled. Shows through: the return of upgrade_schema.
ADrainEnds == \A p \in Procs : (att[p] = "draining") ~> (att[p] # "draining")

\* Audience: client. A handle that is leaving the registry lets go of its tiers. Shows
\* through: the return of a detach.
ADetachCompletes == \A p \in Procs : (att[p] = "unregistering") ~> (att[p] # "unregistering")

\* Audience: client. An upgrade that holds its tiers completes. Shows through: the return of
\* upgrade_schema.
\* An upgrade holding every tier it needs ends: it begins and completes, or finds the
\* schema moved and gives up; one whose process dies ends with it.
AnUpgradeHoldingItsTiersCompletes ==
    \A p \in Procs : (att[p] = "uTiers") ~> (att[p] \notin {"uTiers", "uBody"} \/ pc[p] = "dead")

\* Audience: client. A recovery that is driven completes. Shows through: the return of the
\* recovery run.
\* A recovery under way ends: it completes, or is stopped between steps.
ADrivenRecoveryCompletes ==
    \A p \in Procs : (pc[p] \in RecStates) ~> (pc[p] \notin RecStates)

=============================================================================
