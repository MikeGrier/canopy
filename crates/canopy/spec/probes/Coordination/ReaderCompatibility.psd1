# Copyright (c) Michael Grier.
# Probes of cumulative reader compatibility.
# tools/run-probes.ps1 describes the fields and how a mutant stays current.
@{
    Mutants = @(
        @{
            Name      = 'safe-judged-with-strict-comparison'
            Base      = 'Coordination-CumulativeChain'
            Kind      = 'PROPERTIES'
            Property  = 'AnUpgradeIsJudgedAgainstTheOldestSchema'
            Def       = 'SafeFor'
            Old = @'
    UpF(p) <= (IF Control = "unsafe-judged-against-the-current-schema"
'@
            New = @'
    UpF(p) < (IF Control = "unsafe-judged-against-the-current-schema"
'@
        }

        @{
            Name      = 'unsafe-upgrade-does-not-raise-the-oldest'
            Base      = 'Coordination-CumulativeChainFloor'
            Kind      = 'INVARIANTS'
            Property  = 'OldestCoversTheDataFloor'
            Def       = 'OldestAfterCompleting'
            Old = @'
            ELSE Set(oldest, ulayer, utgt)
'@
            New = @'
            ELSE oldest
'@
        }

        @{
            Name      = 'completion-advances-the-first-layer'
            Base      = 'Coordination-CumulativeLayerPairUnsafe'
            Kind      = 'PROPERTIES'
            Property  = 'AnUpgradeOfOneLayerLeavesTheOthersAlone'
            Def       = 'EndUpgrade'
            Old = @'
    /\ current' = Set(current, ulayer, utgt)
    /\ dfloor' = Set(dfloor, ulayer, Max(dfloor[ulayer], ufloor))
    /\ oldest' = OldestAfterCompleting
    /\ ulayer' = 0
'@
            New = @'
    /\ current' = Set(current, 1, utgt)
    /\ dfloor' = Set(dfloor, ulayer, Max(dfloor[ulayer], ufloor))
    /\ oldest' = OldestAfterCompleting
    /\ ulayer' = 0
'@
        }

        @{
            Name      = 'body-completion-advances-the-first-layer'
            Base      = 'Coordination-CumulativeLayerPairUnsafeBody'
            Kind      = 'PROPERTIES'
            Property  = 'AnUpgradeOfOneLayerLeavesTheOthersAlone'
            Def       = 'UpAdvance'
            Old = @'
    /\ current' = Set(current, ulayer, utgt)
    /\ dfloor' = Set(dfloor, ulayer, Max(dfloor[ulayer], ufloor))
    /\ oldest' = OldestAfterCompleting
    /\ UNCHANGED <<upState, ulayer, utgt, ufloor, ustep, rtier, pc, att, areq, lease,
'@
            New = @'
    /\ current' = Set(current, 1, utgt)
    /\ dfloor' = Set(dfloor, ulayer, Max(dfloor[ulayer], ufloor))
    /\ oldest' = OldestAfterCompleting
    /\ UNCHANGED <<upState, ulayer, utgt, ufloor, ustep, rtier, pc, att, areq, lease,
'@
        }

        @{
            Name      = 'the-intent-records-the-oldest-as-its-floor'
            Base      = 'Coordination-CumulativeChainFloor'
            Kind      = 'PROPERTIES'
            Property  = 'AnUpgradeIsJudgedAgainstTheOldestSchema'
            Def       = 'BeginUpgrade'
            Old = @'
               /\ ufloor' = UpF(p)
'@
            New = @'
               /\ ufloor' = oldest[UpL(p)]
'@
        }

        @{
            Name      = 'admission-ignores-the-oldest-schema'
            Base      = 'Coordination-CumulativeChainFloor'
            Kind      = 'INVARIANTS'
            Property  = 'AnAttachedHandleCanReadTheData'
            Def       = 'ReaderOK'
            Old = @'
            /\ ((oldest[l] <= s /\ s <= current[l]) \/ s = utgt)
       ELSE oldest[l] <= s /\ s <= current[l]
'@
            New = @'
            /\ ((s <= current[l]) \/ s = utgt)
       ELSE s <= current[l]
'@
        }

        @{
            Name      = 'upgrade-rules-are-not-layer-specific'
            Base      = 'Coordination-CumulativeLayers'
            Kind      = 'INVARIANTS'
            Property  = 'TheOtherLayersKeepTheirOrdinaryRules'
            Def       = 'InProgress'
            Old = @'
    upState # "ordinary" /\ (l = ulayer \/ Control = "upgrade-rules-apply-to-every-layer")
'@
            New = @'
    upState # "ordinary"
'@
        }
    )
}
