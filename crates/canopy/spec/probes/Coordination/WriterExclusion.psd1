# Copyright (c) Michael Grier.
# Probes of writer exclusion and the ownership record.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'lock-ignores-holder'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'AtMostOneHolder'
            Def       = 'Lock'
            Old = @'
    /\ lockHolder = None
    /\ lockHolder' = p
'@
            New = @'
    /\ lockHolder' = p
'@
        }

        @{
            Name      = 'die-keeps-lock'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'LockMatchesHolders'
            Def       = 'Die'
            Old = @'
    /\ lockHolder' = IF lockHolder = p THEN None ELSE lockHolder
'@
            New = @'
    /\ lockHolder' = lockHolder
'@
        }

        @{
            Name      = 'die-clears-record'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'AbandonmentIsNeverForgotten'
            Def       = 'Die'
            Old = @'
    /\ UNCHANGED <<eligible, owner, fence, trusted, store, requests, crashes>>
'@
            New = @'
    /\ owner' = IF owner = p THEN None ELSE owner
    /\ UNCHANGED <<eligible, fence, trusted, store, requests, crashes>>
'@
        }

        @{
            Name      = 'die-clears-record-unseen'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'UnresolvedWorkIsNeverInvisible'
            Edits     = @(
                @{
                    Def = 'Die'
                    Old = @'
diedIn' = IF owner = p THEN pc[p] ELSE diedIn
'@
                    New = @'
diedIn' = diedIn
'@
                }
                @{
                    Def = 'Die'
                    Old = @'
UNCHANGED <<owner>>
'@
                    New = @'
owner' = IF owner = p THEN None ELSE owner
    /\ UNCHANGED <<>>
'@
                }
            )
        }

        @{
            Name      = 'die-completes-write'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'DeathLeavesTheFilesystemAsItWas'
            Def       = 'Die'
            Old = @'
UNCHANGED <<store>>
'@
            New = @'
b' = IF pc[p] = "writing" THEN a ELSE b
    /\ UNCHANGED <<a, shared, log, stage, ptgt>>
'@
        }

        @{
            Name      = 'release-keeps-record'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'OwnerRecordNamesItsHolder'
            Def       = 'Release'
            Old = @'
               /\ owner' = None
'@
            New = @'
               /\ owner' = owner
'@
        }

        @{
            Name      = 'release-clears-fence'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'AbandonmentIsNeverForgotten'
            Def       = 'Release'
            Old = @'
    /\ UNCHANGED <<req, eligible, fence, trusted, store, diedIn, counts>>
'@
            New = @'
    /\ fence' = FALSE
    /\ UNCHANGED <<req, eligible, trusted, store, diedIn, counts>>
'@
        }

        @{
            Name      = 'told-writes'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'StoreDataChangesOnlyByAHealthyHolder'
            Def       = 'EffectFrom'
            Old = @'
    ELSE {"open"}
'@
            New = @'
    ELSE {"open", "told"}
'@
        }

        @{
            Name      = 'told-ignores-fence'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'NormalAcquireFindsAHealthyStore'
            Def       = 'Record'
            Old = @'
IF (owner # None \/ fence
'@
            New = @'
IF (owner # None
'@
        }

        @{
            Name      = 'always-told'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'ToldOnlyWithCause'
            Def       = 'Record'
            Old = @'
THEN "told" ELSE "held")
'@
            New = @'
THEN "told" ELSE "told")
'@
        }

        @{
            Name      = 'crash-keeps-trust'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'AbandonmentIsNeverForgotten'
            Def       = 'Crash'
            Old = @'
    /\ trusted' = FALSE
'@
            New = @'
    /\ trusted' = trusted
'@
        }

        @{
            Name      = 'crash-leaves-processes'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'CrashLeavesNoLiveProcess'
            Def       = 'Crash'
            Old = @'
    /\ pc' = [p \in Procs |-> IF pc[p] \in LiveStates THEN "dead" ELSE pc[p]]
'@
            New = @'
    /\ pc' = pc
'@
        }

        @{
            Name      = 'serve-ineligible'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'EveryRequestEndsInExactlyOneOutcome'
            Def       = 'Lock'
            Old = @'
    /\ pc[p] = "waiting" /\ eligible[p]
'@
            New = @'
    /\ pc[p] = "waiting"
'@
        }

        @{
            Name      = 'refuse-eligible'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'OutcomeIsJustified'
            Def       = 'Refuse'
            Old = @'
    /\ pc[p] = "waiting" /\ ~eligible[p]
'@
            New = @'
    /\ pc[p] = "waiting"
'@
        }

        @{
            Name      = 'withdraw-wrong-outcome'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'OutcomeIsJustified'
            Def       = 'Withdraw'
            Old = @'
    /\ req' = Set(req, p, "withdrawn")
'@
            New = @'
    /\ req' = Set(req, p, "refused")
'@
        }

        @{
            Name      = 'die-leaves-request-pending'
            Base      = 'Coordination'
            Kind      = 'INVARIANTS'
            Property  = 'RequestStateMatchesProcess'
            Def       = 'Die'
            Old = @'
    /\ req' = IF pc[p] = "waiting" THEN Set(req, p, "dead") ELSE req
'@
            New = @'
    /\ req' = req
'@
        }

        @{
            Name      = 'attach-changes-data'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'StoreDataChangesOnlyByAHealthyHolder'
            Def       = 'Admit'
            Old = @'
UNCHANGED <<schema, rest>>
'@
            New = @'
a' = Nx(a)
    /\ UNCHANGED <<schema, lease, reg, req, eligible, coord, b, shared, log, stage, ptgt, handle, reader, diedIn, counts>>
'@
        }

        @{
            Name      = 'acquire-recovers'
            Base      = 'Coordination'
            Kind      = 'PROPERTIES'
            Property  = 'StoreDataChangesOnlyByAHealthyHolder'
            Def       = 'Record'
            Old = @'
UNCHANGED <<store>>
'@
            New = @'
b' = a
    /\ UNCHANGED <<a, shared, log, stage, ptgt>>
'@
        }
    )
}
