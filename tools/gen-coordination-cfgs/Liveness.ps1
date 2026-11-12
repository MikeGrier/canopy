# Copyright (c) Michael Grier.
# The liveness configurations of tools/gen-coordination-cfgs.ps1: cyclic workloads, in which
# processes go on writing and attaching forever, checked for one temporal property each.

# A cyclic configuration: no reads, deaths or crashes, deadlock checked, one temporal property.
function Live($name, $header, $s, $specName, $property, $control = 'none') {
    Write-Cfg "$name.cfg" (Header $header) `
        (Cfg $s 2 0 0 0 $control $false $true @() @($property) 0 0 0 $false 0 0 $true $false $specName $true)
}

$plain = Plain 2
$unsafeReader = Upgrade @('unsafeUpgrader', 'reader') @(2, 1)
$safeReader = Upgrade @('safeUpgrader', 'reader') @(2, 1)

# ---- the cyclic workload itself: it has a cycle with a write and one with an admission ----
Live 'Coordination-Witness-WritesGoOnForever' @('Witness: with two writers that go on writing and attaching forever, writes go on',
    'forever. TLC must report Witness_WritesGoOnForever violated, which shows it is reachable.') $plain 'Spec' 'Witness_WritesGoOnForever'
Live 'Coordination-Witness-AdmissionsGoOnForever' @('Witness: with two writers that go on writing and attaching forever, admissions go',
    'on forever. TLC must report Witness_AdmissionsGoOnForever violated, which shows it is reachable.') $plain 'Spec' 'Witness_AdmissionsGoOnForever'
Write-Cfg 'Coordination-Control-UpgraderKeepsTiersCyclic.cfg' `
    (Header @('Negative control: with two upgraders that go on attaching forever, an upgrader keeps its',
        'shared tiers while it waits for the gate exclusively. TLC must report a deadlock.')) `
    (Cfg (Upgrade @('safeUpgrader', 'safeUpgrader') @(2, 2)) 2 0 0 0 'upgrader-keeps-tiers' $false $true @() @() 0 0 0 $false 0 0 $true $false 'Spec' $true)

# ---- the contract's fairness: the progress it buys ----
Live 'Coordination-Live-StoreMakesProgress' @('Under the contract''s fairness two writers going on forever make progress: every',
    'invariant and property holds.') $plain 'ContractSpec' 'StoreMakesProgress'
Live 'Coordination-Live-StoreMakesProgressUpgrade' @('Under the contract''s fairness an upgrader unsafe for readers and a reader going on',
    'forever make progress: every invariant and property holds.') $unsafeReader 'ContractSpec' 'StoreMakesProgress'
Live 'Coordination-Live-AnUpgradeHoldingItsTiersCompletes' @('Under the contract''s fairness an upgrade unsafe for readers that holds its tiers',
    'completes: every invariant and property holds.') $unsafeReader 'ContractSpec' 'AnUpgradeHoldingItsTiersCompletes'
Live 'Coordination-Live-ADrivenRecoveryCompletes' @('Under the contract''s fairness a recovery under way with two writers going on forever',
    'ends: every invariant and property holds.') $plain 'ContractSpec' 'ADrivenRecoveryCompletes'

# ---- each fairness condition, dropped: some progress fails ----
# Name, specification, scenario, what is dropped.
$unfair = @(
    @('WithoutRelease', 'ContractSpecWithoutRelease', $plain, 'a holder need never release'),
    @('WithoutIO', 'ContractSpecWithoutIO', $plain, 'an operation''s I/O need never complete'),
    @('WithoutLock', 'ContractSpecWithoutLock', $plain, 'a writer need never be granted exclusion that stays free to it'),
    @('WithoutGateS', 'ContractSpecWithoutGateS', $plain, 'a handle need never be granted the gate shared while it stays grantable'),
    @('WithoutGateX', 'ContractSpecWithoutGateX', $unsafeReader, 'an upgrader need never be granted the gate exclusively while it stays free'),
    @('WithoutReaderTier', 'ContractSpecWithoutReaderTier', $plain, 'a handle need never be granted the reader tier while it stays grantable'),
    @('WithoutWriterTier', 'ContractSpecWithoutWriterTier', $plain, 'a handle need never be granted the writer tier while it stays grantable')
)
foreach ($u in $unfair) {
    Live "Coordination-Control-Live-$($u[0])" @("Negative control: $($u[3]), so the store need not make progress.",
        'TLC must report StoreMakesProgress violated.') $u[2] $u[1] 'StoreMakesProgress'
}

# ---- each kind of request under the contract's fairness: it can be passed over forever ----
Live 'Coordination-Live-AWaitingWriterIsServed' @('Under the contract''s fairness a writer that waits for exclusion can be passed over',
    'forever by another writer. TLC must report AWaitingWriterIsServed violated.') $plain 'ContractSpec' 'AWaitingWriterIsServed'
Live 'Coordination-Live-AWaitingUpgraderIsServed' @('Under the contract''s fairness an upgrader can be passed over forever by a reader',
    'that goes on attaching. TLC must report AWaitingUpgraderIsServed violated.') $unsafeReader 'ContractSpec' 'AWaitingUpgraderIsServed'
Live 'Coordination-Live-AnAttachingHandleIsServed' @('Under the contract''s fairness a handle that waits to attach beside another that goes',
    'on attaching is admitted, since only an upgrade holds the gate or a tier exclusively: every',
    'invariant and property holds.') $plain 'ContractSpec' 'AnAttachingHandleIsServed'
Live 'Coordination-Live-AnAttachingHandleIsServedUpgrade' @('Under the contract''s fairness a handle that waits to attach beside an upgrader unsafe for',
    'readers is admitted or refused: every invariant and property holds.') $unsafeReader 'ContractSpec' 'AnAttachingHandleIsServed'

# ---- each kind of request with strong fairness on each acquisition ----
$unsafeTwoReaders = Upgrade @('unsafeUpgrader', 'reader', 'reader') @(2, 1, 1)
Live 'Coordination-Live-AWaitingWriterIsServedFair' @('With strong fairness on each acquisition a writer that waits for exclusion is served:',
    'every invariant and property holds.') $plain 'FairSpec' 'AWaitingWriterIsServed'
Live 'Coordination-Live-AWaitingUpgraderIsServedFair' @('With strong fairness on each acquisition an upgrader beside one reader is served:',
    'every invariant and property holds.') $unsafeReader 'FairSpec' 'AWaitingUpgraderIsServed'
Live 'Coordination-Live-AWaitingUpgraderIsServedFairTwoHandles' @('With strong fairness on the upgrader''s takes an upgrader can still be passed over',
    'forever by two readers whose shared holds of the gate overlap, so the gate is never free.',
    'TLC must report AWaitingUpgraderIsServed violated.') $unsafeTwoReaders 'UpgraderFairSpec' 'AWaitingUpgraderIsServed'
Live 'Coordination-Live-AWaitingUpgraderIsServedExclusiveFirst' @('With strong fairness on the upgrader''s takes, and shared admission withheld while an',
    'upgrader waits for the gate, an upgrader beside two readers is served: every invariant and',
    'property holds.') $unsafeTwoReaders 'UpgraderFairSpecExclusiveFirst' 'AWaitingUpgraderIsServed'
