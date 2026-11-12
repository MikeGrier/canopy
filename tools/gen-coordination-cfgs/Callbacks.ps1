# Copyright (c) Michael Grier.
# The callback configurations of tools/gen-coordination-cfgs.ps1: the store calls back into a caller
# under exclusion at the end of each operation (Callbacks), under the rule a callback keeps -- it
# waits on nothing, does not call into the store, and returns -- and a control for each clause.

$plain = Plain 2
$callbackTarget = Upgrade @('safeUpgrader', 'writer') @(2, 3)

# ---- the rule kept: every invariant and property holds ----
Write-Cfg 'Coordination-Callback.cfg' `
    (Header @('Callbacks under exclusion with transactions, two writers, one of which may die, a host crash,',
        'a read and a discovery: every invariant and property holds.')) `
    (Cfg $plain 2 1 1 1 'none' $true $false $allInv $allProp 0 0 0 $true 1 0 $false $false 'Spec' $true)
Write-Cfg 'Coordination-CallbackUpgradeBody.cfg' `
    (Header @('Callbacks under exclusion with the upgrade body, an upgrader and a writer of the target',
        'schema, one of which may die: every invariant and property holds.')) `
    (Cfg $callbackTarget 2 1 0 0 'none' $true $false $allInv $allProp 0 0 0 $true 0 2 $false $false 'Spec' $true)
Write-Cfg 'Coordination-NoDeadlock-Callback.cfg' `
    (Header @('No deadlock with callbacks under exclusion, transactions and two writers, neither of which',
        'gives up or dies: every invariant and property holds, and no state but every process ended',
        'has no successor.')) `
    (Cfg $plain 2 0 0 0 'none' $false $true ($exclusionInvariants + $attachInvariants) ($exclusionProperties + $attachProperties) 0 0 0 $true 0 0 $false $false 'Spec' $true)
Write-Cfg 'Coordination-HoldsEnd-Callback.cfg' `
    (Header @('Under the fairness of each process''s own steps, with callbacks under exclusion,',
        'transactions and two writers, one of which may die, every hold of exclusion ends: every',
        'invariant and property holds.')) `
    (Cfg $plain 2 1 0 0 'none' $true $false @() @('AHoldOfExclusionEnds') 0 0 0 $true 0 0 $false $false 'HoldSpec' $true)
Write-Cfg 'Coordination-Witness-ACallbackReturns.cfg' `
    (Header @('Witness: a callback under exclusion returns. TLC must report Witness_ACallbackReturns',
        'violated, which shows it is reachable.')) `
    (Cfg $plain 1 0 0 0 'none' $true $false @() @('Witness_ACallbackReturns') 0 0 0 $true 0 0 $false $false 'Spec' $true)

# ---- each clause of the rule broken ----
Write-Cfg 'Coordination-Control-CallbackChangesTheStore.cfg' `
    (Header @('Negative control: a callback writes to the store. TLC must report',
        'ACallbackChangesOnlyWhereItsHolderIs violated.')) `
    (Cfg $plain 1 0 0 0 'callback-changes-the-store' $true $false @() @('ACallbackChangesOnlyWhereItsHolderIs') 0 0 0 $true 0 0 $false $false 'Spec' $true)
Write-Cfg 'Coordination-Control-CallbackWaitsForExclusion.cfg' `
    (Header @('Negative control: a callback waits until nobody holds exclusion, which its own holder does,',
        'so it waits forever. TLC must report a deadlock.')) `
    (Cfg $plain 1 0 0 0 'callback-waits-for-exclusion' $false $true @() @() 0 0 0 $true 0 0 $false $false 'Spec' $true)
Write-Cfg 'Coordination-Control-HoldsEnd-CallbackNeverReturns.cfg' `
    (Header @('Negative control: a callback never returns, so its holder holds exclusion forever. TLC must',
        'report AHoldOfExclusionEnds violated.')) `
    (Cfg $plain 1 0 0 0 'callback-never-returns' $true $false @() @('AHoldOfExclusionEnds') 0 0 0 $true 0 0 $false $false 'HoldSpec' $true)
