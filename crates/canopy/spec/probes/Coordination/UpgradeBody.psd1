# Copyright (c) Michael Grier.
# Probes of the schema upgrade's body.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'intent-not-recorded'
            Base      = 'Coordination-UpgradeBody'
            Kind      = 'INVARIANTS'
            Property  = 'AMigrationStepIsNeverUnannounced'
            Def       = 'RecordIntent'
            Old = @'
upState' = IF SafeFor(p) THEN "safe" ELSE "unsafe"
'@
            New = @'
upState' = upState
'@
        }

        @{
            Name      = 'intent-cleared-before-the-advance'
            Base      = 'Coordination-UpgradeBody'
            Kind      = 'PROPERTIES'
            Property  = 'TheIntentClearsOnlyAfterTheSchemaAdvances'
            Def       = 'UpClear'
            Old = @'
    /\ (Advanced \/ (pc[p] = "recUp" /\ Control = "interrupted-upgrade-rolled-back"))
'@
            New = @'
    /\ TRUE
'@
        }

        @{
            Name      = 'resumer-restarts-the-steps'
            Base      = 'Coordination-UpgradeBody'
            Kind      = 'PROPERTIES'
            Property  = 'UpgradeProgressNeverGoesBack'
            Def       = 'RecStart'
            Old = @'
UNCHANGED <<adm>>
'@
            New = @'
ustep' = 0
    /\ UNCHANGED <<att, areq, lease, reg, oldest, current, dfloor, upState, ulayer, utgt, ufloor, rtier>>
'@
        }

        @{
            Name      = 'upgrade-failure-leaves-no-fence'
            Base      = 'Coordination-UpgradeBody'
            Kind      = 'PROPERTIES'
            Property  = 'FailedOperationsLeaveTheCountOddAndFence'
            Def       = 'UpFails'
            Old = @'
    /\ UpSteps > 0 /\ pc[p] = "upBody"
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
'@
            New = @'
    /\ UpSteps > 0 /\ pc[p] = "upBody"
    /\ pc' = Set(pc, p, "told")
    /\ fence' = fence
'@
        }

        @{
            Name      = 'completed-upgrade-reported-refused'
            Base      = 'Coordination-UpgradeBody'
            Kind      = 'PROPERTIES'
            Property  = 'ABodyEndsServedOrRefused'
            Def       = 'Unlock'
            Old = @'
areq' = Set(areq, p, IF Completed(p) THEN "served" ELSE "refused")
'@
            New = @'
areq' = Set(areq, p, "refused")
'@
        }

        @{
            Name      = 'advance-forgets-the-oldest-schema'
            Base      = 'Coordination-UpgradeBodyUnsafe'
            Kind      = 'PROPERTIES'
            Property  = 'TheOldestReaderSchemaMovesWithTheSchema'
            Def       = 'UpAdvance'
            Old = @'
oldest' = OldestAfterCompleting
'@
            New = @'
oldest' = oldest
'@
        }

        @{
            Name      = 'intent-recorded-before-exclusion'
            Base      = 'Coordination-UpgradeBody'
            Kind      = 'PROPERTIES'
            Property  = 'UpgradeEffectsAreMadeUnderExclusionAndTheCount'
            Def       = 'RecordIntent'
            Old = @'
/\ UpSteps > 0 /\ att[p] = "uBody" /\ pc[p] = "open"
'@
            New = @'
/\ UpSteps > 0 /\ att[p] = "uBody" /\ pc[p] = "locked"
'@
        }

        @{
            Name      = 'intent-recorded-outside-the-count'
            Base      = 'Coordination-UpgradeBody'
            Kind      = 'PROPERTIES'
            Property  = 'UpgradeEffectsAreMadeUnderExclusionAndTheCount'
            Def       = 'RecordIntent'
            Old = @'
/\ UpSteps > 0 /\ att[p] = "uBody" /\ pc[p] = "open"
'@
            New = @'
/\ UpSteps > 0 /\ att[p] = "uBody" /\ pc[p] = "held"
'@
        }

        @{
            Name      = 'a-failed-upgrader-resumes-its-own-upgrade'
            Base      = 'Coordination-UpgradeBody'
            Kind      = 'PROPERTIES'
            Property  = 'AResumptionIsMadeOnlyByAHandleThatSpeaksTheTarget'
            Def       = 'RecStart'
            Old = @'
    /\ (Txn \/ UnloggedRecovery) /\ pc[p] = "told" /\ CanWrite(p) /\ att[p] # "uBody"
'@
            New = @'
    /\ (Txn \/ UnloggedRecovery) /\ pc[p] = "told" /\ CanWrite(p)
'@
        }
    )
}
