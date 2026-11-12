# Copyright (c) Michael Grier.
# Probes of clones, leases and the upgrade drain.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'drain-always-done'
            Base      = 'Coordination-Witness-AnUpgradeWaitsForACloneOperation'
            Kind      = 'INVARIANTS'
            Property  = 'NoLeaseIsHeldOnceAConversionBegins'
            Def       = 'DrainDone'
            Old = @'
    /\ (Leases(p) = 0 \/ Control = "drain-ignores-leases")
'@
            New = @'
    /\ TRUE
'@
        }

        @{
            Name      = 'leases-forget-clones'
            Base      = 'Coordination-Witness-AnUpgradeWaitsForACloneOperation'
            Kind      = 'INVARIANTS'
            Property  = 'NoLeaseIsHeldOnceAConversionBegins'
            Def       = 'Leases'
            Old = @'
Leases(p) == lease[p] + (IF rphase[p] # "none" THEN 1 ELSE 0)
'@
            New = @'
Leases(p) == (IF rphase[p] # "none" THEN 1 ELSE 0)
'@
        }

        @{
            Name      = 'clone-starts-anywhere'
            Base      = 'Coordination-Witness-ACloneOperationStarts'
            Kind      = 'PROPERTIES'
            Property  = 'NoOperationStartsWhileTheHandleConverts'
            Def       = 'BeginOp'
            Old = @'
/\ \/ att[p] = "attached"
       \/ att[p] = "draining" /\ Control = "clone-starts-while-draining"
       \/ att[p] = "uBody" /\ Control = "clone-starts-during-the-body"
'@
            New = @'
/\ att[p] # "off"
'@
        }

        @{
            Name      = 'cancelled-conversion-exits'
            Base      = 'Coordination-Witness-AConversionIsCancelled'
            Kind      = 'PROPERTIES'
            Property  = 'ACancelledOrFailedConversionLeavesTheHandleDetached'
            Def       = 'GiveUp'
            Old = @'
                         THEN "idle" ELSE "detached")
'@
            New = @'
                         THEN "idle" ELSE "exited")
'@
        }

        @{
            Name      = 'failed-conversion-exits'
            Base      = 'Coordination-Witness-AConversionFailsAtTheRecheck'
            Kind      = 'PROPERTIES'
            Property  = 'ACancelledOrFailedConversionLeavesTheHandleDetached'
            Def       = 'UpgradeMoved'
            Old = @'
    /\ pc' = Set(pc, p, IF pc[p] = "convertingX" THEN "held" ELSE "detached")
'@
            New = @'
    /\ pc' = Set(pc, p, IF pc[p] = "convertingX" THEN "held" ELSE "exited")
'@
        }

        @{
            Name      = 'cancelled-drain-exits'
            Base      = 'Coordination-Witness-ADrainIsCancelled'
            Kind      = 'PROPERTIES'
            Property  = 'ACancelledDrainLeavesTheHandleAttached'
            Def       = 'CancelDrain'
            Old = @'
               /\ pc' = Set(pc, p, "idle")
'@
            New = @'
               /\ pc' = Set(pc, p, "exited")
'@
        }

        @{
            Name      = 'upgrade-ignores-inside'
            Base      = 'Coordination-Witness-AnUpgradeIsRefusedFromInsideARead'
            Kind      = 'PROPERTIES'
            Property  = 'AnUpgradeNeverStartsFromInsideAnOperation'
            Def       = 'StartUpgrade'
            Old = @'
    /\ (rphase[p] = "none" \/ Control = "upgrade-from-inside-an-operation")
'@
            New = @'
    /\ TRUE
'@
        }

        @{
            Name      = 'upgrade-ignores-inside-caller'
            Base      = 'Coordination-Witness-AnUpgradeIsRefusedFromInsideARead'
            Kind      = 'INVARIANTS'
            Property  = 'NoDrainWaitsOnItsOwnCaller'
            Def       = 'StartUpgrade'
            Old = @'
    /\ (rphase[p] = "none" \/ Control = "upgrade-from-inside-an-operation")
'@
            New = @'
    /\ TRUE
'@
        }

        @{
            Name      = 'die-keeps-leases'
            Base      = 'Coordination-Witness-ADeathEndsTheLeases'
            Kind      = 'INVARIANTS'
            Property  = 'AttachmentMatchesTheProcess'
            Def       = 'Die'
            Old = @'
    /\ lease' = Set(lease, p, 0)
'@
            New = @'
    /\ lease' = lease
'@
        }

        @{
            Name      = 'exit-with-leases'
            Base      = 'Coordination-Witness-ACloneOperationStarts'
            Kind      = 'INVARIANTS'
            Property  = 'AttachmentMatchesTheProcess'
            Def       = 'Exit'
            Old = @'
    /\ pc[p] \in {"idle", "detached"} /\ rphase[p] = "none" /\ lease[p] = 0
'@
            New = @'
    /\ pc[p] \in {"idle", "detached"} /\ rphase[p] = "none"
'@
        }

        @{
            Name      = 'refusal-stays-pending'
            Base      = 'Coordination-Witness-AnUpgradeIsRefusedFromInsideARead'
            Kind      = 'INVARIANTS'
            Property  = 'AttachRequestStateMatchesProcess'
            Def       = 'RefuseUpgrade'
            Old = @'
    /\ Upgrader(p) /\ att[p] = "attached" /\ InsideOperation(p)
    /\ areq' = Set(areq, p, "refused")
'@
            New = @'
    /\ Upgrader(p) /\ att[p] = "attached" /\ InsideOperation(p)
    /\ areq' = Set(areq, p, "pending")
'@
        }

        @{
            Name      = 'drain-holds-no-tiers'
            Base      = 'Coordination-Witness-AnUpgradeWaitsForACloneOperation'
            Kind      = 'INVARIANTS'
            Property  = 'AnOperationInFlightHoldsTheTiers'
            Def       = 'HoldsReaderS'
            Old = @'
                   "attached", "unregistering", "draining"}
'@
            New = @'
                   "attached", "unregistering"}
'@
        }

        @{
            Name      = 'reattach-needs-new'
            Base      = 'Coordination-Witness-ADetachedHandleAttachesAgain'
            Kind      = 'PROPERTIES'
            Property  = 'Witness_ADetachedHandleAttachesAgain'
            Expect    = 'passes'
            Def       = 'BeginAttach'
            Old = @'
    /\ pc[p] \in {"new", "detached"}
'@
            New = @'
    /\ pc[p] = "new"
'@
        }

        @{
            Name      = 'iso-inside-callback-waits'
            Base      = 'Coordination-Control-UpgradeFromInsideAnOperation'
            Kind      = 'INVARIANTS'
            Property  = 'NoDrainWaitsOnItsOwnCaller'
        }

        @{
            Name      = 'iso-inside-callback-starts'
            Base      = 'Coordination-Control-UpgradeFromInsideAnOperation'
            Kind      = 'PROPERTIES'
            Property  = 'AnUpgradeNeverStartsFromInsideAnOperation'
        }

        @{
            Name      = 'iso-holds-exclusion-inside'
            Base      = 'Coordination-Control-UpgraderHoldsExclusion'
            Kind      = 'PROPERTIES'
            Property  = 'AnUpgradeNeverStartsFromInsideAnOperation'
        }

        @{
            Name      = 'iso-leases-hold-tiers'
            Base      = 'Coordination-Control-DrainIgnoresLeases'
            Kind      = 'INVARIANTS'
            Property  = 'AnOperationInFlightHoldsTheTiers'
        }
    )
}
