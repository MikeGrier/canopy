# Copyright (c) Michael Grier.
# Probes of transactions and the commit log.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'prepare-swap-set-empty'
            Base      = 'Coordination-Txn'
            Kind      = 'INVARIANTS'
            Property  = 'PreparedRecordCarriesItsSwapSet'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Prepare'
            Old = @'
    /\ ptgt' = IF stage # 0 THEN stage ELSE Nx(a)
'@
            New = @'
    /\ ptgt' = 0
'@
        }

        @{
            Name      = 'end-keeps-the-swap-set'
            Base      = 'Coordination-Txn'
            Kind      = 'INVARIANTS'
            Property  = 'PreparedRecordCarriesItsSwapSet'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'End'
            Old = @'
    /\ log' = "ended"
    /\ ptgt' = 0
'@
            New = @'
    /\ log' = "ended"
    /\ ptgt' = ptgt
'@
        }

        @{
            Name      = 'abort-keeps-staging'
            Base      = 'Coordination-Txn'
            Kind      = 'INVARIANTS'
            Property  = 'StagingIsAlwaysAnnouncedByTheLog'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Abort'
            Old = @'
    /\ stage' = 0
    /\ log' = "empty"
'@
            New = @'
    /\ stage' = stage
    /\ log' = "empty"
'@
        }

        @{
            Name      = 'abort-keeps-the-log'
            Base      = 'Coordination-Txn'
            Kind      = 'INVARIANTS'
            Property  = 'AFinishedOperationLeavesTheStoreSettled'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Abort'
            Old = @'
    /\ log' = "empty"
    /\ ptgt' = 0
    /\ pc' = Set(pc, p, "wrote")
'@
            New = @'
    /\ log' = log
    /\ ptgt' = 0
    /\ pc' = Set(pc, p, "wrote")
'@
        }

        @{
            Name      = 'sweep-skipped'
            Base      = 'Coordination-Txn'
            Kind      = 'INVARIANTS'
            Property  = 'AFinishedOperationLeavesTheStoreSettled'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Sweep'
            Old = @'
    /\ Logs /\ pc[p] = "ended"
    /\ stage' = 0
'@
            New = @'
    /\ Logs /\ pc[p] = "ended"
    /\ stage' = stage
'@
        }

        @{
            Name      = 'begin-before-the-opening-raise'
            Base      = 'Coordination-Txn'
            Kind      = 'INVARIANTS'
            Property  = 'CountIsOddWhileAnOperationIsUnresolved'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Begin'
            Old = @'
/\ Logs /\ pc[p] = "open"
'@
            New = @'
/\ Logs /\ pc[p] = "held"
'@
        }

        @{
            Name      = 'request-resets-the-log'
            Base      = 'Coordination-Txn'
            Kind      = 'PROPERTIES'
            Property  = 'TheLogChangesOnlyUnderExclusion'
            Overrides = @{ MaxRequests = '2' }
            Def       = 'Request'
            Old = @'
    /\ requests' = Tick(requests)
    /\ UNCHANGED <<adm, eligible, coord, store, handle, reader, diedIn, deaths,
                   crashes, reads>>
'@
            New = @'
    /\ requests' = Tick(requests)
    /\ log' = "empty"
    /\ UNCHANGED <<adm, eligible, coord, a, b, shared, stage, ptgt, handle, reader,
                   diedIn, deaths, crashes, reads>>
'@
        }

        @{
            Name      = 'abort-writes-the-namespace'
            Base      = 'Coordination-Txn'
            Kind      = 'PROPERTIES'
            Property  = 'AFailureBeforePrepareLeavesNothingBehind'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'Abort'
            Old = @'
    /\ UNCHANGED <<adm, req, eligible, coord, a, b, mygen, reader, diedIn, counts>>
'@
            New = @'
    /\ a' = IF stage # 0 THEN stage ELSE a
    /\ UNCHANGED <<adm, req, eligible, coord, b, mygen, reader, diedIn, counts>>
'@
        }

        @{
            Name      = 'failure-leaves-no-fence'
            Base      = 'Coordination-Txn'
            Kind      = 'PROPERTIES'
            Property  = 'FailedOperationsLeaveTheCountOddAndFence'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'FailUnresolved'
            Old = @'
    /\ pc[p] \in FailStates
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
'@
            New = @'
    /\ pc[p] \in FailStates
    /\ pc' = Set(pc, p, "told")
    /\ fence' = fence
'@
        }

        @{
            Name      = 'prepare-failure-leaves-no-fence'
            Base      = 'Coordination-Txn'
            Kind      = 'PROPERTIES'
            Property  = 'FailedOperationsLeaveTheCountOddAndFence'
            Overrides = @{ MaxRequests = '1' }
            Def       = 'PrepareFails'
            Old = @'
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, a, b,
'@
            New = @'
    /\ pc' = Set(pc, p, "told")
    /\ fence' = fence
    /\ UNCHANGED <<adm, req, eligible, lockHolder, owner, trusted, count, gen, a, b,
'@
        }

        @{
            Name      = 'iso-apply-before-prepare-state'
            Base      = 'Coordination-Control-ApplyBeforePrepare'
            Kind      = 'INVARIANTS'
            Property  = 'TransactionStateMatchesTheLog'
        }

        @{
            Name      = 'iso-prepare-before-ingest-state'
            Base      = 'Coordination-Control-PrepareBeforeIngest'
            Kind      = 'INVARIANTS'
            Property  = 'TransactionStateMatchesTheLog'
        }

        @{
            Name      = 'iso-failure-rolled-back-count'
            Base      = 'Coordination-Control-FailureAfterPrepareIsRolledBack'
            Kind      = 'INVARIANTS'
            Property  = 'CountIsOddWhileAnOperationIsUnresolved'
        }

        @{
            Name      = 'iso-ignore-restart-buries-work'
            Base      = 'Coordination-Control-IgnoreRestart'
            Kind      = 'PROPERTIES'
            Property  = 'ANewTransactionStartsFromASettledStore'
            Overrides = @{ MaxRequests = '2'; Txn = 'TRUE' }
        }

        @{
            Name      = 'iso-reset-while-recovery-required-staging'
            Base      = 'Coordination-Control-ResetWhileRecoveryRequired'
            Kind      = 'INVARIANTS'
            Property  = 'StagingIsAlwaysAnnouncedByTheLog'
        }
    )
}
