\* Copyright (c) Michael Grier.
\* Split from Coordination.tla at 70668aa.
----------------------- MODULE CoordinationWitnesses ------------------------
EXTENDS CoordinationProperties

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
