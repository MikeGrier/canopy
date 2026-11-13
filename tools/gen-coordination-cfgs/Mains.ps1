# Copyright (c) Michael Grier.
# The main configurations of tools/gen-coordination-cfgs.ps1: every invariant and property holds.

# ---- main configurations: every invariant and property holds ----
$allInv = $exclusionInvariants + $attachInvariants
$allProp = $exclusionProperties + $attachProperties
Write-Cfg 'Coordination.cfg' (Header @('The contract with two processes: every invariant and property holds.')) `
    (Cfg (Plain 2) 3 1 1 1 'none' $true $false $allInv $allProp)
Write-Cfg 'Coordination-ThreeProcs.cfg' (Header @('The contract with three processes, one of which may die: every invariant and', 'property holds.')) `
    (Cfg (Plain 3) 3 1 0 0 'none' $true $false $allInv $allProp)
Write-Cfg 'Coordination-ThreeProcsCrashAndRead.cfg' (Header @('The contract with three processes, a host crash and a read: every invariant and', 'property holds.')) `
    (Cfg (Plain 3) 2 0 1 1 'none' $true $false $allInv $allProp)

# Admission and upgrade scenarios.
$attachScenarios = @(
    @('Attachment', 'one safe upgrader, a writer and a reader',
      (Upgrade @('safeUpgrader', 'writer', 'reader') @(2, 2, 1))),
    @('AttachmentUnsafe', 'one unsafe upgrader, a writer and a reader',
      (Upgrade @('unsafeUpgrader', 'writer', 'reader') @(2, 2, 1))),
    @('AttachmentTwoUpgraders', 'a safe and an unsafe upgrader and a writer',
      (Upgrade @('safeUpgrader', 'unsafeUpgrader', 'writer') @(2, 2, 2)))
)
foreach ($a in $attachScenarios) {
    Write-Cfg "Coordination-$($a[0]).cfg" `
        (Header @("Admission and upgrade with $($a[1]), which may die: every invariant and", 'property holds.')) `
        (Cfg $a[2] 1 1 0 0 'none' $true $false $allInv $allProp 1)
}

# No waiting forever: nobody gives up or dies, and the only terminal state is every process ended.
$deadlockFree = @(
    @('SafeUpgrade', 'a safe upgrader, a writer and a reader', (Upgrade @('safeUpgrader', 'writer', 'reader') @(2, 2, 1))),
    @('UnsafeUpgrade', 'an unsafe upgrader, a writer and a reader', (Upgrade @('unsafeUpgrader', 'writer', 'reader') @(2, 2, 1))),
    @('TwoUpgraders', 'a safe and an unsafe upgrader and a writer', (Upgrade @('safeUpgrader', 'unsafeUpgrader', 'writer') @(2, 2, 2))),
    @('TwoSafeUpgraders', 'two safe upgraders and a writer', (Upgrade @('safeUpgrader', 'safeUpgrader', 'writer') @(2, 2, 2)))
)
foreach ($d in $deadlockFree) {
    Write-Cfg "Coordination-NoDeadlock-$($d[0]).cfg" `
        (Header @("No deadlock with $($d[1]), none of which gives up or dies: every invariant", 'and property holds, and no state but every process ended has no successor.')) `
        (Cfg $d[2] 2 0 0 0 'none' $false $true ($attachInvariants + @('TypeOK')) $attachProperties 1)
}
Write-Cfg 'Coordination-NoDeadlock-DrainTwoLeases.cfg' `
    (Header @('No deadlock with a safe upgrader draining two operations of its handle and a reader,', 'none of which gives up or dies: every invariant and property holds, and no state but', 'every process ended has no successor.')) `
    (Cfg (Upgrade @('safeUpgrader', 'reader') @(2, 1)) 1 0 0 1 'none' $false $true ($attachInvariants + @('TypeOK')) $attachProperties 2)

# The registry: entries, deaths, crashes and listings, for each mix of handles.
# Name, what it is, scenario, entries, deaths, crashes, listings.
$registryScenarios = @(
    @('Registry', 'two handles that share one entry, one of which may die', (Plain 2), 1, 1, 0, 2),
    @('RegistryRoomy', 'two handles with an entry each, one of which may die', (Plain 2), 2, 1, 0, 1),
    @('RegistryCrash', 'two handles that share one entry, across a host crash', (Plain 2), 1, 0, 1, 1),
    @('RegistryUpgrade', 'an upgrader and a reader with an entry each, one of which may die', (Upgrade @('safeUpgrader', 'reader') @(2, 1)), 2, 1, 0, 1),
    @('RegistryUpgradeFull', 'an upgrader and a reader that share one entry, one of which may die', (Upgrade @('safeUpgrader', 'reader') @(2, 1)), 1, 1, 0, 1),
    @('RegistryThreeProcs', 'three handles with two entries', (Plain 3), 2, 0, 0, 0),
    @('RegistryThreeProcsDeath', 'three handles with one entry, one of which may die', (Plain 3), 1, 1, 0, 0)
)
foreach ($r in $registryScenarios) {
    Write-Cfg "Coordination-$($r[0]).cfg" `
        (Header @("The registry with $($r[1]): every invariant and property holds.")) `
        (Cfg $r[2] 0 $r[4] $r[5] 0 'none' $true $false $allInv $allProp 0 $r[3] $r[6])
}
Write-Cfg 'Coordination-NoDeadlock-Registry.cfg' `
    (Header @('No deadlock with a registry of one entry, an upgrader and a reader, none of which gives', 'up or dies: every invariant and property holds, and no state but every process ended', 'has no successor.')) `
    (Cfg (Upgrade @('safeUpgrader', 'reader') @(2, 1)) 0 0 0 0 'none' $false $true ($attachInvariants + @('TypeOK')) $attachProperties 1 1 1)

# Transactions: the write is begin, ingest, prepare, namespace apply and end over a commit log.
# Name, what it is, scenario, requests, deaths, crashes, reads.
$rwTxn = Upgrade @('reader', 'writer') @(1, 2)
$txnScenarios = @(
    @('Txn', 'two writers, one of which may die and the host may crash', (Plain 2), 2, 1, 1, 1),
    @('TxnThreeProcs', 'three writers, one of which may die', (Plain 3), 2, 1, 0, 0),
    @('TxnReader', 'a reader and a writer, with a read, a death and a host crash', $rwTxn, 2, 1, 1, 1)
)
foreach ($t in $txnScenarios) {
    Write-Cfg "Coordination-$($t[0]).cfg" `
        (Header @("Transactions over a commit log with $($t[1]): every invariant and", 'property holds.')) `
        (Cfg $t[2] $t[3] $t[4] $t[5] $t[6] 'none' $true $false $allInv $allProp 0 0 0 $true)
}
Write-Cfg 'Coordination-NoDeadlock-Txn.cfg' `
    (Header @('No deadlock with transactions and two writers, neither of which gives up or dies:', 'every invariant and property holds, and no state but every process ended has no', 'successor.')) `
    (Cfg (Plain 2) 2 0 0 0 'none' $false $true ($exclusionInvariants + $attachInvariants) ($exclusionProperties + $attachProperties) 0 0 0 $true)

# Recovery and discovery: a told holder runs recovery from the commit log, and any attached handle
# can discover. Name, what it is, scenario, requests, deaths, crashes, reads, discoveries.
$rwRec = Upgrade @('reader', 'writer') @(1, 2)
$recScenarios = @(
    @('Recovery', 'two writers, one of which may die and the host may crash, and a discovery', (Plain 2), 3, 1, 1, 0, 1),
    @('RecoveryReads', 'two writers, with a read, a death, a host crash and a discovery', (Plain 2), 2, 1, 1, 1, 1),
    @('RecoveryRead', 'a reader and a writer, with a read, a death, a host crash and a discovery', $rwRec, 2, 1, 1, 1, 1),
    @('RecoveryThreeProcs', 'three writers, one of which may die, and a discovery', (Plain 3), 2, 1, 0, 0, 1)
)
foreach ($t in $recScenarios) {
    Write-Cfg "Coordination-$($t[0]).cfg" `
        (Header @("Recovery and discovery with $($t[1]): every invariant and property holds.")) `
        (Cfg $t[2] $t[3] $t[4] $t[5] $t[6] 'none' $true $false $allInv $allProp 0 0 0 $true $t[7])
}
# Recovery of an unlogged write, which has no log: a told holder makes a store half applied whole.
Write-Cfg 'Coordination-RecoveryUnlogged.cfg' `
    (Header @('Recovery of unlogged writes with two writers, one of which may die and the host may', 'crash, with a read and a discovery: every invariant and property holds.')) `
    (Cfg (Plain 2) 2 1 1 1 'none' $true $false $allInv $allProp 0 0 0 $false 1 0 $true)
# Each write choosing to be a transaction or not, with recovery of both.
Write-Cfg 'Coordination-RecoveryMixed.cfg' `
    (Header @('Recovery of writes that each choose to be a transaction or not, with two writers, one of', 'which may die and the host may crash, a read and a discovery: every invariant and', 'property holds.')) `
    (Cfg (Plain 2) 2 1 1 1 'none' $true $false $allInv $allProp 0 0 0 $false 1 0 $true $true)
Write-Cfg 'Coordination-NoDeadlock-Recovery.cfg' `
    (Header @('No deadlock with recovery and two writers, neither of which gives up or dies:', 'every invariant and property holds, and no state but every process ended has no', 'successor.')) `
    (Cfg (Plain 2) 2 0 0 0 'none' $false $true ($exclusionInvariants + $attachInvariants) ($exclusionProperties + $attachProperties) 0 0 0 $true 1)

# The upgrade body: an upgrader takes writer exclusion, records its intent, makes its migration
# steps, advances the schema and clears the intent, and an interrupted upgrade is resumed by a
# writer that speaks the target schema. Name, what it is, scenario, requests, deaths, crashes,
# reads, leases, slots, listings.
$bSat = Upgrade @('safeUpgrader', 'writer') @(2, 3)
$bSaw = Upgrade @('safeUpgrader', 'writer') @(2, 2)
$bSar = Upgrade @('safeUpgrader', 'reader') @(2, 1)
$bSwr = Upgrade @('safeUpgrader', 'writer', 'reader') @(2, 2, 1)
$bStwr = Upgrade @('safeUpgrader', 'writer', 'reader') @(2, 3, 1)
$bUtwr = Upgrade @('unsafeUpgrader', 'writer', 'reader') @(2, 3, 1)
$bTwo = Upgrade @('safeUpgrader', 'safeUpgrader') @(2, 2)
$bodyScenarios = @(
    @('UpgradeBody', 'an upgrader and a writer that speaks the target schema, one of which may die', $bSat, 2, 1, 0, 0, 0, 0, 0),
    @('UpgradeBodyCrash', 'an upgrader and a writer that speaks the target schema, with a death and a host crash', $bSat, 2, 1, 1, 0, 0, 0, 0),
    @('UpgradeBodyReader', 'an upgrader, a writer that speaks the target schema and a reader, one of which may die', $bStwr, 2, 1, 0, 0, 0, 0, 0),
    @('UpgradeBodyUnsafe', 'an upgrader unsafe for readers, a writer that speaks the target schema and a reader', $bUtwr, 2, 1, 0, 0, 0, 0, 0),
    @('UpgradeBodyTwoUpgraders', 'two upgraders, one of which may die', $bTwo, 1, 1, 0, 0, 0, 0, 0),
    @('UpgradeBodySourceWriter', 'an upgrader and a writer that speaks only the source schema, one of which may die', $bSaw, 2, 1, 0, 0, 0, 0, 0),
    @('UpgradeBodyLeases', 'an upgrader draining an operation of one of its clones before its body', $bSat, 1, 0, 0, 0, 1, 0, 0),
    @('UpgradeBodyRegistry', 'an upgrader and a reader that share one registry entry, one of which may die', $bSar, 0, 1, 0, 0, 0, 1, 1),
    @('UpgradeBodyReads', 'an upgrader, a writer that speaks the target schema and a reader that reads, one of which may die', $bStwr, 1, 1, 0, 1, 0, 0, 0)
)
foreach ($b in $bodyScenarios) {
    Write-Cfg "Coordination-$($b[0]).cfg" `
        (Header @("The upgrade body with $($b[1]): every invariant and", 'property holds.')) `
        (Cfg $b[2] $b[3] $b[4] $b[5] $b[6] 'none' $true $false $allInv $allProp $b[7] $b[8] $b[9] $true 0 2)
}
Write-Cfg 'Coordination-NoDeadlock-UpgradeBody.cfg' `
    (Header @('No deadlock with the upgrade body, an upgrader and a writer, neither of which gives up or', 'dies: every invariant and property holds, and no state but every process ended has no', 'successor.')) `
    (Cfg $bSaw 2 0 0 0 'none' $false $true ($exclusionInvariants + $attachInvariants) ($exclusionProperties + $attachProperties) 0 0 0 $true 0 2)
Write-Cfg 'Coordination-NoDeadlock-UpgradeBodyReader.cfg' `
    (Header @('No deadlock with the upgrade body, an upgrader, a writer and a reader, none of which gives', 'up or dies: every invariant and property holds, and no state but every process ended has', 'no successor.')) `
    (Cfg $bSwr 2 0 0 0 'none' $false $true ($exclusionInvariants + $attachInvariants) ($exclusionProperties + $attachProperties) 0 0 0 $true 0 2)
Write-Cfg 'Coordination-NoDeadlock-UpgradeBodyTwoUpgraders.cfg' `
    (Header @('No deadlock with the upgrade body, two upgraders and a writer, none of which gives up or', 'dies: every invariant and property holds, and no state but every process ended has no', 'successor.')) `
    (Cfg (Upgrade @('safeUpgrader', 'safeUpgrader', 'writer') @(2, 2, 2)) 1 0 0 0 'none' $false $true ($exclusionInvariants + $attachInvariants) ($exclusionProperties + $attachProperties) 0 0 0 $true 0 2)

# Cumulative reader compatibility: an upgrade is judged against the oldest reader schema the store
# admits, over upgrades in a row in one layer and over upgrades of several layers. Name, what it
# is, scenario, requests, deaths, crashes, reads, upgrade steps.
$compatScenarios = @(
    @('CumulativeChain', 'two upgrades in a row that are safe for readers and a reader of the first source schema, one handle of which may die', $layered.ChainSafe, 1, 1, 0, 0, 0),
    @('CumulativeChainFloor', 'two upgrades in a row, the second safe against the current schema and not against the oldest, and a reader of the first source schema, one handle of which may die', $layered.ChainFloor, 1, 1, 0, 0, 0),
    @('CumulativeChainCrash', 'two upgrades in a row, the second safe against the current schema and not against the oldest, and a reader, across a host crash', $layered.ChainFloor, 1, 0, 1, 0, 0),
    @('CumulativeLayers', 'an upgrade of one of two layers that is safe for readers, a writer that speaks its target and a reader, one handle of which may die', $layered.TwoLayersSafe, 1, 1, 0, 0, 0),
    @('CumulativeLayersUnsafe', 'an upgrade of one of two layers that is unsafe for readers, a writer that speaks its target and a reader, one handle of which may die', $layered.TwoLayersUnsafe, 1, 1, 0, 0, 0),
    @('CumulativeLayerPair', 'an upgrade of each of two layers in turn, both safe for readers, and a reader, one handle of which may die', $layered.TwoLayerUpgradesSafe, 1, 1, 0, 0, 0),
    @('CumulativeLayerPairUnsafe', 'an upgrade of one layer that is safe for readers and one of the other that is unsafe, and a reader, one handle of which may die', $layered.TwoLayerUpgradesUnsafe, 1, 1, 0, 0, 0),
    @('CumulativeLayerPairFloor', 'an upgrade of one layer that is safe for readers and one of the other that is safe against the current schema and not against the oldest, and a reader, one handle of which may die', $layered.TwoLayerUpgradesFloor, 1, 1, 0, 0, 0),
    @('CumulativeLayerPairReads', 'an upgrade of each of two layers, one unsafe for readers, and a reader that reads', $layered.TwoLayerUpgradesUnsafe, 1, 0, 0, 1, 0),
    @('CumulativeChainBody', 'two upgrades in a row through their bodies, both safe for readers, and a reader, one handle of which may die', $layered.ChainSafe, 1, 1, 0, 0, 1),
    @('CumulativeChainFloorBody', 'two upgrades in a row through their bodies, the second safe against the current schema and not against the oldest, and a reader, one handle of which may die', $layered.ChainFloor, 1, 1, 0, 0, 1),
    @('CumulativeLayersUnsafeBody', 'an upgrade of one of two layers through its body, unsafe for readers, a writer that speaks its target and a reader, one handle of which may die', $layered.TwoLayersUnsafe, 1, 1, 0, 0, 2),
    @('CumulativeLayerPairUnsafeBody', 'an upgrade of each of two layers through its body, one unsafe for readers, and a reader, one handle of which may die', $layered.TwoLayerUpgradesUnsafe, 1, 1, 0, 0, 1)
)
foreach ($c in $compatScenarios) {
    Write-Cfg "Coordination-$($c[0]).cfg" `
        (Header @("Cumulative reader compatibility with $($c[1]): every invariant and", 'property holds.')) `
        (Cfg $c[2] $c[3] $c[4] $c[5] $c[6] 'none' $true $false $allInv $allProp 0 0 0 ($c[7] -gt 0) 0 $c[7])
}
Write-Cfg 'Coordination-NoDeadlock-CumulativeChainFloor.cfg' `
    (Header @('No deadlock with two upgrades in a row, the second safe against the current schema and not', 'against the oldest, and a reader, none of which gives up or dies: every invariant and', 'property holds, and no state but every process ended has no successor.')) `
    (Cfg $layered.ChainFloor 1 0 0 0 'none' $false $true ($attachInvariants + $layerInvariants + @('TypeOK')) ($attachProperties + $layerProperties) 0)
Write-Cfg 'Coordination-NoDeadlock-CumulativeLayerPair.cfg' `
    (Header @('No deadlock with an upgrade of each of two layers, one unsafe for readers, and a reader, none', 'of which gives up or dies: every invariant and property holds, and no state but every', 'process ended has no successor.')) `
    (Cfg $layered.TwoLayerUpgradesUnsafe 1 0 0 0 'none' $false $true ($attachInvariants + $layerInvariants + @('TypeOK')) ($attachProperties + $layerProperties) 0)
Write-Cfg 'Coordination-NoDeadlock-CumulativeLayerPairBody.cfg' `
    (Header @('No deadlock with an upgrade of each of two layers through its body, one unsafe for readers,', 'and a reader, none of which gives up or dies: every invariant and property holds, and no', 'state but every process ended has no successor.')) `
    (Cfg $layered.TwoLayerUpgradesUnsafe 1 0 0 0 'none' $false $true ($exclusionInvariants + $attachInvariants) ($exclusionProperties + $attachProperties) 0 0 0 $true 0 1)

