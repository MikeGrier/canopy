# Copyright (c) Michael Grier.
#
# tools/gen-shared-publication-cfgs.ps1 -- write every crates/canopy/spec/SharedPublication*.cfg
# from tables.
#
# Usage:
#   pwsh tools/gen-shared-publication-cfgs.ps1
#
# Each configuration states in its header what TLC must report, which tools/run-tlc-all.ps1 reads.
# It removes every existing SharedPublication*.cfg first, so a configuration dropped from a table
# leaves the spec directory. Change a configuration by changing its table, never the generated file;
# regenerating changes no file when the tables have not changed. Output is LF and 7-bit ASCII.

$ErrorActionPreference = 'Stop'
$spec = Join-Path (Split-Path -Parent $PSScriptRoot) 'crates/canopy/spec'
$utf8 = New-Object System.Text.UTF8Encoding($false)
Get-ChildItem $spec -Filter 'SharedPublication*.cfg' | Remove-Item

# Configurations that take about a minute or more: the sweep keeps them out of the work-item loop.
$milestoneTier = @('SharedPublication-ThreeDocs', 'SharedPublication-ThreeRequests', 'SharedPublication-ResolveThreeRequests',
    'SharedPublication-ThreeWriters', 'SharedPublication-ResolveThreeWriters', 'SharedPublication-Mixed',
    'SharedPublication-MixedResolve')

function Write-Config([string]$name, [string[]]$header, [string]$fix, [bool]$txn,
                      [string[]]$invariants, [string[]]$properties = @(), [string]$control = 'none',
                      [int]$procs = 2, [int]$docs = 2, [int]$requests = 2, [int]$deaths = 1,
                      [int]$crashes = 1, [string]$coordControl = 'none', [bool]$mixed = $false) {
    $lines = @('\* Copyright (c) Michael Grier.')
    foreach ($h in $header) { $lines += '\* ' + $h }
    if ($milestoneTier -contains $name) { $lines += '\* Tier: milestone.' }
    $flag = { param($b) if ($b) { 'TRUE' } else { 'FALSE' } }
    $lines += 'CONSTANTS'
    $lines += "    Procs = {$((1..$procs) -join ', ')}"
    $lines += "    MaxRequests = $requests"
    $lines += "    MaxDeaths = $deaths"
    $lines += "    MaxCrashes = $crashes"
    $lines += '    MaxReads = 0'
    $lines += '    MaxLeases = 0'
    $lines += '    NSlots = 0'
    $lines += '    MaxListings = 0'
    $lines += '    MaxDiscoveries = 0'
    $lines += '    MaxUpgrades = 0'
    $lines += '    Escapes = FALSE'
    $lines += "    Txn = $(& $flag $txn)"
    $lines += "    UnloggedRecovery = $(& $flag (-not $txn))"
    $lines += "    MixedWrites = $(& $flag $mixed)"
    $lines += '    Cyclic = FALSE'
    $lines += '    CycleMod = 0'
    $lines += '    UpSteps = 0'
    $lines += "    Scenario = `"Plain$procs`""
    $lines += "    Control = `"$coordControl`""
    $lines += "    Docs = {$((1..$docs) -join ', ')}"
    $lines += '    NPtr = 3'
    $lines += '    NBlob = 1'
    $lines += "    Fix = `"$fix`""
    $lines += "    PubControl = `"$control`""
    $lines += 'SPECIFICATION SpSpec'
    $lines += 'INVARIANTS'
    $lines += ($invariants | ForEach-Object { "    $_" })
    if ($properties.Count -gt 0) { $lines += 'PROPERTIES'; $lines += ($properties | ForEach-Object { "    $_" }) }
    $lines += 'CHECK_DEADLOCK FALSE'
    $text = ($lines -join "`n") + "`n"
    [System.IO.File]::WriteAllText((Join-Path $spec "$name.cfg"), $text, $utf8)
}

$all = @('SpTypeOK', 'DurablePointerHasBlob', 'LivePointerHasBlob', 'EveryDocumentHasAMajority',
         'OutcomeIsFixedOnceRecovered', 'AHealthyHolderFindsEveryDocumentSettled')
$props = @('OnlyAHolderChangesADocument')

# Both guards, each with unlogged and with logged writes, two writers, a death and a host crash:
# name, guard, logged, what it is.
$mains = @(
    @('SharedPublication', 'record', $false, 'The "record" guard with unlogged writes'),
    @('SharedPublication-Resolve', 'resolve', $false, 'The "resolve" guard with unlogged writes'),
    @('SharedPublication-RecordTxn', 'record', $true, 'The "record" guard with logged writes'),
    @('SharedPublication-ResolveTxn', 'resolve', $true, 'The "resolve" guard with logged writes')
)
foreach ($m in $mains) {
    Write-Config $m[0] @("$($m[3]) by two writers, one of which may die and the host may", 'crash: every invariant and property holds.') $m[1] $m[2] $all $props
}
Write-Config 'SharedPublication-Mixed' @('The "record" guard with writes that each choose to be a transaction or not, by two', 'writers, one of which may die and the host may crash: every invariant and property holds.') 'record' $false $all $props 'none' 2 2 2 1 1 'none' $true
Write-Config 'SharedPublication-MixedResolve' @('The "resolve" guard with writes that each choose to be a transaction or not, by two', 'writers, one of which may die and the host may crash: every invariant and property holds.') 'resolve' $false $all $props 'none' 2 2 2 1 1 'none' $true
Write-Config 'SharedPublication-Witness-RecoveryRollsForwardBothKindsOfWrite' @('Witness: Under the "record" guard with writes that each choose their form, recovery rolls', 'forward an unlogged write''s record and a transaction in one behaviour.', 'TLC must report Witness_RecoveryRollsForwardBothKindsOfWrite violated, which shows it is reachable.') 'record' $false @('Witness_RecoveryRollsForwardBothKindsOfWrite') @() 'none' 2 2 2 1 1 'none' $true
Write-Config 'SharedPublication-ThreeRequests' @('The "record" guard with unlogged writes by two writers making three requests, one of', 'which may die and the host may crash: every invariant and property holds.') 'record' $false $all $props 'none' 2 2 3
Write-Config 'SharedPublication-ResolveThreeRequests' @('The "resolve" guard with unlogged writes by two writers making three requests, one of', 'which may die and the host may crash: every invariant and property holds.') 'resolve' $false $all $props 'none' 2 2 3
Write-Config 'SharedPublication-ThreeWriters' @('The "record" guard with unlogged writes by three writers, one of which may die and the', 'host may crash: every invariant and property holds.') 'record' $false $all $props 'none' 3
Write-Config 'SharedPublication-ResolveThreeWriters' @('The "resolve" guard with unlogged writes by three writers, one of which may die and the', 'host may crash: every invariant and property holds.') 'resolve' $false $all $props 'none' 3
Write-Config 'SharedPublication-ThreeDocs' @('The "record" guard with unlogged writes to three documents by two writers, one of', 'which may die and the host may crash: every invariant and property holds.') 'record' $false $all $props 'none' 2 3

# Negative controls: name, guard, logged, control, target, kind, description.
$controls = @(
    @('NoGuard', 'none', $false, 'none', 'EveryDocumentHasAMajority', 'inv', 'With no guard, recovery does nothing for the documents, and a write that fails part-way over one that did leaves no majority.'),
    @('OpenSettles', 'record', $false, 'open-settles', 'OnlyAHolderChangesADocument', 'prop', 'A handle settles every record when it attaches.'),
    @('RecoverySkipsADocument', 'record', $false, 'recovery-skips-a-document', 'OutcomeIsFixedOnceRecovered', 'inv', 'Recovery settles the record of the first document only.'),
    @('RecoveryLeavesTheDocumentsRecord', 'record', $false, 'recovery-leaves-the-documents', 'OutcomeIsFixedOnceRecovered', 'inv', 'Under the "record" guard recovery does no work on the documents.'),
    @('RecoveryLeavesTheDocumentsResolve', 'resolve', $false, 'recovery-leaves-the-documents', 'OutcomeIsFixedOnceRecovered', 'inv', 'Under the "resolve" guard recovery does no work on the documents.'),
    @('AbandonedRecordReplacedWithoutTheFence', 'record', $false, 'none', 'AHealthyHolderFindsEveryDocumentSettled', 'inv', 'Under the "record" guard an acquirer that finds an abandoned ownership record replaces it without setting the fence, and the next acquirer after it is not told.', 'replace-without-fence', 3),
    @('AbandonedRecordReplacedWithoutTheFenceResolve', 'resolve', $false, 'none', 'AHealthyHolderFindsEveryDocumentSettled', 'inv', 'Under the "resolve" guard an acquirer that finds an abandoned ownership record replaces it without setting the fence, and the next acquirer after it is not told.', 'replace-without-fence', 3),
    @('RestartIgnored', 'record', $false, 'none', 'AHealthyHolderFindsEveryDocumentSettled', 'inv', 'Under the "record" guard an acquirer does not consult whether the coordination state survived a host restart.', 'ignore-restart'),
    @('RestartIgnoredResolve', 'resolve', $false, 'none', 'AHealthyHolderFindsEveryDocumentSettled', 'inv', 'Under the "resolve" guard an acquirer does not consult whether the coordination state survived a host restart.', 'ignore-restart'),
    @('PreparedTransactionRolledBack', 'record', $true, 'none', 'AHealthyHolderFindsEveryDocumentSettled', 'inv', 'Recovery rolls a prepared transaction back instead of forward from its prepare record.', 'recovery-rolls-back-a-prepared-transaction')
)
foreach ($c in $controls) {
    $inv = @('SpTypeOK'); $prop = @()
    if ($c[5] -eq 'inv') { $inv += $c[4] } else { $prop = @($c[4]) }
    $coord = if ($c.Count -gt 7) { $c[7] } else { 'none' }
    $requests = if ($c.Count -gt 8) { $c[8] } else { 2 }
    Write-Config ('SharedPublication-Control-' + $c[0]) @("Negative control: $($c[6])", "TLC must report $($c[4]) violated.") $c[1] $c[2] $inv $prop $c[3] 2 2 $requests 1 1 $coord
}

# Witnesses: name, guard, logged, description.
$witnesses = @(
    @('ARecoverySettlesARecordOfTheSecondDocument', 'record', $false, 'Recovery rolls forward the record of the second document.'),
    @('ARecoveryResolvesTheSecondDocument', 'resolve', $false, 'Recovery sets a replica of the second document to its majority.'),
    @('ARecoveryRollsATransactionForward', 'record', $true, 'Recovery rolls a prepared transaction forward on its document.'),
    @('AHolderDiesWithARecordPending', 'record', $false, 'A holder dies with a durable record pending.'),
    @('ARestartLosesTheFenceWithARecordPending', 'record', $false, 'A host crash loses the fence while a durable record is pending.')
)
foreach ($w in $witnesses) {
    Write-Config ('SharedPublication-Witness-' + $w[0]) @("Witness: $($w[3])", "TLC must report Witness_$($w[0]) violated, which shows it is reachable.") $w[1] $w[2] @("Witness_$($w[0])")
}

'wrote {0} configurations' -f (Get-ChildItem $spec -Filter 'SharedPublication*.cfg').Count
