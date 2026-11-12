# Copyright (c) Michael Grier.
# Probes of the admission gate and the attachment tiers.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'admit-ignores-rules'
            Base      = 'Coordination-Witness-AReaderIsRefusedDuringAnUnsafeUpgrade'
            Kind      = 'INVARIANTS'
            Property  = 'AttachedHandlesStayAdmissible'
            Def       = 'Admit'
            Old = @'
    /\ att[p] = "tiers"
    /\ IF Admissible(p)
'@
            New = @'
    /\ att[p] = "tiers"
    /\ IF TRUE
'@
        }

        @{
            Name      = 'gate-x-ignores-shared'
            Base      = 'Coordination-Witness-AnUpgraderWaitsBehindAnAdmission'
            Kind      = 'INVARIANTS'
            Property  = 'AnExclusiveGateHolderCoexistsWithNoOtherAdmission'
            Def       = 'TakeGateX'
            Old = @'
    /\ ~GateSHeld /\ ~GateXHeld
'@
            New = @'
    /\ ~GateXHeld
'@
        }

        @{
            Name      = 'gate-s-ignores-x'
            Base      = 'Coordination-Witness-AnUpgraderWaitsBehindAnAdmission'
            Kind      = 'INVARIANTS'
            Property  = 'AnExclusiveGateHolderCoexistsWithNoOtherAdmission'
            Def       = 'TakeGateS'
            Old = @'
    /\ att[p] = "wGate" /\ ~GateXHeld
'@
            New = @'
    /\ att[p] = "wGate"
'@
        }

        @{
            Name      = 'writer-tier-s-ignores-x'
            Base      = 'Coordination-Witness-AWriterWaitsForTheWriterTierBehindAnUpgrade'
            Kind      = 'INVARIANTS'
            Property  = 'TierHoldersAreCompatible'
            Def       = 'TakeWriterS'
            Old = @'
    /\ ~WriterXHeld
'@
            New = ''
        }

        @{
            Name      = 'first-tier-ignores-shared'
            Base      = 'Coordination-Witness-AnUnsafeUpgradeCompletes'
            Kind      = 'INVARIANTS'
            Property  = 'TierHoldersAreCompatible'
            Def       = 'TakeFirstTier'
            Old = @'
THEN /\ ~ReaderSHeld /\ ~ReaderXHeld
'@
            New = @'
THEN /\ ~ReaderXHeld
'@
        }

        @{
            Name      = 'exit-keeps-attachment'
            Base      = 'Coordination-Witness-AReaderIsAdmitted'
            Kind      = 'INVARIANTS'
            Property  = 'AttachmentMatchesTheProcess'
            Def       = 'Exit'
            Old = @'
          ELSE /\ pc' = Set(pc, p, "exited")
               /\ att' = Set(att, p, "off")
'@
            New = @'
          ELSE /\ pc' = Set(pc, p, "exited")
               /\ att' = att
'@
        }

        @{
            Name      = 'die-keeps-attachment'
            Base      = 'Coordination-Witness-AReaderIsRefusedDuringAnUnsafeUpgrade'
            Kind      = 'INVARIANTS'
            Property  = 'AttachmentMatchesTheProcess'
            Def       = 'Die'
            Old = @'
    /\ deaths' = deaths + 1
    /\ att' = Set(att, p, "off")
'@
            New = @'
    /\ deaths' = deaths + 1
    /\ att' = att
'@
        }

        @{
            Name      = 'give-up-keeps-attachment'
            Base      = 'Coordination-Witness-AnAttachingHandleGivesUp'
            Kind      = 'INVARIANTS'
            Property  = 'AttachmentMatchesTheProcess'
            Def       = 'GiveUp'
            Old = @'
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p,
                 IF pc[p] # "converting" THEN "exited"
'@
            New = @'
    /\ att' = att
    /\ pc' = Set(pc, p,
                 IF pc[p] # "converting" THEN "exited"
'@
        }

        @{
            Name      = 'refusal-keeps-attachment'
            Base      = 'Coordination-Witness-AReaderIsRefused'
            Kind      = 'INVARIANTS'
            Property  = 'AttachmentMatchesTheProcess'
            Def       = 'Admit'
            Old = @'
               /\ areq' = IF RegistryOn THEN areq ELSE Set(areq, p, "served")
          ELSE /\ lph[p] = "none"
               /\ att' = Set(att, p, "off")
'@
            New = @'
               /\ areq' = IF RegistryOn THEN areq ELSE Set(areq, p, "served")
          ELSE /\ lph[p] = "none"
               /\ att' = att
'@
        }

        @{
            Name      = 'upgrade-moved-keeps-attachment'
            Base      = 'Coordination-Witness-AnUpgraderGivesUpAtTheRecheck'
            Kind      = 'INVARIANTS'
            Property  = 'AttachmentMatchesTheProcess'
            Def       = 'UpgradeMoved'
            Old = @'
    /\ att' = Set(att, p, "off")
    /\ pc' = Set(pc, p, IF pc[p] = "convertingX" THEN "held" ELSE "detached")
'@
            New = @'
    /\ att' = att
    /\ pc' = Set(pc, p, IF pc[p] = "convertingX" THEN "held" ELSE "detached")
'@
        }

        @{
            Name      = 'begin-attach-not-pending'
            Base      = 'Coordination-Witness-AReaderIsAdmitted'
            Kind      = 'INVARIANTS'
            Property  = 'AttachRequestStateMatchesProcess'
            Def       = 'BeginAttach'
            Old = @'
    /\ att' = Set(att, p, "wGate")
    /\ areq' = Set(areq, p, "pending")
'@
            New = @'
    /\ att' = Set(att, p, "wGate")
    /\ areq' = areq
'@
        }

        @{
            Name      = 'end-upgrade-no-outcome'
            Base      = 'Coordination-Witness-ASafeUpgradeCompletes'
            Kind      = 'INVARIANTS'
            Property  = 'AttachRequestStateMatchesProcess'
            Def       = 'EndUpgrade'
            Old = @'
    /\ areq' = Set(areq, p, "served")
    /\ UNCHANGED rest
'@
            New = @'
    /\ areq' = areq
    /\ UNCHANGED rest
'@
        }

        @{
            Name      = 'reader-requests-exclusion'
            Base      = 'Coordination-Witness-ReadersAttachWhileASafeUpgradeRuns'
            Kind      = 'INVARIANTS'
            Property  = 'ExclusionIsHeldOnlyByAttachedWriters'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Request'
            Old = @'
Request(p) ==
    /\ CanWrite(p)
'@
            New = @'
Request(p) ==
    /\ TRUE
'@
        }

        @{
            Name      = 'recheck-skipped'
            Base      = 'Coordination-Witness-AnUpgraderGivesUpAtTheRecheck'
            Kind      = 'PROPERTIES'
            Property  = 'AnUpgradeBeginsFromTheSchemaItExpects'
            Def       = 'BeginUpgrade'
            Old = @'
/\ current[UpL(p)] = SpeaksOf[p][UpL(p)] /\ upState = "ordinary"
'@
            New = @'
/\ TRUE
'@
        }

        @{
            Name      = 'unsafe-end-forgets-oldest'
            Base      = 'Coordination-Witness-AnUnsafeUpgradeCompletes'
            Kind      = 'PROPERTIES'
            Property  = 'SchemasMoveOnlyWhenAnUpgradeCompletes'
            Def       = 'EndUpgrade'
            Old = @'
oldest' = OldestAfterCompleting
'@
            New = @'
oldest' = oldest
'@
        }

        @{
            Name      = 'end-upgrade-leaves-state'
            Base      = 'Coordination-Witness-ASafeUpgradeCompletes'
            Kind      = 'PROPERTIES'
            Property  = 'SchemasMoveOnlyWhenAnUpgradeCompletes'
            Def       = 'EndUpgrade'
            Old = @'
    /\ (pc[p] = "convertingX" \/ lph[p] = "none")
    /\ upState' = "ordinary"
'@
            New = @'
    /\ (pc[p] = "convertingX" \/ lph[p] = "none")
    /\ upState' = upState
'@
        }

        @{
            Name      = 'reader-rule-ignores-progress'
            Base      = 'Coordination-Witness-AReaderIsRefusedDuringAnUnsafeUpgrade'
            Kind      = 'PROPERTIES'
            Property  = 'Witness_AReaderIsRefusedDuringAnUnsafeUpgrade'
            Expect    = 'passes'
            Def       = 'ReaderOK'
            Old = @'
THEN upState = "safe"
            /\ ((oldest[l] <= s /\ s <= current[l]) \/ s = utgt)
'@
            New = @'
THEN ((oldest[l] <= s /\ s <= current[l]) \/ s = utgt)
'@
        }

        @{
            Name      = 'writer-rule-ignores-progress'
            Base      = 'Coordination-Witness-AWriterIsAdmittedToResumeAnInterruptedUpgrade'
            Kind      = 'PROPERTIES'
            Property  = 'Witness_AWriterIsAdmittedToResumeAnInterruptedUpgrade'
            Expect    = 'passes'
            Def       = 'WriterOK'
            Old = @'
IF InProgress(l)
       THEN s = utgt
            \/ (Control = "interrupted-upgrade-admits-source-writers"
                /\ s = current[l])
       ELSE s = current[l]
'@
            New = @'
s = current[l]
'@
        }

        @{
            Name      = 'iso-order-keeps-tiers'
            Base      = 'Coordination-Control-UpgraderKeepsTiers'
            Kind      = 'INVARIANTS'
            Property  = 'NothingIsAcquiredAgainstTheOrder'
        }

        @{
            Name      = 'iso-gate-keeps-tiers'
            Base      = 'Coordination-Control-UpgraderKeepsTiers'
            Kind      = 'INVARIANTS'
            Property  = 'NoHandleWaitsForTheGateWhileHoldingATier'
        }

        @{
            Name      = 'iso-order-unsafe-writer-first'
            Base      = 'Coordination-Control-UnsafeWriterTierFirst'
            Kind      = 'INVARIANTS'
            Property  = 'NothingIsAcquiredAgainstTheOrder'
        }

        @{
            Name      = 'iso-order-holds-exclusion'
            Base      = 'Coordination-Control-UpgraderHoldsExclusion'
            Kind      = 'INVARIANTS'
            Property  = 'NothingIsAcquiredAgainstTheOrder'
        }

        @{
            Name      = 'equiv-skip-first'
            Base      = @('Coordination-AttachmentUnsafe', 'Coordination-AttachmentTwoUpgraders', 'Coordination-Attachment')
            Expect    = 'passes'
            Def       = 'TakeReaderS'
            Old = @'
    /\ att[p] = "hasGate" /\ ~ReaderXHeld
'@
            New = @'
    /\ att[p] = "hasGate"
'@
        }

        @{
            Name      = 'equiv-second-tier-ignores-writers'
            Base      = @('Coordination-AttachmentUnsafe', 'Coordination-AttachmentTwoUpgraders', 'Coordination-Attachment')
            Expect    = 'passes'
            Def       = 'TakeSecondTier'
            Old = @'
    /\ \/ att[p] = "uRd" /\ ~WriterSHeld /\ ~WriterXHeld
'@
            New = @'
    /\ \/ att[p] = "uRd" /\ ~WriterXHeld
'@
        }
    )
}
