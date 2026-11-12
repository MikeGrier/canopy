# Copyright (c) Michael Grier.
# The liveness configurations of tools/gen-coordination-cfgs.ps1: every hold ends, on bounded
# configurations, under the fairness of each process's own steps (HoldSpec), and each condition of
# it dropped as a control that lets a hold last forever.

# A bounded configuration checking temporal properties only, under a hold specification.
function Holds($name, $header, $s, $specName, $properties, $requests, $deaths, $reads = 0, $leases = 0, $slots = 0, $listings = 0, $txn = $false, $upSteps = 0, $unloggedRecovery = $false) {
    Write-Cfg "$name.cfg" (Header $header) `
        (Cfg $s $requests $deaths 0 $reads 'none' $true $false @() $properties $leases $slots $listings $txn 0 $upSteps $unloggedRecovery $false $specName)
}

$plain = Plain 2
$safeTarget = Upgrade @('safeUpgrader', 'writer') @(2, 3)
$safeReader = Upgrade @('safeUpgrader', 'reader') @(2, 1)
$unsafeWR = Upgrade @('unsafeUpgrader', 'writer', 'reader') @(2, 2, 1)

# ---- every hold ends ----
Holds 'Coordination-HoldsEnd-Txn' @('Under the fairness of each process''s own steps, with transactions and two writers, one',
    'of which may die, every hold of exclusion and every recovery under way ends and every',
    'attach is admitted or refused: every invariant and property holds.') $plain 'HoldSpec' `
    @('AHoldOfExclusionEnds', 'ADrivenRecoveryCompletes', 'AnAttachDropsTheGate', 'AnAdmissionCompletes') 2 1 0 0 0 0 $true
Holds 'Coordination-HoldsEnd-Unlogged' @('Under the fairness of each process''s own steps, with unlogged writes, their recovery,',
    'a read and two writers, one of which may die, every hold of exclusion and every',
    'recovery under way ends: every invariant and property holds.') $plain 'HoldSpec' `
    @('AHoldOfExclusionEnds', 'ADrivenRecoveryCompletes') 2 1 1 0 0 0 $false 0 $true
Holds 'Coordination-HoldsEnd-UpgradeBody' @('Under the fairness of each process''s own steps, with an upgrader draining an operation',
    'of its handle before its body and a writer of the target schema, one of which may die, every',
    'hold of exclusion, drain and upgrade holding its tiers ends: every invariant and property',
    'holds.') $safeTarget 'HoldSpec' `
    @('AHoldOfExclusionEnds', 'AnUpgradeHoldingItsTiersCompletes', 'ADrainEnds', 'AnAdmissionCompletes', 'AnAttachDropsTheGate') 1 1 0 1 0 0 $true 2
Holds 'Coordination-HoldsEnd-Registry' @('Under the fairness of each process''s own steps, with an upgrader and a reader that share',
    'one registry entry, a listing and an operation of a clone, one of which may die, every',
    'admission, drain, detach and upgrade holding its tiers ends: every invariant and property',
    'holds.') $safeReader 'HoldSpec' `
    @('ADetachCompletes', 'ADrainEnds', 'AnAdmissionCompletes', 'AnUpgradeHoldingItsTiersCompletes') 0 1 0 1 1 1
Holds 'Coordination-HoldsEnd-Unsafe' @('Under the fairness of each process''s own steps, with an upgrader unsafe for readers, a',
    'writer and a reader, one of which may die, every hold of exclusion and upgrade holding its',
    'tiers ends: every invariant and property holds.') $unsafeWR 'HoldSpec' `
    @('AHoldOfExclusionEnds', 'AnUpgradeHoldingItsTiersCompletes', 'AnAdmissionCompletes', 'AnAttachDropsTheGate') 1 1

# ---- each condition dropped: a hold can last forever ----
Holds 'Coordination-Control-HoldsEnd-WithoutRelease' @('Negative control: a holder need never release, so it can hold exclusion forever. TLC must',
    'report AHoldOfExclusionEnds violated.') $plain 'HoldSpecWithoutRelease' @('AHoldOfExclusionEnds') 1 0 0 0 0 0 $true
Holds 'Coordination-Control-HoldsEnd-WithoutLeases' @('Negative control: a clone''s operation need never end, so a drain can wait for it forever.',
    'TLC must report ADrainEnds violated.') $safeReader 'HoldSpecWithoutLeases' @('ADrainEnds') 0 0 0 1
# Name, scenario, property, requests, leases, slots, listings, transactions.
$ownSteps = @(
    @('Gate', $plain, 'AnAttachDropsTheGate', 0, 0, 0, 0, $false),
    @('Admission', $plain, 'AnAdmissionCompletes', 0, 0, 0, 0, $false),
    @('Detach', $safeReader, 'ADetachCompletes', 0, 0, 1, 0, $false),
    @('Upgrade', $safeReader, 'AnUpgradeHoldingItsTiersCompletes', 0, 0, 0, 0, $false),
    @('Recovery', $plain, 'ADrivenRecoveryCompletes', 2, 0, 0, 0, $true)
)
foreach ($o in $ownSteps) {
    Holds "Coordination-Control-HoldsEnd-WithoutOwnSteps-$($o[0])" @('Negative control: a process need never take its own steps, so what it holds it can hold',
        "forever. TLC must report $($o[2]) violated.") $o[1] 'HoldSpecWithoutOwnSteps' @($o[2]) $o[3] 0 0 $o[4] $o[5] $o[6] $o[7]
}
