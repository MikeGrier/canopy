\* Copyright (c) Michael Grier.
\* Split from CoordinationProperties.tla at 18b499a.
------------------- MODULE CoordinationUpgradeProperties --------------------
EXTENDS CoordinationTransactionProperties

-----------------------------------------------------------------------------
(* The upgrade body.                                                       *)

\* Audience: internal.
\* An upgrader finds writer exclusion free: it holds the writer tier
\* exclusively, so no handle that may write is attached to hold or want it.
AnUpgraderFindsExclusionFree ==
    [][\A p \in Procs :
          (UpSteps > 0 /\ att[p] = "uTiers" /\ att'[p] = "uBody") =>
              lockHolder = None]_vars

\* Audience: internal.
\* A migration step is never durable without the intent that says to finish it,
\* so an upgrader that dies after its first step leaves recovery something to
\* resume.
AMigrationStepIsNeverUnannounced == ustep > 0 => upState # "ordinary"

\* Audience: internal.
\* Progress only grows while the intent stands.
UpgradeProgressNeverGoesBack ==
    [][(upState # "ordinary" /\ upState' # "ordinary") => ustep' >= ustep]_vars

\* Audience: client. A layer's schema version advances only after every migration step is done.
\* Shows through: the schema a store reports.
\* The layer's schema advances only after every migration step.
TheVersionAdvancesOnlyAfterEveryStep ==
    [][(UpSteps > 0 /\ current' # current) =>
          (upState # "ordinary" /\ ustep = UpSteps)]_vars

\* Audience: client. An upgrade unsafe for readers stops admitting the older readers, and one safe
\* for them does not. Shows through: the outcome of attach.
\* The oldest reader schema moves with the schema, and only for an upgrade that
\* is unsafe for readers.
TheOldestReaderSchemaMovesWithTheSchema ==
    [][UpSteps > 0 =>
          ((oldest' # oldest) <=> (current' # current /\ upState = "unsafe"))]_vars

\* Audience: client. An interrupted upgrade is resumed to completion and never undone.
\* Shows through: the schema a store reports after recovery.
\* The intent clears only once the schema has advanced, so an interrupted upgrade
\* is resumed to completion and never undone.
TheIntentClearsOnlyAfterTheSchemaAdvances ==
    [][(UpSteps > 0 /\ upState # "ordinary" /\ upState' = "ordinary") =>
          Advanced]_vars

\* Audience: internal.
\* The upgrade's effects are made under exclusion, by its upgrader in its body or
\* by a recoverer, and inside the count's bracket.
UpgradeEffectsAreMadeUnderExclusionAndTheCount ==
    [][(UpSteps > 0 /\ (upState' # upState \/ ustep' # ustep \/ current' # current))
          => /\ Odd(Visible)
             /\ \E p \in Procs :
                   lockHolder = p /\ pc[p] \in {"open", "upBody", "recUp"}]_vars

\* Audience: client. Only a handle that speaks the target schema resumes an interrupted upgrade.
\* Shows through: the outcome of recovery.
\* An interrupted upgrade is resumed only by a handle that speaks the target
\* schema.
AResumptionIsMadeOnlyByAHandleThatSpeaksTheTarget ==
    [][\A p \in Procs :
          (pc[p] = "told" /\ pc'[p] = "recUp") =>
              SpeaksOf[p][ulayer] = utgt]_vars

\* Audience: client. While an upgrade is in progress no handle that speaks only the source schema
\* writes. Shows through: writes from a handle that is too old.
\* While an upgrade's intent stands, whoever holds exclusion is its upgrader or
\* speaks the target schema, so no handle that speaks only the source schema
\* writes.
NoHandleThatSpeaksOnlyTheSourceHoldsExclusionDuringAnUpgrade ==
    IntentStands =>
        \A p \in Procs :
            lockHolder = p =>
                (att[p] = "uBody" \/ SpeaksOf[p][ulayer] = utgt)

\* Audience: client. An upgrade whose body completes is served; one that does not is refused and
\* leaves its handle detached. Shows through: the outcome of an upgrade.
\* A body that completes serves its upgrade and one that does not refuses it and
\* leaves its handle detached.
ABodyEndsServedOrRefused ==
    [][\A p \in Procs :
          (UpSteps > 0 /\ att[p] = "uBody" /\ att'[p] = "off"
           /\ pc'[p] # "dead") =>
              IF Completed(p) THEN pc'[p] = "exited" /\ areq'[p] = "served"
                              ELSE pc'[p] = "detached" /\ areq'[p] = "refused"]_vars

-----------------------------------------------------------------------------
(* Cumulative reader compatibility.                                        *)

\* Audience: client. An upgrade is safe for readers exactly when the oldest reader schema the
\* store admits can read what it leaves. Shows through: the outcome of an upgrade and the readers
\* admitted after it.
\* An upgrade is judged against the oldest reader schema the store still admits:
\* it is safe for readers exactly when that schema can read what it leaves, and
\* every schema above it can then read it too.
AnUpgradeIsJudgedAgainstTheOldestSchema ==
    [][(ulayer = 0 /\ ulayer' # 0) =>
          ((upState' = "safe") <=> (ufloor' <= oldest[ulayer']))]_vars

\* Audience: client. An upgrade of one layer changes nothing in another. Shows through: the
\* schemas a store reports.
\* An upgrade of one layer changes nothing in another: neither its schemas nor the
\* floor of its data.
AnUpgradeOfOneLayerLeavesTheOthersAlone ==
    [][\A l \in Layers :
          l # ulayer => /\ current'[l] = current[l]
                        /\ oldest'[l] = oldest[l]
                        /\ dfloor'[l] = dfloor[l]]_vars

\* Audience: client. Every reader schema the store admits can read its data. Shows through: attach
\* and reads.
\* The oldest reader schema the store admits never falls below what the data in a
\* layer requires, so every schema it admits can read the data.
OldestCoversTheDataFloor == \A l \in Layers : dfloor[l] <= oldest[l]

\* Audience: client. While one layer upgrades, the admission rules of every other layer are
\* unchanged. Shows through: the outcome of attach.
\* Whatever upgrade is in progress, the rules of every other layer are its ordinary
\* ones: a reader needs a schema from the oldest to the current, and a writer the
\* current.
TheOtherLayersKeepTheirOrdinaryRules ==
    \A p \in Procs, l \in Layers :
        l # ulayer =>
            LET s == SpeaksOf[p][l] IN
            /\ ReaderOK(l, s) <=> (oldest[l] <= s /\ s <= current[l])
            /\ WriterOK(l, s) <=> s = current[l]

\* Audience: client. An attached handle can read the data in every layer, whatever completes after
\* it attached. Shows through: reads.
\* An attached handle speaks a schema that can read the data in every layer,
\* whatever has completed since it attached.
AnAttachedHandleCanReadTheData ==
    \A p \in Procs :
        att[p] = "attached" => \A l \in Layers : dfloor[l] <= SpeaksOf[p][l]

=============================================================================
