\* Copyright (c) Michael Grier.
------------------------ MODULE CoordinationLiveness ------------------------
(***************************************************************************)
(* What the contract's lack of a fairness requirement does: the fairness   *)
(* the contract guarantees or assumes, the progress it buys, and the       *)
(* requests it leaves to be passed over. These are checked in the cyclic   *)
(* configurations (Cyclic), where processes go on writing, reading and     *)
(* attaching forever and a behaviour can repeat a state.                   *)
(*                                                                         *)
(* ContractSpec carries only the fairness the contract guarantees or       *)
(* assumes: every holder eventually releases and every handle eventually   *)
(* detaches, every I/O eventually completes, and for each synchronization  *)
(* resource -- writer exclusion, each mode of the admission gate, each     *)
(* tier -- weak fairness on each process's acquisition: a lock call whose  *)
(* lock stays grantable returns, which is an I/O completing. Waiters stay  *)
(* unordered: one whose take is grantable only now and then, because       *)
(* another waiter wins each time the resource frees, can be passed over.   *)
(* Each ContractSpecWithout* drops one of those conditions, as a control.  *)
(* FairSpec makes each acquisition strongly fair. UpgraderFairSpec keeps   *)
(* only the fairness an upgrader's service depends on, which TLC can check *)
(* with three processes, and UpgraderFairSpecExclusiveFirst also withholds *)
(* shared admission at the gate while an upgrader waits for it             *)
(* exclusively, which is an option and not a rule of the contract.         *)
(***************************************************************************)
EXTENDS CoordinationWitnesses

-----------------------------------------------------------------------------
(* Steps and acquisitions.                                                 *)

\* A write's first effect.
AWrite == \E p \in Procs : pc[p] \in {"open", "prepared"} /\ pc'[p] = "writing"

\* A handle is admitted.
AnAdmission == \E p \in Procs : att[p] # "attached" /\ att'[p] = "attached"

\* An upgrader's exclusive take of each tier.
TakesReaderTierX(p) ==
    \/ TakeFirstTier(p) /\ att'[p] = "uRd"
    \/ TakeSecondTier(p) /\ att[p] = "uWr"
TakesWriterTierX(p) ==
    \/ TakeFirstTier(p) /\ att'[p] # "uRd"
    \/ TakeSecondTier(p) /\ att[p] = "uRd"

\* The acquisitions of each synchronization resource, by any process.
LockAcq == \E p \in Procs : Lock(p)
GateSAcq == \E p \in Procs : TakeGateS(p)
GateXAcq == \E p \in Procs : TakeGateX(p)
ReaderTierAcq == \E p \in Procs : TakeReaderS(p) \/ TakesReaderTierX(p)
WriterTierAcq == \E p \in Procs : TakeWriterS(p) \/ TakesWriterTierX(p)
AnyAcq == LockAcq \/ GateSAcq \/ GateXAcq \/ ReaderTierAcq \/ WriterTierAcq

\* The steps of an operation, a read, a recovery, an attachment or an upgrade once it is
\* under way that are no one's choice, each an I/O the contract assumes completes.
Completes(p) ==
    \/ EndOpen(p) \/ FirstEffect(p) \/ LastEffect(p) \/ BeginClose(p) \/ EndClose(p)
    \/ LeaveInner(p)
    \/ Ingest(p) \/ Prepare(p) \/ End(p) \/ Sweep(p) \/ Unlock(p)
    \/ RecWhole(p) \/ RecSwap1(p) \/ RecSwap2(p) \/ RecEnd(p) \/ RecReset(p)
    \/ RecSweep(p) \/ RecCount(p) \/ RecClear(p)
    \/ ReadBefore2(p) \/ ObserveA(p) \/ ObserveB(p) \/ ReadAfter(p) \/ ReadAfter1(p)
    \/ ReadAfter2(p)
    \/ DropGate(p) \/ Admit(p) \/ DrainDone(p) \/ BeginUpgrade(p) \/ EndUpgrade(p)
    \/ UpgradeMoved(p) \/ SetFence(p) \/ Record(p)

-----------------------------------------------------------------------------
(* Fairness.                                                               *)

\* Fairness is stated only for the processes whose role can take the step: for any other
\* the step is never enabled and its fairness holds trivially, and TLC expands every
\* fairness conjunct, so each one it need not carry keeps the formula within its limit.
WriterProcs == {p \in Procs : CanWrite(p)}
UpgraderProcs == {p \in Procs : Upgrader(p)}

\* Strong fairness, since a holder's or handle's own choices -- an operation nested under
\* the hold, a request between attachments -- can interrupt the step again and again.
FairRelease ==
    (\A p \in WriterProcs : SF_vars(Release(p))) /\ \A p \in Procs : SF_vars(Exit(p))
\* An operation's last effect has its own, since nested operations under the hold can go on
\* completing while it waits.
FairIO ==
    (\A p \in Procs : SF_vars(Completes(p))) /\ \A p \in WriterProcs : SF_vars(LastEffect(p))
\* A take whose resource stays grantable to that process is made: the lock call returns.
FairLock == \A p \in WriterProcs : WF_vars(Lock(p))
FairGateS == \A p \in Procs : WF_vars(TakeGateS(p))
FairGateX == \A p \in UpgraderProcs : WF_vars(TakeGateX(p))
FairReaderTier ==
    (\A p \in Procs : WF_vars(TakeReaderS(p)))
    /\ \A p \in UpgraderProcs : WF_vars(TakesReaderTierX(p))
FairWriterTier ==
    (\A p \in WriterProcs : WF_vars(TakeWriterS(p)))
    /\ \A p \in UpgraderProcs : WF_vars(TakesWriterTierX(p))

ContractFairness ==
    FairRelease /\ FairIO /\ FairLock /\ FairGateS /\ FairGateX /\ FairReaderTier
    /\ FairWriterTier

ContractSpec == Init /\ [][Next]_vars /\ ContractFairness

ContractSpecWithoutRelease ==
    Init /\ [][Next]_vars /\ FairIO /\ FairLock /\ FairGateS /\ FairGateX
    /\ FairReaderTier /\ FairWriterTier
ContractSpecWithoutIO ==
    Init /\ [][Next]_vars /\ FairRelease /\ FairLock /\ FairGateS /\ FairGateX
    /\ FairReaderTier /\ FairWriterTier
ContractSpecWithoutLock ==
    Init /\ [][Next]_vars /\ FairRelease /\ FairIO /\ FairGateS /\ FairGateX
    /\ FairReaderTier /\ FairWriterTier
ContractSpecWithoutGateS ==
    Init /\ [][Next]_vars /\ FairRelease /\ FairIO /\ FairLock /\ FairGateX
    /\ FairReaderTier /\ FairWriterTier
ContractSpecWithoutGateX ==
    Init /\ [][Next]_vars /\ FairRelease /\ FairIO /\ FairLock /\ FairGateS
    /\ FairReaderTier /\ FairWriterTier
ContractSpecWithoutReaderTier ==
    Init /\ [][Next]_vars /\ FairRelease /\ FairIO /\ FairLock /\ FairGateS
    /\ FairGateX /\ FairWriterTier
ContractSpecWithoutWriterTier ==
    Init /\ [][Next]_vars /\ FairRelease /\ FairIO /\ FairLock /\ FairGateS
    /\ FairGateX /\ FairReaderTier

\* Strong fairness on each individual acquisition but shared admission at the gate.
EachOtherAcquisition ==
    /\ \A p \in WriterProcs : SF_vars(Lock(p)) /\ SF_vars(TakeWriterS(p))
    /\ \A p \in Procs : SF_vars(TakeReaderS(p))
    /\ \A p \in UpgraderProcs :
        SF_vars(TakeGateX(p)) /\ SF_vars(TakesReaderTierX(p)) /\ SF_vars(TakesWriterTierX(p))

\* Strong fairness on each individual acquisition. It implies the weak fairness of
\* ContractSpec on each, which is left out for the size of the formula, so FairSpec is
\* ContractSpec with strong fairness on each acquisition.
FairSpec ==
    Init /\ [][Next]_vars /\ FairRelease /\ FairIO /\ EachOtherAcquisition
    /\ \A p \in Procs : SF_vars(TakeGateS(p))

\* An upgrader waits for the gate exclusively.
ExclusiveWaiter == \E q \in Procs : att[q] \in {"uWGate", "uWGateT"}

\* The option of withholding shared admission at the gate while an upgrader waits for it.
NextExclusiveFirst == Next /\ ~(ExclusiveWaiter /\ GateSAcq)

\* An upgrader's exclusive takes are each strongly fair.
UpgraderTakes ==
    \A p \in UpgraderProcs :
        SF_vars(TakeGateX(p)) /\ SF_vars(TakesReaderTierX(p)) /\ SF_vars(TakesWriterTierX(p))

\* What an upgrader's service depends on besides its own takes: every handle eventually
\* exits, every I/O completes, and a handle's shared take that stays grantable is made.
\* The fairness of writes is left out: an upgrader that never gets past a write never
\* waits as an upgrader, and TLC cannot carry FairSpec's whole formula for three
\* processes. FairSpec, and ContractSpec with UpgraderTakes, each imply it, so what holds
\* under UpgraderFairSpec holds under both.
HandleFairness ==
    /\ \A p \in Procs : SF_vars(Exit(p)) /\ SF_vars(Completes(p)) /\ WF_vars(TakeReaderS(p))
    /\ \A p \in UpgraderProcs : WF_vars(TakeWriterS(p))

UpgraderFairSpec ==
    Init /\ [][Next]_vars /\ HandleFairness /\ UpgraderTakes
    /\ \A p \in Procs : WF_vars(TakeGateS(p))

\* UpgraderFairSpec, with shared admission withheld while an upgrader waits for the gate.
UpgraderFairSpecExclusiveFirst ==
    Init /\ [][NextExclusiveFirst]_vars /\ HandleFairness /\ UpgraderTakes
    /\ \A p \in Procs : WF_vars(TakeGateS(p) /\ ~ExclusiveWaiter)

-----------------------------------------------------------------------------
(* Progress.                                                               *)

\* Where a process waits for a resource: writer exclusion, the gate or a tier.
WaitingAtt == {"wGate", "hasGate", "gateWr", "rd", "rdChecked", "uWGate", "uWGateT",
               "uGate", "uRd", "uWr"}
NobodyWaits == \A p \in Procs : pc[p] # "waiting" /\ att[p] \notin WaitingAtt

\* Audience: internal.
\* The store as a whole makes progress: while anyone waits for a resource, some
\* resource is acquired again and again.
StoreMakesProgress == []<>NobodyWaits \/ []<><<AnyAcq>>_vars

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

\* Audience: client. A writer that waits for exclusion, and neither gives up nor dies, is
\* eventually served. Shows through: the return of a write.
AWaitingWriterIsServed ==
    \A p \in Procs : (pc[p] = "waiting") ~> (pc[p] # "waiting")

\* Audience: client. An upgrader that waits for the gate or its tiers is eventually served.
\* Shows through: the return of upgrade_schema.
AWaitingUpgraderIsServed ==
    \A p \in Procs :
        (att[p] \in {"uWGate", "uWGateT", "uGate", "uRd", "uWr"})
            ~> (att[p] \notin {"uWGate", "uWGateT", "uGate", "uRd", "uWr"})

\* Audience: client. A handle that waits to attach is eventually admitted or refused. Shows
\* through: the return of an attach.
AnAttachingHandleIsServed ==
    \A p \in Procs :
        (att[p] \in {"wGate", "hasGate", "gateWr", "rd", "rdChecked"})
            ~> (att[p] \notin {"wGate", "hasGate", "gateWr", "rd", "rdChecked"})

-----------------------------------------------------------------------------
(* Witnesses: each is violated by a behaviour that goes on forever.        *)

\* Writes eventually stop, which a behaviour with a write in a cycle violates.
Witness_WritesGoOnForever == <>[][~AWrite]_vars

\* Admissions eventually stop, which a behaviour with an admission in a cycle violates.
Witness_AdmissionsGoOnForever == <>[][~AnAdmission]_vars

=============================================================================
