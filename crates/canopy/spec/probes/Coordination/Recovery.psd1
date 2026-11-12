# Copyright (c) Michael Grier.
# Probes of recovery and its discovery.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'recovery-sweep-skipped'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'RecoveryPublishesTheCountOnlyOnceTheStoreIsSettled'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'RecSweep'
            Old = @'
    /\ pc[p] = "recSweep"
    /\ stage' = 0
'@
            New = @'
    /\ pc[p] = "recSweep"
    /\ stage' = stage
'@
        }

        @{
            Name      = 'recovery-end-record-skipped'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'RecoveryPublishesTheCountOnlyOnceTheStoreIsSettled'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'RecEnd'
            Old = @'
          /\ log' = "ended"
'@
            New = @'
          /\ log' = log
'@
        }

        @{
            Name      = 'recovery-second-swap-skipped'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'AnEndRecordFollowsEverySwap'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'RecSwap2'
            Old = @'
    /\ b' = ptgt
'@
            New = @'
    /\ b' = b
'@
        }

        @{
            Name      = 'recovery-skips-the-swaps'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'AnEndRecordFollowsEverySwap'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'RecStart'
            Old = @'
                    THEN "recSwap1"
'@
            New = @'
                    THEN "recEnd"
'@
        }

        @{
            Name      = 'recovery-count-left-odd'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'TheFenceIsClearedOnlyByAFinishedRecovery'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'RecCount'
            Old = @'
    /\ count' = RecoveredCount
'@
            New = @'
    /\ count' = Visible
'@
        }

        @{
            Name      = 'recovery-trust-not-restored'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'TheFenceIsClearedOnlyByAFinishedRecovery'
            Overrides = @{ MaxRequests = '2' }
            Def       = 'RecCount'
            Old = @'
    /\ trusted' = TRUE
'@
            New = @'
    /\ trusted' = trusted
'@
        }

        @{
            Name      = 'recovery-fence-never-cleared'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'RecoveryClearsTheFenceAtItsEnd'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'RecClear'
            Old = @'
    /\ fence' = FALSE
'@
            New = @'
    /\ fence' = fence
'@
        }

        @{
            Name      = 'recovery-stop-clears-the-fence'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'TheFenceIsClearedOnlyByAFinishedRecovery'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'RecStop'
            Old = @'
    /\ pc' = Set(pc, p, "told")
    /\ UNCHANGED <<adm, req, eligible, coord, store, handle, reader, diedIn, counts>>
'@
            New = @'
    /\ pc' = Set(pc, p, "told")
    /\ fence' = FALSE
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, store,
                   handle, reader, diedIn, counts>>
'@
        }

        @{
            Name      = 'recovery-resets-the-log-before-the-sweep'
            Base      = 'Coordination-Recovery'
            Kind      = 'INVARIANTS'
            Property  = 'StagingIsAlwaysAnnouncedByTheLog'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'RecEnd'
            Old = @'
       \/ /\ log = "begun"
          /\ stage' = 0
          /\ pc' = Set(pc, p, "recReset")
'@
            New = @'
       \/ /\ log = "begun"
          /\ stage' = stage
          /\ pc' = Set(pc, p, "recReset")
'@
        }

        @{
            Name      = 'discovery-reads-nothing'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'DiscoveryReportsAbandonedWorkWhenNobodyHoldsExclusion'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Discover'
            Old = @'
    /\ disc' = Set(disc, p, IF Reported THEN "required" ELSE "notRequired")
'@
            New = @'
    /\ disc' = Set(disc, p, "notRequired")
'@
        }

        @{
            Name      = 'discovery-always-required'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'DiscoveryReportsRequiredOnlyWithCause'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Discover'
            Old = @'
    /\ disc' = Set(disc, p, IF Reported THEN "required" ELSE "notRequired")
'@
            New = @'
    /\ disc' = Set(disc, p, "required")
'@
        }

        @{
            Name      = 'discovery-ignores-a-dead-holders-record'
            Base      = 'Coordination-Recovery'
            Kind      = 'PROPERTIES'
            Property  = 'DiscoveryReportsAbandonedWorkWhenNobodyHoldsExclusion'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Reported'
            Old = @'
    \/ owner # None
       /\ (lockHolder = None \/ Control = "discovery-trusts-the-ownership-record")
'@
            New = @'
    \/ FALSE
'@
        }
    )
}
