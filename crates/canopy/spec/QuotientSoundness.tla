\* Copyright (c) Michael Grier.
---------------------------- MODULE QuotientSoundness ----------------------------
(***************************************************************************)
(* A one-step check that SharedPublicationCheck's QuotientView is sound.  *)
(*                                                                         *)
(* The first copy of the composition explores the reduced model. The      *)
(* second is given a perturbation of the first: the same state but for one *)
(* thing the view forgets -- a gap between versions, blob versions nothing *)
(* names replaced by others, the count moved by                            *)
(* two, a stale saved count, an older remembered generation, request      *)
(* intent or readiness no step can read, the cached revision, or the      *)
(* abstract transaction cells moved together. One copy then steps, and the *)
(* other must answer with a step that reaches the same view and gives      *)
(* every step property the same value. A perturbation that cannot be      *)
(* answered leaves TLC no step, which it reports as a deadlock whose trace *)
(* ends at the challenging action. Each perturbed state must also have the *)
(* same view and give every checked state predicate the same value.        *)
(*                                                                         *)
(* The check is local: it asks the question once for each view of the     *)
(* reduced model and each perturbation, so a revision that breaks the view *)
(* is found at the action that breaks it, at bounds that do not grow with  *)
(* the reference model's history.                                         *)
(***************************************************************************)
EXTENDS QuotientSoundnessPair

CONSTANT ViewControl   \* TRUE checks a view that also forgets the fence, a negative control
CONSTANT SoundnessFocus
CONSTANT SoundnessPerturbation

ASSUME SoundnessPerturbation \in
    {"all", "gap", "garbageLive", "garbageDur", "count", "cells", "shared",
     "cache", "stale", "forgotten", "older", "intent", "ready", "intervened"}

VARIABLES
    phase,   \* "reset", "challengeA", "respondB", "challengeB" or "respondA"
    kind,    \* the perturbation the second copy was given
    target   \* the view and step properties the challenger's step reached

pvars == <<spvars, bvars, phase, kind, target>>

NoTarget == <<>>

\* The view checked. The control's view also forgets the fence.
CheckedView ==
    LET v == QuotientView IN [i \in DOMAIN v |-> IF ViewControl /\ i = 6 THEN FALSE ELSE v[i]]
CheckedViewB ==
    LET v == B!QuotientView IN [i \in DOMAIN v |-> IF ViewControl /\ i = 6 THEN FALSE ELSE v[i]]

\* Every state predicate a SharedPublicationCheck configuration checks.
StateFacts ==
    <<DurablePointerHasBlob, LivePointerHasBlob,
      LivePointersHaveTheirNode, DurablePointersHaveTheirNode, EveryDocumentHasAMajority,
      OutcomeIsFixedOnceRecovered, AHealthyHolderFindsEveryDocumentSettled,
      DocumentWorkUsesCurrentGeneration, SavedRecoveryCountNeverLeads,
      StoredVersionsNeverLeadTheGeneration, OnlyOrdinaryRequestsConsumeTheBudget,
      CountIsOddWhileAnOperationIsUnresolved, AFinishedOperationLeavesTheStoreSettled,
      WritesBounded,
      Witness_ARecoverySettlesARecordOfTheSecondDocument,
      Witness_ARecoveryResolvesTheSecondDocument, Witness_ARecoveryRollsATransactionForward,
      Witness_ARecoveryCompletesAllUnits, Witness_ADirectoryDebtUnitRuns,
      Witness_AWriterIsRefusedBetweenRecoveryUnits, Witness_AHolderTerminatesWithARecordPending,
      Witness_RecoveryRollsForwardBothKindsOfWrite,
      Witness_ARestartLosesTheFenceWithARecordPending,
      Witness_ATransactionRemovesANode, Witness_ARecoveryRemovesANode,
      Witness_ADirectoryIsCreatedAndRenamed, Witness_ARecoveryCreatesADirectory>>
StateFactsB ==
    <<B!DurablePointerHasBlob, B!LivePointerHasBlob,
      B!LivePointersHaveTheirNode, B!DurablePointersHaveTheirNode, B!EveryDocumentHasAMajority,
      B!OutcomeIsFixedOnceRecovered, B!AHealthyHolderFindsEveryDocumentSettled,
      B!DocumentWorkUsesCurrentGeneration, B!SavedRecoveryCountNeverLeads,
      B!StoredVersionsNeverLeadTheGeneration, B!OnlyOrdinaryRequestsConsumeTheBudget,
      B!CountIsOddWhileAnOperationIsUnresolved, B!AFinishedOperationLeavesTheStoreSettled,
      B!WritesBounded,
      B!Witness_ARecoverySettlesARecordOfTheSecondDocument,
      B!Witness_ARecoveryResolvesTheSecondDocument, B!Witness_ARecoveryRollsATransactionForward,
      B!Witness_ARecoveryCompletesAllUnits, B!Witness_ADirectoryDebtUnitRuns,
      B!Witness_AWriterIsRefusedBetweenRecoveryUnits, B!Witness_AHolderTerminatesWithARecordPending,
      B!Witness_RecoveryRollsForwardBothKindsOfWrite,
      B!Witness_ARestartLosesTheFenceWithARecordPending,
      B!Witness_ATransactionRemovesANode, B!Witness_ARecoveryRemovesANode,
      B!Witness_ADirectoryIsCreatedAndRenamed, B!Witness_ARecoveryCreatesADirectory>>

\* Every step predicate a SharedPublicationCheck configuration's properties check.
StepFacts ==
    <<OnlyAHolderChangesADocumentStep, ADirectoryClearSurvivesOnlyLive, ALiveRollbackCollects,
      AWriteCollects, ARecoveryCollectsARolledBackTransaction,
      ARecoveryCollectsACommittedTransaction, ARollbackCollectionFailureRequiresRecovery,
      AStaleRecoveryRestarts, AWriteRemoveCollisionIsRejected,
      AWriteBeneathRemovedNodeIsRejected, ARemovalAtExistingDocumentIsRejected,
      ARemovalBeneathExistingDocumentIsRejected, ADirectoryIntentTears,
      ADirectoryIntentIsRepaired>>
StepFactsB ==
    <<B!OnlyAHolderChangesADocumentStep, B!ADirectoryClearSurvivesOnlyLive,
      B!ALiveRollbackCollects, B!AWriteCollects, B!ARecoveryCollectsARolledBackTransaction,
      B!ARecoveryCollectsACommittedTransaction, B!ARollbackCollectionFailureRequiresRecovery,
      B!AStaleRecoveryRestarts, B!AWriteRemoveCollisionIsRejected,
      B!AWriteBeneathRemovedNodeIsRejected, B!ARemovalAtExistingDocumentIsRejected,
      B!ARemovalBeneathExistingDocumentIsRejected, B!ADirectoryIntentTears,
      B!ADirectoryIntentIsRepaired>>

-----------------------------------------------------------------------------
(* Perturbations: each changes only something the view forgets.           *)

\* A gap opened in the versions at c: every version from c up moves up by one.
Gap(c, v) == IF v \in Nat /\ v >= c THEN v + 1 ELSE v
GapSwaps(c, swaps) ==
    [index \in DOMAIN swaps |->
        [doc |-> swaps[index].doc,
         ver |-> Gap(c, swaps[index].ver),
         effect |-> swaps[index].effect,
         precondition |-> swaps[index].precondition,
         expected |-> Gap(c, swaps[index].expected)]]

VersionGap(c) ==
    [Snap EXCEPT
        !.gen = Gap(c, gen),
        !.mygen = [h \in Procs |-> Gap(c, mygen[h])],
        !.ptrLive = [d \in Docs |-> [r \in Ptr |-> Gap(c, ptrLive[d][r])]],
        !.ptrDur = [d \in Docs |-> [r \in Ptr |-> Gap(c, ptrDur[d][r])]],
        !.blobLive = [d \in Docs |-> [k \in Blob |-> {Gap(c, v) : v \in blobLive[d][k]}]],
        !.blobDur = [d \in Docs |-> [k \in Blob |-> {Gap(c, v) : v \in blobDur[d][k]}]],
        !.record = [d \in Docs |-> [live |-> Gap(c, record[d].live),
                                    dur |-> Gap(c, record[d].dur)]],
        !.prep = GapSwaps(c, prep),
        !.wkeep = {Gap(c, v) : v \in wkeep}]

Up(v, k) == IF v = NoVal THEN NoVal ELSE v + k

\* The count, every saved count and every sample of it, moved up by two.
CountShift ==
    [Snap EXCEPT
        !.count = count + 2,
        !.runCount = [h \in Procs |-> Up(runCount[h], 2)],
        !.rbefore = [h \in Procs |-> Up(rbefore[h], 2)],
        !.rpart = [h \in Procs |-> Up(rpart[h], 1)],
        !.scache = [h \in Procs |-> Up(scache[h], 2)]]

\* The abstract transaction cells, and what a reader saw of them, moved up together.
CellShift ==
    [Snap EXCEPT
        !.a = a + 1,
        !.b = b + 1,
        !.stage = IF stage = 0 THEN 0 ELSE stage + 1,
        !.ptgt = IF ptgt = 0 THEN 0 ELSE ptgt + 1,
        !.rseenA = [h \in Procs |-> Up(rseenA[h], 1)],
        !.rseenB = [h \in Procs |-> Up(rseenB[h], 1)]]

Flip(f, h) == [f EXCEPT ![h] = ~@]

\* A replica's versions nothing names, replaced by others nothing names: one not stored there if
\* the generation leaves one, otherwise one of them alone.
SpareVersions(garbage) == ((1..gen) \ NamedVersions) \ garbage
OtherGarbage(versions) ==
    LET garbage == versions \ NamedVersions
    IN IF SpareVersions(garbage) # {}
       THEN (versions \ garbage) \cup {CHOOSE v \in SpareVersions(garbage) : TRUE}
       ELSE IF Cardinality(garbage) >= 2
            THEN (versions \ garbage) \cup {CHOOSE v \in garbage : TRUE}
            ELSE versions

\* The blob replicas holding versions nothing names that another such set can replace.
GarbageReplicas(blobs) ==
    {<<d, k>> \in Docs \X Blob :
        blobs[d][k] \ NamedVersions # {} /\ OtherGarbage(blobs[d][k]) # blobs[d][k]}

Perturbations ==
    {<<<<"gap", VersionRank(c)>>, VersionGap(c)>> : c \in NamedVersions \ {0}}
    \cup {<<<<"garbageLive", (d - 1) * NBlob + k>>,
            [Snap EXCEPT !.blobLive = [blobLive EXCEPT ![d][k] = OtherGarbage(@)]]>> :
              <<d, k>> \in GarbageReplicas(blobLive)}
    \cup {<<<<"garbageDur", (d - 1) * NBlob + k>>,
            [Snap EXCEPT !.blobDur = [blobDur EXCEPT ![d][k] = OtherGarbage(@)]]>> :
              <<d, k>> \in GarbageReplicas(blobDur)}
    \cup {<<<<"count", 0>>, CountShift>>, <<<<"cells", 0>>, CellShift>>,
          <<<<"shared", 0>>, [Snap EXCEPT !.shared = shared + 1]>>}
    \cup {<<<<"cache", h>>, [Snap EXCEPT !.cache = [cache EXCEPT ![h] = @ + 1]]>> :
              h \in {q \in Procs : cache[q] # NoVal}}
    \cup {<<<<"stale", h>>, [Snap EXCEPT !.runCount = [runCount EXCEPT ![h] = @ - 1]]>> :
              h \in {q \in Procs : runCount[q] \notin {NoVal, count} /\ runCount[q] >= 1}}
    \cup {<<<<"forgotten", h>>, [Snap EXCEPT !.mygen = [mygen EXCEPT ![h] = NoVal]]>> :
              h \in {q \in Procs : mygen[q] \notin {NoVal, gen}}}
    \cup {<<<<"older", h>>, [Snap EXCEPT !.mygen = [mygen EXCEPT ![h] = gen - 1]]>> :
              h \in {q \in Procs : mygen[q] = NoVal /\ gen >= 1}}
    \cup {<<<<"intent", h>>, [Snap EXCEPT !.requestRecovery = Flip(requestRecovery, h)]>> :
              h \in {q \in Procs : pc[q] \notin InOperationStates}}
    \cup {<<<<"ready", h>>, [Snap EXCEPT !.unitReady = Flip(unitReady, h)]>> :
              h \in {q \in Procs : pc[q] \notin {"held", "told"}}}
    \cup {<<<<"intervened", h>>, [Snap EXCEPT !.unitIntervened = Flip(unitIntervened, h)]>> :
              h \in {q \in Procs : ~(pc[q] \in {"held", "told"} /\ unitReady[q]
                                       /\ requestRecovery[q])}}
    \cup IF ViewControl THEN {<<<<"fence", 0>>, [Snap EXCEPT !.fence = ~fence]>>} ELSE {}

SelectedPerturbations ==
    {p \in Perturbations : SoundnessPerturbation = "all"
                          \/ p[1][1] = SoundnessPerturbation}

-----------------------------------------------------------------------------
(* The challenge.                                                          *)

SoundnessInit ==
    /\ SpInit
    /\ B!SpInit
    /\ phase = "reset"
    /\ kind = <<"none", 0>>
    /\ target = NoTarget

NodeRemovalFocused ==
    \/ wph \in {"planRemove", "planRemoveAtDocument", "planRemoveBelowDocument",
               "nodeRen", "nodeSync", "nodeDone"}
    \/ wph \in {"none", "planned"} /\ \E holder \in Procs : W(holder) /\ pc[holder] = "open"

DirectoryCreationFocused ==
    CASE SoundnessFocus = "directory-creation-transaction" -> wph = "txBegin"
      [] SoundnessFocus = "directory-creation-append-plan" -> wph = "dirAppend"
      [] SoundnessFocus = "directory-creation-append-repair" -> wph = "dirAppendRepair"
      [] SoundnessFocus = "directory-creation-append-sync" -> wph = "dirAppendSync"
      [] SoundnessFocus = "directory-creation-create" ->
            wph \in {"dirCreate", "dirRename"}
      [] SoundnessFocus = "directory-creation-sync" ->
            wph \in {"dirSync", "dirClear", "dirClearSync"}
      [] SoundnessFocus = "directory-creation-complete" -> wph = "dirDone"
      [] SoundnessFocus = "directory-creation-recovery" ->
            wph = "none" /\ \E holder \in Procs : runPhase[holder] = "directories"
      [] OTHER ->
            (wph \in {"txBegin", "dirAppend", "dirAppendRepair", "dirAppendSync",
                      "dirCreate", "dirRename", "dirSync", "dirClear", "dirClearSync",
                      "dirDone"}
             \/ (wph = "none" /\ \E holder \in Procs : runPhase[holder] = "directories"))

SoundnessFocusEnabled ==
    CASE SoundnessFocus = "all" -> TRUE
      [] SoundnessFocus = "node-removal" -> NodeRemovalFocused
      [] OTHER -> DirectoryCreationFocused

\* The second copy is given a perturbation of the first, which stays within the scope the
\* reduced model's checks cover.
Perturb ==
    /\ phase = "reset"
    /\ WritesBounded
    /\ SoundnessFocusEnabled
    /\ \E p \in SelectedPerturbations : LoadB(p[2]) /\ kind' = p[1]
    /\ phase' \in {"challengeA", "challengeB"}
    /\ target' = NoTarget
    /\ UNCHANGED spvars

\* Ordinary quotient steps outside a focused M12 region, or a focused region to which
\* the selected perturbation family does not apply, advance both copies together.
Advance ==
    /\ phase = "reset"
    /\ WritesBounded
    /\ (~SoundnessFocusEnabled \/ SelectedPerturbations = {})
    /\ QuotientNext
    /\ LoadB(Snap')
    /\ UNCHANGED <<phase, kind, target>>

AdvanceQuiescent ==
    /\ phase = "reset"
    /\ WritesBounded
    /\ (~SoundnessFocusEnabled \/ SelectedPerturbations = {})
    /\ ~ENABLED QuotientNext
    /\ UNCHANGED pvars

\* The first copy has left that scope: nothing more is challenged from it.
OutOfScope ==
    /\ phase = "reset"
    /\ ~WritesBounded
    /\ UNCHANGED pvars

ChallengeA ==
    /\ phase = "challengeA"
    /\ QuotientNext
    /\ target' = <<CheckedView', StepFacts>>
    /\ phase' = "respondB"
    /\ UNCHANGED <<bvars, kind>>

RespondB ==
    /\ phase = "respondB"
    /\ B!QuotientNext
    /\ <<CheckedViewB', StepFactsB>> = target
    /\ phase' = "reset"
    /\ UNCHANGED <<spvars, kind, target>>

ChallengeB ==
    /\ phase = "challengeB"
    /\ B!QuotientNext
    /\ target' = <<CheckedViewB', StepFactsB>>
    /\ phase' = "respondA"
    /\ UNCHANGED <<spvars, kind>>

RespondA ==
    /\ phase = "respondA"
    /\ QuotientNext
    /\ <<CheckedView', StepFacts>> = target
    /\ phase' = "reset"
    /\ UNCHANGED <<bvars, kind, target>>

\* Neither copy can step: a challenge has nothing to answer.
Quiescent ==
    /\ phase \in {"challengeA", "challengeB"}
    /\ ~ENABLED QuotientNext
    /\ ~ENABLED B!QuotientNext
    /\ phase' = "reset"
    /\ UNCHANGED <<spvars, bvars, kind, target>>

SoundnessNext ==
    \/ Perturb \/ Advance \/ AdvanceQuiescent \/ OutOfScope \/ Quiescent
    \/ ChallengeA \/ RespondB \/ ChallengeB \/ RespondA

SoundnessSpec == SoundnessInit /\ [][SoundnessNext]_pvars

\* The M12 namespace soundness checks isolate the branches established by earlier
\* one-step publication and transaction checks.
RemovalEffectsOnly ==
    /\ \A index \in DOMAIN prep : prep[index].effect = "remove"
    /\ \A index \in DOMAIN b_prep : b_prep[index].effect = "remove"

WriteEffectsWithoutNodeRemoval ==
    /\ removeNodes = <<>>
    /\ b_removeNodes = <<>>
    /\ \A index \in DOMAIN prep : prep[index].effect = "write"
    /\ \A index \in DOMAIN b_prep : b_prep[index].effect = "write"

RemovalSoundnessScope == QuotientTypeOK /\ RemovalEffectsOnly
WriteSoundnessScope == QuotientTypeOK /\ WriteEffectsWithoutNodeRemoval

\* Challenge metadata is relevant only while a challenge is outstanding. At reset,
\* every enabled action overwrites it, so the completed challenge must not distinguish states.
ProductView ==
    IF phase = "reset"
    THEN <<CheckedView, phase, <<"none", 0>>, NoTarget>>
    ELSE <<CheckedView, phase, kind, target>>

-----------------------------------------------------------------------------
(* Properties.                                                             *)

\* Audience: internal.
\* A perturbation changes nothing the view keeps.
SameView == phase \in {"challengeA", "challengeB"} => CheckedView = CheckedViewB

\* Audience: internal.
\* Every checked state predicate depends only on the view.
FactsAgree == phase \in {"challengeA", "challengeB"} => StateFacts = StateFactsB

=============================================================================
