# Copyright (c) Michael Grier.
# Probes of the modification count, the fence, the generation and reads.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'open-without-raise'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'CountIsOddWhileAnOperationIsUnresolved'
            Def       = 'EndOpen'
            Old = @'
    /\ pc[p] = "opening"
    /\ count' = Nx(count)
'@
            New = @'
    /\ pc[p] = "opening"
    /\ count' = count
'@
        }

        @{
            Name      = 'close-without-raise'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'NormalAcquireFindsAHealthyStore'
            Def       = 'EndClose'
            Old = @'
    /\ pc[p] = "closing"
    /\ count' = Nx(count)
'@
            New = @'
    /\ pc[p] = "closing"
    /\ count' = count
'@
        }

        @{
            Name      = 'effect-before-opening'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'CountIsOddWhileAnOperationIsUnresolved'
            Def       = 'EffectFrom'
            Old = @'
    ELSE {"open"}
'@
            New = @'
    ELSE {"open", "held"}
'@
        }

        @{
            Name      = 'open-fails-raises'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'CountIsRaisedOnlyByItsHolder'
            Def       = 'OpenFails'
            Old = @'
    /\ pc' = Set(pc, p, "closed")
    /\ UNCHANGED <<req, eligible, coord, store, handle, reader, diedIn, counts>>
'@
            New = @'
    /\ pc' = Set(pc, p, "closed")
    /\ count' = Nx(count)
    /\ UNCHANGED <<req, eligible, lockHolder, owner, fence, trusted, gen, store, handle, reader, diedIn, counts>>
'@
        }

        @{
            Name      = 'close-fails-unfenced'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'FailedOperationsLeaveTheCountOddAndFence'
            Def       = 'CloseFails'
            Old = @'
    /\ pc[p] = "closing"
    /\ pc' = Set(pc, p, "told")
    /\ fence' = TRUE
'@
            New = @'
    /\ pc[p] = "closing"
    /\ pc' = Set(pc, p, "told")
    /\ fence' = fence
'@
        }

        @{
            Name      = 'fail-unresolved-unfenced'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'UnresolvedWorkIsNeverInvisible'
            Def       = 'FailUnresolved'
            Old = @'
fence' = TRUE
'@
            New = @'
fence' = fence
'@
        }

        @{
            Name      = 'rollback-incomplete'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'UnresolvedWorkIsNeverInvisible'
            Def       = 'Rollback'
            Old = @'
    /\ a' = b
    /\ shared' = Pv(shared)
    /\ cache' = Set(cache, p, Pv(shared))
    /\ UNCHANGED <<req, eligible, coord, b, mygen, reader, diedIn, counts>>
'@
            New = @'
    /\ shared' = Pv(shared)
    /\ cache' = Set(cache, p, Pv(shared))
    /\ UNCHANGED <<req, eligible, coord, a, b, mygen, reader, diedIn, counts>>
'@
        }

        @{
            Name      = 'generation-not-advanced'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'GenerationAdvancesOnEveryAcquisitionAndOnlyThen'
            Def       = 'Record'
            Old = @'
    /\ gen' = Nx(gen)
    /\ mygen' = Set(mygen, p, Nx(gen))
'@
            New = @'
    /\ gen' = gen
    /\ mygen' = Set(mygen, p, gen)
'@
        }

        @{
            Name      = 'effect-not-in-holder-cache'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'GenerationMatchImpliesAnAccurateCache'
            Def       = 'FirstEffect'
            Old = @'
    /\ cache' = Set(cache, p, Nx(shared))
'@
            New = @'
    /\ cache' = cache
'@
        }

        @{
            Name      = 'death-evens-the-count'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'DeathLeavesTheFilesystemAsItWas'
            Def       = 'Die'
            Old = @'
UNCHANGED <<count>>
'@
            New = @'
count' = IF Odd(count) THEN Nx(count) ELSE count
    /\ UNCHANGED <<>>
'@
        }

        @{
            Name      = 'classify-ignores-parity'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'SettledReadSawNoPartOfAnOperation'
            Def       = 'Undisturbed'
            Old = @'
Undisturbed(x, y) == x = y /\ Even(x)
'@
            New = @'
Undisturbed(x, y) == x = y
'@
        }

        @{
            Name      = 'classify-ignores-movement'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'SettledReadSawNoPartOfAnOperation'
            Def       = 'Undisturbed'
            Old = @'
Undisturbed(x, y) == x = y /\ Even(x)
'@
            New = @'
Undisturbed(x, y) == Even(x) /\ Even(y)
'@
        }

        @{
            Name      = 'publication-keeps-sample-cache'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'ACachedSampleIsNeverStale'
            Def       = 'Cleared'
            Old = @'
Cleared == IF Control = "late-invalidation" THEN scache
           ELSE [q \in Procs |-> NoVal]
'@
            New = @'
Cleared == scache
'@
        }

        @{
            Name      = 'untrusted-reads-as-stored'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'CountIsOddWhileAnOperationIsUnresolved'
            Def       = 'Visible'
            Old = @'
    IF trusted \/ Control = "read-untrusted-as-stored"
'@
            New = @'
    IF TRUE
'@
        }

        @{
            Name      = 'release-mid-operation'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'NormalAcquireFindsAHealthyStore'
            Def       = 'Release'
            Old = @'
    /\ pc[p] \in {"held", "closed", "told"}
'@
            New = @'
    /\ pc[p] \in {"held", "closed", "told", "wrote"}
'@
        }

        @{
            Name      = 'inner-raises-on-entry'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'CountIsOddWhileAnOperationIsUnresolved'
            Def       = 'EnterInner'
            Old = @'
UNCHANGED <<coord, reader>>
'@
            New = @'
count' = Nx(count)
    /\ scache' = Cleared
    /\ UNCHANGED <<lockHolder, owner, fence, trusted, gen, rphase, rbefore, rseenA, rseenB, rpart, disc>>
'@
        }

        @{
            Name      = 'iso-open-fails-raises'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'FailedOpeningPublicationHasNoEffect'
            Def       = 'OpenFails'
            Old = @'
    /\ pc' = Set(pc, p, "closed")
    /\ UNCHANGED <<req, eligible, coord, store, handle, reader, diedIn, counts>>
'@
            New = @'
    /\ pc' = Set(pc, p, "closed")
    /\ count' = Nx(count)
    /\ UNCHANGED <<req, eligible, lockHolder, owner, fence, trusted, gen, store, handle, reader, diedIn, counts>>
'@
        }

        @{
            Name      = 'iso-rollback-incomplete'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'AFinishedOperationLeavesTheStoreSettled'
            Def       = 'Rollback'
            Old = @'
    /\ a' = b
    /\ shared' = Pv(shared)
    /\ cache' = Set(cache, p, Pv(shared))
    /\ UNCHANGED <<req, eligible, coord, b, mygen, reader, diedIn, counts>>
'@
            New = @'
    /\ shared' = Pv(shared)
    /\ cache' = Set(cache, p, Pv(shared))
    /\ UNCHANGED <<req, eligible, coord, a, b, mygen, reader, diedIn, counts>>
'@
        }

        @{
            Name      = 'iso-effect-not-in-holder-cache'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'AHolderCacheIsAccurate'
            Def       = 'FirstEffect'
            Old = @'
    /\ cache' = Set(cache, p, Nx(shared))
'@
            New = @'
    /\ cache' = cache
'@
        }

        @{
            Name      = 'iso-generation-not-advanced'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'AHolderCacheIsAccurate'
            Def       = 'Record'
            Old = @'
    /\ gen' = Nx(gen)
    /\ mygen' = Set(mygen, p, Nx(gen))
'@
            New = @'
    /\ gen' = gen
    /\ mygen' = Set(mygen, p, gen)
'@
        }

        @{
            Name      = 'iso-open-without-raise'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'EffectsAreBracketedByTheCount'
            Def       = 'EndOpen'
            Old = @'
    /\ pc[p] = "opening"
    /\ count' = Nx(count)
'@
            New = @'
    /\ pc[p] = "opening"
    /\ count' = count
'@
        }

        @{
            Name      = 'iso-close-without-raise'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'CountIsOddWhileAnOperationIsUnresolved'
            Def       = 'EndClose'
            Old = @'
    /\ pc[p] = "closing"
    /\ count' = Nx(count)
'@
            New = @'
    /\ pc[p] = "closing"
    /\ count' = count
'@
        }

        @{
            Name      = 'iso-fail-unresolved-unfenced'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'FailedOperationsLeaveTheCountOddAndFence'
            Def       = 'FailUnresolved'
            Old = @'
fence' = TRUE
'@
            New = @'
fence' = fence
'@
        }

        @{
            Name      = 'iso-death-evens-the-count'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'CountIsOddWhileAnOperationIsUnresolved'
            Def       = 'Die'
            Old = @'
UNCHANGED <<count>>
'@
            New = @'
count' = IF Odd(count) THEN Nx(count) ELSE count
    /\ UNCHANGED <<>>
'@
        }

        @{
            Name      = 'iso-untrusted-reads-as-stored'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'SettledReadSawNoPartOfAnOperation'
            Def       = 'Visible'
            Old = @'
    IF trusted \/ Control = "read-untrusted-as-stored"
'@
            New = @'
    IF TRUE
'@
        }
    )
}
