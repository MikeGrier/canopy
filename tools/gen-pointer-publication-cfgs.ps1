# Copyright (c) Michael Grier.
#
# tools/gen-pointer-publication-cfgs.ps1 -- write every crates/canopy/spec/PointerPublication*.cfg
# from tables.
#
# Usage:
#   pwsh tools/gen-pointer-publication-cfgs.ps1
#
# Each configuration states in its header what TLC must report, which tools/run-tlc-all.ps1 reads.
# It removes every existing PointerPublication*.cfg first, so a configuration dropped from a table
# leaves the spec directory. Change a configuration by changing its table, never the generated file;
# regenerating changes no file when the tables have not changed. Output is LF and 7-bit ASCII.

$ErrorActionPreference = 'Stop'
$spec = Join-Path (Split-Path -Parent $PSScriptRoot) 'crates/canopy/spec'
$utf8 = New-Object System.Text.UTF8Encoding($false)
Get-ChildItem $spec -Filter 'PointerPublication*.cfg' | Remove-Item

function Write-Config([string]$name, [string[]]$header, [string]$fix, [bool]$syncBeforeCollect,
                      [string[]]$invariants, [string[]]$properties = @(), [bool]$recordDetail = $false,
                      [string]$control = 'none', [bool]$loggedWrites = $false,
                      [bool]$maintenance = $false) {
    $lines = @('\* Copyright (c) Michael Grier.')
    foreach ($h in $header) { $lines += '\* ' + $h }
    $lines += 'CONSTANTS'
    $lines += '    NPtr = 3'
    $lines += '    NBlob = 2'
    $lines += '    Versions = {"A", "B", "C"}'
    $lines += '    Initial = "A"'
    $lines += '    MaxWrites = 2'
    $lines += '    MaxCrashes = 1'
    $lines += "    SyncBeforeCollect = $(if ($syncBeforeCollect) { 'TRUE' } else { 'FALSE' })"
    $lines += "    Fix = `"$fix`""
    $lines += "    RecordDetail = $(if ($recordDetail) { 'TRUE' } else { 'FALSE' })"
    $lines += "    LoggedWrites = $(if ($loggedWrites) { 'TRUE' } else { 'FALSE' })"
    $lines += "    Maintenance = $(if ($maintenance) { 'TRUE' } else { 'FALSE' })"
    $lines += "    Control = `"$control`""
    $lines += 'SPECIFICATION Spec'
    $lines += 'INVARIANTS'
    $lines += ($invariants | ForEach-Object { "    $_" })
    if ($properties.Count -gt 0) { $lines += 'PROPERTIES'; $lines += ($properties | ForEach-Object { "    $_" }) }
    $lines += 'CHECK_DEADLOCK FALSE'
    $text = ($lines -join "`n") + "`n"
    [System.IO.File]::WriteAllText((Join-Path $spec "$name.cfg"), $text, $utf8)
}

$all = @('TypeOK', 'DurablePointerHasBlob', 'LivePointerHasBlob', 'IdleImpliesSettled')
$noStranding = @('TypeOK', 'DurablePointerHasBlob', 'LivePointerHasBlob')
$recordProperties = @('PointersMoveOnlyUnderADurableRecord', 'TheRecordGoesOnlyOnceItsVersionIsDurable')

# The protocol as built: with no STRANDED-VOTE guard selected, every invariant but the one the
# guard is for holds.
Write-Config 'PointerPublication' @('The current protocol: every invariant and property holds.') 'none' $true $noStranding

# Candidate guards for STRANDED-VOTE: name, guard, outcome header, properties of the guard.
$guards = @(
    @('PointerPublication-FixRecord', 'record', @('STRANDED-VOTE candidate guard "record" holds: every invariant and property holds,', 'including IdleImpliesSettled.'), $recordProperties),
    @('PointerPublication-FixResolve', 'resolve', @('STRANDED-VOTE candidate guard "resolve" holds: every invariant and property holds,', 'including IdleImpliesSettled.'), @()),
    @('PointerPublication-FixResolveLazy', 'resolve-lazy', @('STRANDED-VOTE candidate guard "resolve-lazy" fails: live unanimity says nothing', 'about the durable replicas, so a crash after a skipped resolve exposes three', 'values. TLC must report IdleImpliesSettled violated.'), @()),
    @('PointerPublication-FixFence', 'fence', @('STRANDED-VOTE candidate guard "fence" fails: the fence lives in the process, so', 'closing and reopening clears it and the next partial publication strands the', 'vote. TLC must report IdleImpliesSettled violated.'), @())
)
foreach ($g in $guards) { Write-Config $g[0] $g[2] $g[1] $true $all $g[3] }

# The "record" guard with its record's live and durable state separate and every fsync able to fail.
Write-Config 'PointerPublication-RecordDetail' @('The "record" guard with its record''s live and durable state separate, so that', 'writing and clearing it can fail part-way: every invariant and property holds.') 'record' $true $all $recordProperties $true

# Every guard beside a write that a transaction makes, mixed with the writes that are not logged.
Write-Config 'PointerPublication-MixedRecord' @('The "record" guard with its record''s live and durable state separate, beside', 'writes that a transaction makes: every invariant and property holds.') 'record' $true $all $recordProperties $true 'none' $true
Write-Config 'PointerPublication-MixedRecordAtomic' @('The "record" guard with its record written and cleared in one step, beside writes', 'that a transaction makes: every invariant and property holds.') 'record' $true $all $recordProperties $false 'none' $true
Write-Config 'PointerPublication-MixedResolve' @('The "resolve" guard beside writes that a transaction makes: every invariant and', 'property holds.') 'resolve' $true $all @() $false 'none' $true
Write-Config 'PointerPublication-MixedResolveSkippedByLoggedWrites' @('The "resolve" guard, which a transaction''s write of the document does not run:', 'every invariant and property holds, since the transaction''s own recovery restores', 'the pointer majority.') 'resolve' $true $all @() $false 'logged-write-skips-the-guard' $true

# Every guard with the operations that read or repair a document without publishing to it.
$maintInvariants = $noStranding + @('IdleImpliesSettledUnlessAReplicaIsLost', 'ACheckReadsASettledRecord')
$maintProperties = $recordProperties + @('RepairOnlyFillsAMissingReplicaWithTheVote')
Write-Config 'PointerPublication-MaintenanceRecord' @('The "record" guard with checks, collection, repair and scrub: every invariant and property holds.') 'record' $true $maintInvariants $maintProperties $true 'none' $true $true
Write-Config 'PointerPublication-MaintenanceResolve' @('The "resolve" guard with checks, collection, repair and scrub: every invariant and property holds.') 'resolve' $true $maintInvariants @('RepairOnlyFillsAMissingReplicaWithTheVote') $false 'none' $true $true
Write-Config 'PointerPublication-MaintenanceRecordLostReplica' @('The "record" guard with a lost pointer replica: a write that fails part-way publishing to a', 'document that has lost one replica leaves three values, because only a repair', 'restores a replica under this guard. TLC must report IdleImpliesSettled violated.') 'record' $true @('TypeOK', 'IdleImpliesSettled') @() $true 'none' $true $true
Write-Config 'PointerPublication-MaintenanceResolveLostReplica' @('The "resolve" guard with a lost pointer replica, which the next write restores:', 'every invariant and property holds, including IdleImpliesSettled.') 'resolve' $true @('TypeOK', 'IdleImpliesSettled') @() $false 'none' $true $true

# Negative controls.
Write-Config 'PointerPublication-NoCollectSync' @('Negative control: without the namespace sync before collection,', 'TLC must report DurablePointerHasBlob violated (the DANGLING-DURABLE', 'counterexample).') 'none' $false @('TypeOK', 'DurablePointerHasBlob')
Write-Config 'PointerPublication-Settled' @('STRANDED-VOTE: no operation in progress implies a pointer majority (a version', 'or absence). With no guard selected the protocol strands the vote, so TLC must', 'report IdleImpliesSettled violated.') 'none' $true @('IdleImpliesSettled')

# Controls of the record: name, control, target, kind, description.
$controls = @(
    @('RecordNotSynced', 'record-not-synced', 'PointersMoveOnlyUnderADurableRecord', 'prop', 'The pointer replicas are renamed before the record is made durable.'),
    @('RecordNotSyncedStrandsTheVote', 'record-not-synced', 'IdleImpliesSettled', 'inv', 'The pointer replicas are renamed before the record is made durable, and a crash loses it.'),
    @('SettleWithoutSync', 'settle-without-sync', 'PointersMoveOnlyUnderADurableRecord', 'prop', 'A settle rolls the pointer replicas forward from a record that is in the file and not yet durable.'),
    @('SettleWithoutSyncStrandsTheVote', 'settle-without-sync', 'IdleImpliesSettled', 'inv', 'A settle rolls the pointer replicas forward from a record that is in the file and not yet durable, and a crash loses the record.'),
    @('RecordClearedEarly', 'record-cleared-early', 'TheRecordGoesOnlyOnceItsVersionIsDurable', 'prop', 'The record is cleared before the pointer replicas are renamed.'),
    @('RecordClearedEarlyStrandsTheVote', 'record-cleared-early', 'IdleImpliesSettled', 'inv', 'The record is cleared before the pointer replicas are renamed, and a crash leaves them without it.')
)
foreach ($c in $controls) {
    $inv = @(); $prop = @()
    if ($c[3] -eq 'inv') { $inv = @($c[2]) } else { $prop = @($c[2]) }
    if ($inv.Count -eq 0) { $inv = @('TypeOK') }
    Write-Config ('PointerPublication-Control-' + $c[0]) @("Negative control: $($c[4])", "TLC must report $($c[2]) violated.") 'record' $true $inv $prop $true $c[1]
}

# Controls of the writes a transaction makes, beside the writes that are not logged: name, guard,
# control, target, kind, description.
$mixedControls = @(
    @('SettleLooksOnlyAtTheFile-DurablePointerHasBlob', 'record', 'settle-looks-only-at-the-file', 'DurablePointerHasBlob', 'inv', 'A settle fsyncs the record only when the file shows one, so a stale durable record is not seen.'),
    @('SettleLooksOnlyAtTheFile-LivePointerHasBlob', 'record', 'settle-looks-only-at-the-file', 'LivePointerHasBlob', 'inv', 'A settle fsyncs the record only when the file shows one, so a stale durable record is not seen.'),
    @('LoggedWriteSkipsTheRecordGuard-DurablePointerHasBlob', 'record', 'logged-write-skips-the-guard', 'DurablePointerHasBlob', 'inv', 'A transaction''s write of the document does not run the record guard.'),
    @('LoggedWriteSkipsTheRecordGuard-LivePointerHasBlob', 'record', 'logged-write-skips-the-guard', 'LivePointerHasBlob', 'inv', 'A transaction''s write of the document does not run the record guard.')
)
foreach ($c in $mixedControls) {
    $detail = $c[1] -eq 'record'
    Write-Config ('PointerPublication-Control-' + $c[0]) @("Negative control: $($c[5])", "TLC must report $($c[3]) violated.") $c[1] $true @('TypeOK', $c[3]) @() $detail $c[2] $true
}

# Controls of the operations that settle first: name, control, target, kind, description.
$settleControls = @(
    @('WriteSkipsTheGuard', 'write-skips-the-guard', 'IdleImpliesSettled', 'A write does not run the record guard.', $false, $false),
    @('CheckSkipsTheSettle', 'check-skips-the-settle', 'ACheckReadsASettledRecord', 'A precondition check reads without settling a pending record.', $true, $true),
    @('CollectSkipsTheSettle', 'collect-skips-the-settle', 'DurablePointerHasBlob', 'A collection pass runs without settling a pending record.', $true, $true)
)
foreach ($c in $settleControls) {
    Write-Config ('PointerPublication-Control-' + $c[0]) @("Negative control: $($c[3])", "TLC must report $($c[2]) violated.") 'record' $true @('TypeOK', $c[2]) @() $true $c[1] $c[4] $c[5]
}

# A repair and a scrub that do not settle first: the model finds nothing wrong, because each writes only
# the vote's version to a replica that has none.
Write-Config 'PointerPublication-MaintenanceRecordRepairWithoutSettle' @('The "record" guard with a repair that does not settle a pending record first: every', 'invariant and property holds, because a repair writes only the vote''s version to a', 'replica that has none.') 'record' $true $maintInvariants $maintProperties $true 'repair-skips-the-settle' $true $true
Write-Config 'PointerPublication-MaintenanceRecordScrubWithoutSettle' @('The "record" guard with a scrub that does not settle a pending record first: every', 'invariant and property holds, because a scrub writes only the vote''s version to a', 'replica that has none.') 'record' $true $maintInvariants $maintProperties $true 'scrub-skips-the-settle' $true $true

# Witnesses of the record's and the transaction's states: name, description, control, logged writes.
$witnesses = @(
    @('AFailedRecordSyncLeavesTheRecordUnsynced', 'A write''s fsync of its record fails, and the record is in the file and not durable.', 'none', $false),
    @('AStaleRecordIsLeftByAFailedClear', 'A write''s fsync of the clear fails, and the file is empty while the durable record names the version.', 'none', $false),
    @('AWriteFollowsAFailedClear', 'A write begins with a stale durable record and writes its own, when the settle looks only at the file.', 'settle-looks-only-at-the-file', $false),
    @('ALoggedWriteFollowsAFailedClear', 'A transaction writes the document with a stale durable record, when the settle looks only at the file.', 'settle-looks-only-at-the-file', $true),
    @('APreparedTransactionIsRolledForward', 'A recovery rolls a prepared transaction forward.', 'none', $true),
    @('AnOperationSettlesAPendingRecord', 'A check, collection, repair or scrub finds a record and settles it.', 'none', $true, $true),
    @('APointerReplicaIsLost', 'A pointer replica of a settled document is lost.', 'none', $true, $true),
    @('ARepairFillsAMissingReplica', 'A repair or a scrub writes the vote''s version to the replica that was lost.', 'none', $true, $true)
)
foreach ($w in $witnesses) {
    Write-Config ('PointerPublication-Witness-' + $w[0]) @("Witness: $($w[1])", "TLC must report Witness_$($w[0]) violated, which shows it is reachable.") 'record' $true @("Witness_$($w[0])") @() $true $w[2] $w[3] ([bool]$w[4])
}

'wrote {0} configurations' -f (Get-ChildItem $spec -Filter 'PointerPublication*.cfg').Count
