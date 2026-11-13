\* Copyright (c) Michael Grier.
\* Split from Coordination.tla at 70668aa.
----------------------- MODULE CoordinationWitnesses ------------------------
EXTENDS CoordinationUpgradeProperties

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

Witness_ACallbackReturns ==
    [][~(\E p \in Procs : pc[p] = "wrote" /\ pc'[p] = "observed")]_vars

\* A waiter that registered and slept is woken by a change to what it waits for.
Witness_AWaiterIsNotified ==
    [][~(\E p \in Procs : woken[p] = "asleep" /\ woken'[p] = "sample" /\ Ready(p))]_vars

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
            /\ SpeaksOf[p][ulayer] = utgt)]_vars

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

-----------------------------------------------------------------------------
(* Transactions.                                                           *)

Witness_ATransactionCommits ==
    [][~(\E p \in Procs : pc[p] = "applied" /\ pc'[p] = "ended")]_vars

Witness_AFailureBeforeIngestIsRolledBack ==
    [][~(\E p \in Procs : pc[p] = "begun" /\ pc'[p] = "wrote")]_vars

Witness_AFailureAfterIngestIsRolledBack ==
    [][~(\E p \in Procs : pc[p] = "ingested" /\ pc'[p] = "wrote")]_vars

Witness_PrepareFailsWithoutARecord ==
    [][~(\E p \in Procs : pc[p] = "ingested" /\ pc'[p] = "told"
                          /\ log' = "begun")]_vars

Witness_PrepareFailsWithARecord ==
    [][~(\E p \in Procs : pc[p] = "ingested" /\ pc'[p] = "told"
                          /\ log' = "prepared")]_vars

Witness_ASwapFailsAfterPrepare ==
    [][~(\E p \in Procs : pc[p] = "prepared" /\ pc'[p] = "told")]_vars

Witness_TheEndRecordFails ==
    [][~(\E p \in Procs : pc[p] = "applied" /\ pc'[p] = "told")]_vars

Witness_TheSweepFails ==
    [][~(\E p \in Procs : pc[p] = "ended" /\ pc'[p] = "told")]_vars

Witness_TheLogIsReset ==
    [][~(\E p \in Procs : pc[p] \in {"held", "closed"} /\ log # "empty"
                          /\ log' = "empty")]_vars

Witness_ToldAfterDeathAfterBegin ==
    ~(diedIn = "begun" /\ HoldsTold)

Witness_ToldAfterDeathAfterIngest ==
    ~(diedIn = "ingested" /\ HoldsTold)

Witness_ToldAfterDeathAfterPrepare ==
    ~(diedIn = "prepared" /\ log = "prepared" /\ HoldsTold)

Witness_ToldAfterDeathAfterTheSwaps ==
    ~(diedIn = "applied" /\ HoldsTold)

Witness_ToldAfterDeathAfterEnd ==
    ~(diedIn = "ended" /\ HoldsTold)

Witness_ACrashLeavesAPreparedTransaction ==
    ~(diedIn = "crash" /\ log = "prepared")

-----------------------------------------------------------------------------
(* Recovery and discovery.                                                 *)

Witness_ARecoveryRollsAPreparedTransactionForward ==
    [][~(\E p \in Procs : pc[p] = "recEnd" /\ log = "prepared" /\ log' = "ended")]_vars

Witness_AnUnloggedWriteIsRolledForward ==
    [][~(\E p \in Procs : pc[p] = "recWhole" /\ b' # b)]_vars

Witness_AnUnloggedWriteIsRolledBack ==
    [][~(\E p \in Procs : pc[p] = "recWhole" /\ a' # a)]_vars

Witness_AnUnloggedWriteFollowsATransaction ==
    ~(\E p \in Procs : pc[p] = "writing" /\ ~TxnOp /\ log = "ended")

Witness_ARecoveryFollowsARestartWithNothingHalfApplied ==
    [][~(\E p \in Procs : pc[p] = "told" /\ pc'[p] = "recEnd" /\ ~trusted /\ ~Partial)]_vars

Witness_ARecoveryRollsABegunTransactionBack ==
    [][~(\E p \in Procs : pc[p] = "recReset" /\ log' = "empty")]_vars

Witness_ARecoveryCompletes ==
    [][~(\E p \in Procs : pc[p] = "recClear" /\ pc'[p] = "closed")]_vars

Witness_ARecoveryClearsTheFence ==
    [][~(fence /\ ~fence')]_vars

Witness_ARecoveryRestoresTrust ==
    [][~(~trusted /\ trusted')]_vars

Witness_ARecoveryFollowsAHostCrash ==
    [][~(\E p \in Procs : pc[p] = "recCount" /\ ~trusted)]_vars

\* A prepared transaction means the count was odd, so an even stored count beside
\* one is a count a crash rolled back.
Witness_ARecoveryFollowsACrashThatRolledTheCountBack ==
    [][~(\E p \in Procs : pc[p] = "told" /\ pc'[p] \in RecStates
                          /\ ~trusted /\ Even(count) /\ log = "prepared")]_vars

Witness_ARecoveryFollowsAHoldersDeath ==
    [][~(\E p \in Procs : pc[p] = "recCount" /\ pc'[p] = "recClear"
                          /\ diedIn \in TxnStates)]_vars

Witness_ARecoveryFollowsAnUnresolvedFailure ==
    [][~(\E p \in Procs : pc[p] = "recClear" /\ pc'[p] = "closed"
                          /\ deaths = 0 /\ crashes = 0)]_vars

Witness_ARecoveryIsStopped ==
    [][~(\E p \in Procs : pc[p] \in RecStates /\ pc'[p] = "told")]_vars

Witness_ARecovererDiesAfterTheFirstSwap ==
    ~(diedIn = "recSwap2" /\ HoldsTold)

Witness_ARecovererDiesAfterTheSwaps ==
    ~(diedIn = "recEnd" /\ HoldsTold)

Witness_ARecovererDiesAfterTheEndRecord ==
    ~(diedIn = "recSweep" /\ HoldsTold)

Witness_ARecovererDiesAfterTheSweep ==
    ~(diedIn = "recCount" /\ HoldsTold)

Witness_ARecovererDiesAfterTheCount ==
    ~(diedIn = "recClear" /\ HoldsTold)

Witness_ARecoveryIsResumedAfterADeath ==
    [][~(\E p \in Procs : pc[p] = "recCount" /\ pc'[p] = "recClear"
                          /\ diedIn \in RecStates)]_vars

\* A recoverer that died after its first swap left the namespace half applied; the
\* next one finishes the swaps, writes the end record and sweeps before it
\* publishes the count.
Witness_ARecoveryIsResumedAfterADeathBetweenItsSwaps ==
    [][~(\E p \in Procs : pc[p] = "recCount" /\ pc'[p] = "recClear"
                          /\ diedIn = "recSwap2" /\ log = "ended" /\ Settled)]_vars

Witness_DiscoveryFindsAHoldersAbandonedWork ==
    [][~(\E p \in Procs : Discover(p) /\ disc'[p] = "required"
                          /\ owner # None /\ lockHolder = None
                          /\ ~fence /\ trusted)]_vars

Witness_DiscoveryFindsTheFence ==
    [][~(\E p \in Procs : Discover(p) /\ disc'[p] = "required"
                          /\ fence /\ lockHolder = None)]_vars

Witness_DiscoveryFindsAnUntrustedState ==
    [][~(\E p \in Procs : Discover(p) /\ disc'[p] = "required"
                          /\ ~trusted /\ ~fence /\ owner = None)]_vars

Witness_DiscoveryMeetsALiveHoldersTransaction ==
    [][~(\E p \in Procs : Discover(p) /\ disc'[p] = "notRequired"
                          /\ \E h \in Procs : lockHolder = h /\ pc[h] \in TxnStates)]_vars

Witness_DiscoveryFindsAHealthyStore ==
    [][~(\E p \in Procs : Discover(p) /\ disc'[p] = "notRequired"
                          /\ lockHolder = None)]_vars

Witness_AReaderDiscovers ==
    [][~(\E p \in Procs : Discover(p) /\ ~CanWrite(p))]_vars
-----------------------------------------------------------------------------
(* The upgrade body.                                                       *)

UpgraderIn(p, s) == Upgrader(p) /\ att[p] = "uBody" /\ pc[p] = s

Witness_TheIntentIsRecorded ==
    [][~(\E p \in Procs : pc[p] = "open" /\ pc'[p] = "upBody")]_vars

Witness_AnUpgradeCompletesThroughItsBody ==
    [][~(\E p \in Procs : pc[p] = "upBody" /\ pc'[p] = "wrote")]_vars

Witness_AMigrationStepIsMade ==
    [][~(\E p \in Procs : pc[p] = "upBody" /\ ustep' > ustep)]_vars

Witness_TheSchemaAdvancesInTheBody ==
    [][~(\E p \in Procs : pc[p] = "upBody" /\ current' # current)]_vars

Witness_AnUnsafeUpgradeAdvancesTheOldestReaderSchema ==
    [][~(\E p \in Procs : pc[p] = "upBody" /\ oldest' # oldest)]_vars

Witness_AnUpgraderIsToldAndRefused ==
    [][~(\E p \in Procs : UpgraderIn(p, "locked") /\ pc'[p] = "told")]_vars

Witness_AToldUpgraderIsDetached ==
    [][~(\E p \in Procs : UpgraderIn(p, "releasing") /\ pc'[p] = "detached"
                          /\ areq'[p] = "refused")]_vars

Witness_AnUpgraderGivesUpBeforeTheIntent ==
    [][~(\E p \in Procs : UpgraderIn(p, "open") /\ pc'[p] = "wrote")]_vars

Witness_AnUpgradeFailsInItsBody ==
    [][~(\E p \in Procs : pc[p] = "upBody" /\ pc'[p] = "told")]_vars

Witness_AnUpgraderDiesBeforeTheIntent ==
    [][~(\E p \in Procs : Upgrader(p) /\ att[p] = "uBody"
                          /\ pc[p] \in {"held", "opening", "open"}
                          /\ pc'[p] = "dead")]_vars

Witness_AnUpgraderDiesBeforeItsFirstStep ==
    [][~(\E p \in Procs : UpgraderIn(p, "upBody") /\ ustep = 0 /\ pc'[p] = "dead")]_vars

Witness_AnUpgraderDiesBetweenSteps ==
    [][~(\E p \in Procs : UpgraderIn(p, "upBody") /\ ustep >= 1 /\ ustep < UpSteps
                          /\ pc'[p] = "dead")]_vars

Witness_AnUpgraderDiesAfterItsLastStep ==
    [][~(\E p \in Procs : UpgraderIn(p, "upBody") /\ ustep = UpSteps /\ ~Advanced
                          /\ pc'[p] = "dead")]_vars

Witness_AnUpgraderDiesAfterTheSchemaAdvances ==
    [][~(\E p \in Procs : UpgraderIn(p, "upBody") /\ Advanced /\ pc'[p] = "dead")]_vars

Witness_ToldAfterAnUpgradersDeath ==
    ~(diedIn = "upBody" /\ HoldsTold)

Witness_AnInterruptedUpgradeIsResumedBeforeAnyStep ==
    [][~(\E p \in Procs : pc[p] = "told" /\ pc'[p] = "recUp" /\ ustep = 0)]_vars

Witness_AnInterruptedUpgradeIsResumedMidMigration ==
    [][~(\E p \in Procs : pc[p] = "told" /\ pc'[p] = "recUp"
                          /\ ustep >= 1 /\ ustep < UpSteps)]_vars

Witness_AnInterruptedUpgradeIsResumedAfterTheSchemaAdvanced ==
    [][~(\E p \in Procs : pc[p] = "told" /\ pc'[p] = "recUp" /\ Advanced)]_vars

Witness_AResumerMakesTheRemainingSteps ==
    [][~(\E p \in Procs : pc[p] = "recUp" /\ ustep' > ustep)]_vars

Witness_AResumerAdvancesTheSchema ==
    [][~(\E p \in Procs : pc[p] = "recUp" /\ current' # current)]_vars

Witness_AnInterruptedUpgradeIsResumedToCompletion ==
    [][~(\E p \in Procs : pc[p] = "recUp" /\ pc'[p] = "recEnd")]_vars

Witness_AResumerIsStoppedInTheUpgrade ==
    [][~(\E p \in Procs : pc[p] = "recUp" /\ pc'[p] = "told")]_vars

Witness_AResumerDiesInTheUpgrade ==
    ~(diedIn = "recUp" /\ HoldsTold)

Witness_AReaderIsAdmittedDuringTheBody ==
    [][~(\E p, q \in Procs : ~CanWrite(p) /\ Admitted(p) /\ pc[q] = "upBody")]_vars

Witness_AnUpgradeCompletesAfterAFailedClosingPublication ==
    [][~(\E p \in Procs : UpgraderIn(p, "releasing") /\ pc'[p] = "exited"
                          /\ fence)]_vars

Witness_DiscoveryMeetsALiveUpgradersBody ==
    [][~(\E p, q \in Procs : Discover(p) /\ disc'[p] = "notRequired" /\ pc[q] = "upBody")]_vars

Witness_DiscoveryFindsAnInterruptedUpgrade ==
    [][~(\E p \in Procs : Discover(p) /\ disc'[p] = "required" /\ IntentStands
                          /\ lockHolder = None /\ ~fence /\ trusted)]_vars

-----------------------------------------------------------------------------
(* Cumulative reader compatibility.                                        *)

Witness_AReaderOfAnOlderSchemaSurvivesASafeUpgrade ==
    [][~(\E q \in Procs :
            current' # current /\ upState = "safe"
            /\ ~CanWrite(q) /\ att[q] = "attached" /\ att'[q] = "attached"
            /\ SpeaksOf[q][ulayer] < current[ulayer])]_vars

Witness_AReaderTwoSchemasBehindSurvivesASafeUpgrade ==
    [][~(\E q \in Procs :
            current' # current /\ upState = "safe"
            /\ ~CanWrite(q) /\ att[q] = "attached" /\ att'[q] = "attached"
            /\ SpeaksOf[q][ulayer] + 2 <= current'[ulayer])]_vars

Witness_AnUpgradeIsUnsafeAgainstTheOldestAndSafeAgainstTheCurrent ==
    [][~(ulayer = 0 /\ ulayer' # 0 /\ upState' = "unsafe"
         /\ ufloor' <= current[ulayer'])]_vars

Witness_AnUpgradeOfOneLayerRunsBesideHandlesOfAnother ==
    [][~(\E q \in Procs :
            att[q] # "attached" /\ att'[q] = "attached" /\ upState # "ordinary"
            /\ \E l \in Layers : l # ulayer /\ SpeaksOf[q][l] < current[l])]_vars

Witness_TwoLayersAreUpgradedInTurn ==
    [][~(\E l, m \in Layers :
            l # m /\ current'[l] # current[l] /\ current[m] # Current[m])]_vars

Witness_AWriterIsAdmittedBesideAnUpgradeOfAnotherLayer ==
    [][~(\E q \in Procs :
            CanWrite(q) /\ att[q] # "attached" /\ att'[q] = "attached"
            /\ upState # "ordinary" /\ SpeaksOf[q][ulayer] = utgt
            /\ \E l \in Layers : l # ulayer /\ SpeaksOf[q][l] = current[l])]_vars

Witness_AnUnsafeUpgradeOfTheSecondLayerCompletes ==
    [][~(upState = "unsafe" /\ ulayer = 2 /\ oldest'[2] # oldest[2])]_vars

=============================================================================
