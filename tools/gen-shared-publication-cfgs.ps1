# Copyright (c) Michael Grier.
#
# tools/gen-shared-publication-cfgs.ps1 -- write every crates/canopy/spec/SharedPublication*.cfg
# from tables.
#
# Usage:
#   pwsh tools/gen-shared-publication-cfgs.ps1
#   pwsh tools/gen-shared-publication-cfgs.ps1 -RecoveryUnits
#
# Each configuration states in its header what TLC must report, which tools/run-tlc-all.ps1 reads.
# It replaces only the selected family's configurations; scans and copies must not overlap a run.
# Change a configuration by changing its table, never the generated file;
# regenerating changes no file when the tables have not changed. Output is LF and 7-bit ASCII.

param(
    [switch]$RecoveryUnits,
    [switch]$RawUnits,
    [string]$SpecDirectory = (Join-Path (Split-Path -Parent $PSScriptRoot) 'crates/canopy/spec')
)

$ErrorActionPreference = 'Stop'
$spec = $SpecDirectory
if ($RecoveryUnits -and $RawUnits) { throw 'Select the checked or raw unit family, not both.' }
$unitModel = $true
$RawUnits = -not $RecoveryUnits
$family = if ($RecoveryUnits) { 'SharedPublicationCheck' } else { 'SharedPublication' }
$utf8 = New-Object System.Text.UTF8Encoding($false)
Get-ChildItem $spec -File | Where-Object { $_.Name -eq "$family.cfg" -or $_.Name.StartsWith("$family-") -and $_.Extension -eq '.cfg' } | Remove-Item
Get-ChildItem $spec -File -Filter 'SharedPublicationUnits*.cfg' | Where-Object {
    $_.BaseName -eq 'SharedPublicationUnits' -or $_.BaseName.StartsWith('SharedPublicationUnits-') -or
    $_.BaseName -eq 'SharedPublicationUnitsCheck' -or $_.BaseName.StartsWith('SharedPublicationUnitsCheck-')
} | Remove-Item

# Configurations that take about a minute or more: the sweep keeps them out of the work-item loop.
$milestoneTier = @('SharedPublication-ThreeDocs', 'SharedPublication-ThreeRequests', 'SharedPublication-ResolveThreeRequests',
    'SharedPublication-ThreeWriters', 'SharedPublication-ResolveThreeWriters', 'SharedPublication-Mixed',
    'SharedPublication-MixedResolve', 'SharedPublication', 'SharedPublication-RecordTxn',
    'SharedPublication-Resolve', 'SharedPublication-ResolveTxn',
    'SharedPublication-HealthyPublication', 'SharedPublication-HealthyTransactions',
    'SharedPublication-SharedPublication', 'SharedPublication-SharedTransactions',
    'SharedPublication-DirectoryDebt', 'SharedPublication-UnitMultiWriterPublication',
    'SharedPublication-UnitMultiWriterResolve', 'SharedPublication-UnitMultiWriterTransaction',
    'SharedPublication-NamespaceTransactions',
    'SharedPublication-Control-NoGuard', 'SharedPublication-Control-PreparedTransactionRolledBack',
    'SharedPublication-Control-RecoveryLeavesTheDocumentsRecord',
    'SharedPublication-Control-RecoveryLeavesTheDocumentsResolve',
    'SharedPublication-Control-RecoverySkipsADocument',
    'SharedPublication-Witness-ARecoveryAppliesTheSecondTransactionSwap',
    'SharedPublication-Witness-RecoveryRollsForwardBothKindsOfWrite')

# Positive configurations whose unreduced closure waits for a frozen boundary
# (DESIGN-NOTES "Unreduced closures wait for a frozen boundary").
$boundaryClosure = @('SharedPublication-HealthyPublication', 'SharedPublication-HealthyTransactions',
    'SharedPublication-SharedPublication', 'SharedPublication-SharedTransactions',
    'SharedPublication-UnitMultiWriterPublication', 'SharedPublication-UnitMultiWriterTransaction',
    'SharedPublication-UnitMultiWriterResolve', 'SharedPublication-StaleRecoverySchedule',
    'SharedPublication', 'SharedPublication-Resolve', 'SharedPublication-RecordTxn',
    'SharedPublication-ResolveTxn', 'SharedPublication-Mixed', 'SharedPublication-MixedResolve',
    'SharedPublication-ThreeRequests', 'SharedPublication-ResolveThreeRequests',
    'SharedPublication-ThreeWriters', 'SharedPublication-ResolveThreeWriters', 'SharedPublication-ThreeDocs')

function Write-Config([string]$name, [string[]]$header, [string]$fix, [bool]$txn,
                      [string[]]$invariants, [string[]]$properties = @(), [string]$control = 'none',
                      [int]$procs = 2, [int]$docs = 2, [int]$requests = 2, [int]$terminations = 1,
                      [int]$crashes = 1, [string]$coordControl = 'none', [bool]$mixed = $false,
                      [string]$recoverySchedule = 'all', [bool]$reference = $false,
                      [int]$directoryDebt = 0, [bool]$shared = $false,
                      [string]$preconditionMode = 'unconditional', [bool]$initialNodeRemoved = $false) {
    $baseName = $name
    $name = $name -replace '^SharedPublication', $family
    $lines = @('\* Copyright (c) Michael Grier.')
    foreach ($h in $header) { $lines += '\* ' + $h }
    if ($RawUnits -and $boundaryClosure -contains $baseName) { $lines += '\* Pair closure: boundary.' }
    if ($RawUnits) { $lines += '\* Tier: evidence.' }
    elseif ($milestoneTier -contains $baseName) { $lines += '\* Tier: milestone.' }
    $flag = { param($b) if ($b) { 'TRUE' } else { 'FALSE' } }
    $lines += 'CONSTANTS'
    $lines += "    Procs = {$((1..$procs) -join ', ')}"
    $lines += "    MaxRequests = $(if ($unitModel) { 9 * $requests } else { $requests })"
    if ($unitModel) {
        $lines += "    MaxWrites = $requests"
        $lines += "    RecoverySchedule = `"$recoverySchedule`""
        $lines += "    InitialDirectoryDebt = $directoryDebt"
        $lines += "    InitialNodeRemoved = $(& $flag $initialNodeRemoved)"
        $lines += "    TransactionPreconditions = `"$preconditionMode`""
    }
    $lines += "    MaxTerminations = $terminations"
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
    $lines += "    PublicationsFail = $(& $flag (-not $shared))"
    $lines += "    CrashFences = $(& $flag $shared)"
    $lines += '    UpSteps = 0'
    $lines += '    Callbacks = FALSE'
    $lines += '    Notifications = FALSE'
    $lines += "    Scenario = `"Plain$procs`""
    $lines += "    Control = `"$coordControl`""
    $lines += "    Docs = {$((1..$docs) -join ', ')}"
    $lines += '    NPtr = 3'
    $lines += '    NBlob = 1'
    $lines += "    Fix = `"$fix`""
    $lines += "    PubControl = `"$control`""
    $lines += $(if ($RecoveryUnits -and -not $reference) { 'SPECIFICATION QuotientSpec' } elseif ($RawUnits -and $shared) { 'SPECIFICATION SpSharedSpec' } elseif ($RawUnits) { 'SPECIFICATION SpFileSpec' } else { 'SPECIFICATION SpSpec' })
    if ($unitModel) {
        $lines += 'CONSTRAINT WritesBounded'
        if ($RecoveryUnits -and -not $reference) { $lines += 'VIEW QuotientView' }
    }
    $lines += 'INVARIANTS'
    $lines += ($invariants | ForEach-Object {
        if ($RecoveryUnits -and -not $reference -and $_ -eq 'SpTypeOK') { '    QuotientTypeOK' }
        else { "    $_" }
    })
    if ($RecoveryUnits -and -not $reference) { $lines += '    OnlyOrdinaryRequestsConsumeTheBudget' }
    if ($properties.Count -gt 0) { $lines += 'PROPERTIES'; $lines += ($properties | ForEach-Object { "    $_" }) }
    $lines += 'CHECK_DEADLOCK FALSE'
    $text = ($lines -join "`n") + "`n"
    [System.IO.File]::WriteAllText((Join-Path $spec "$name.cfg"), $text, $utf8)
}

$all = @('SpTypeOK', 'DurablePointerHasBlob', 'LivePointerHasBlob',
         'LivePointersHaveTheirNode', 'DurablePointersHaveTheirNode', 'EveryDocumentHasAMajority',
         'OutcomeIsFixedOnceRecovered', 'AHealthyHolderFindsEveryDocumentSettled')
$props = @('OnlyAHolderChangesADocument')

if ($unitModel) {
    if ($RecoveryUnits) { $all += @('DocumentWorkUsesCurrentGeneration', 'SavedRecoveryCountNeverLeads', 'StoredVersionsNeverLeadTheGeneration') }
    Write-Config 'SharedPublication-HealthyPublication' @('Two writers publish without host or process loss: every invariant and property holds.') 'record' $false $all $props 'none' 2 2 2 0 0
    Write-Config 'SharedPublication-HealthyTransactions' @('Two writers transact without host or process loss: every invariant and property holds.') 'record' $true $all $props 'none' 2 2 2 0 0
    Write-Config 'SharedPublication-SharedPublication' @('Shared coordination publications cannot fail and restart fenced: every invariant and property holds.') 'record' $false $all $props 'none' 2 2 2 1 1 -shared $true
    Write-Config 'SharedPublication-SharedTransactions' @('Shared coordination with transactions: every invariant and property holds.') 'record' $true $all $props 'none' 2 2 2 1 1 -shared $true
    Write-Config 'SharedPublication-UnitMultiWriterPublication' @('Two writers and two documents with recovery units: every invariant and property holds.') 'record' $false $all $props 'none' 2 2 1 1 1
    Write-Config 'SharedPublication-UnitMultiWriterTransaction' @('Two transaction writers and two documents with recovery units: every invariant and property holds.') 'record' $true $all $props 'none' 2 2 1 1 1
    Write-Config 'SharedPublication-UnitMultiWriterResolve' @('Two writers with the resolve guard and recovery units: every invariant and property holds.') 'resolve' $false $all $props 'none' 2 2 1 1 1
    Write-Config 'SharedPublication-UnitControlNoGuard' @('TLC must report EveryDocumentHasAMajority violated.') 'none' $false @('EveryDocumentHasAMajority') @() 'none' 1 1 2 0 0
    Write-Config 'SharedPublication-DirectoryDebt' @('Directory-debt units precede publication recovery: every invariant and property holds.') 'record' $false $all $props 'none' 2 1 1 0 2 -directoryDebt 1
    Write-Config 'SharedPublication-Witness-DirectoryDebtUnitRuns' @('TLC must report Witness_ADirectoryDebtUnitRuns violated.') 'record' $false @('Witness_ADirectoryDebtUnitRuns') @() 'none' 2 1 1 0 1 -directoryDebt 1
    Write-Config 'SharedPublication-Witness-DirectoryClearSurvivesOnlyLive' @('TLC must report Witness_ADirectoryClearSurvivesOnlyLive violated.') 'record' $false @('SpTypeOK') @('Witness_ADirectoryClearSurvivesOnlyLive') 'none' 2 1 1 0 2 -directoryDebt 1
    Write-Config 'SharedPublication-ReferencePublication' @('The bounded unreduced publication reference: every invariant and property holds.') 'record' $false $all $props 'none' 1 1 1 0 0 -reference $true
    Write-Config 'SharedPublication-ReferenceTransaction' @('The bounded unreduced transaction reference: every invariant and property holds.') 'record' $true $all $props 'none' 1 1 1 0 0 -reference $true
    Write-Config 'SharedPublication-PublicationUnits' @('Publication recovery in separate exclusion units: every invariant and property holds.') 'record' $false ($all + @('CountIsOddWhileAnOperationIsUnresolved', 'AFinishedOperationLeavesTheStoreSettled')) $props 'none' 1 1 1 0 0
    Write-Config 'SharedPublication-TransactionUnits' @('Transaction recovery in separate exclusion units: every invariant and property holds.') 'record' $true $all $props 'none' 1 1 1 0 0
    Write-Config 'SharedPublication-NamespaceTransactions' @('One namespace tree supports remove, recreate and recovery: every invariant and property holds.') 'record' $true $all $props 'none' 1 1 2 1 1
    Write-Config 'SharedPublication-Witness-RecoveryCompletesAllUnits' @('TLC must report Witness_ARecoveryCompletesAllUnits violated.') 'record' $true @('Witness_ARecoveryCompletesAllUnits') @() 'none' 1 1 1 0 0
    Write-Config 'SharedPublication-Witness-AWriteCollects' @('TLC must report Witness_AWriteCollects violated.') 'record' $false @('SpTypeOK') @('Witness_AWriteCollects') 'none' 1 1 1 0 0
    Write-Config 'SharedPublication-Witness-WriterRefusedBetweenUnits' @('TLC must report Witness_AWriterIsRefusedBetweenRecoveryUnits violated.') 'record' $false @('Witness_AWriterIsRefusedBetweenRecoveryUnits') @() 'none' 2 1 2 0 0
    Write-Config 'SharedPublication-StaleRecoverySchedule' @('The resumed recovery settles the new record: every invariant and property holds.') 'record' $false $all $props 'none' 2 1 3 0 0 -recoverySchedule 'stale-completion'
    Write-Config 'SharedPublication-Control-StaleRecoverySchedule' @('TLC must report OutcomeIsFixedOnceRecovered violated.') 'record' $false @('OutcomeIsFixedOnceRecovered') @() 'resume-stale-recovery' 2 1 3 0 0 -recoverySchedule 'stale-completion'
    Write-Config 'SharedPublication-Witness-StaleRecoveryRestarts' @('TLC must report Witness_AStaleRecoveryRestarts violated.') 'record' $false @('SpTypeOK') @('Witness_AStaleRecoveryRestarts') 'none' 2 1 3 0 0 -recoverySchedule 'stale-completion'
    foreach ($witness in @('ALiveRollbackCollects', 'ARecoveryCollectsARolledBackTransaction',
                          'ARecoveryCollectsACommittedTransaction', 'ARollbackCollectionFailureRequiresRecovery')) {
        Write-Config "SharedPublication-Witness-$witness" @("TLC must report Witness_$witness violated.") 'record' $true @('SpTypeOK') @("Witness_$witness") 'none' 1 1 1 0 0
    }
    Write-Config 'SharedPublication-Witness-ATransactionPreparesTwoDocuments' @('TLC must report Witness_ATransactionPreparesTwoDocuments violated.') 'record' $true @('SpTypeOK', 'Witness_ATransactionPreparesTwoDocuments') @() 'none' 1 2 1 0 0
    Write-Config 'SharedPublication-Witness-ATransactionPlansDigestPreconditions' @('TLC must report Witness_ATransactionPlansDigestPreconditions violated.') 'record' $true @('SpTypeOK', 'Witness_ATransactionPlansDigestPreconditions') @() 'none' 1 2 1 0 0 -preconditionMode all
    Write-Config 'SharedPublication-Witness-ATransactionAcceptsDigestPreconditions' @('TLC must report Witness_ATransactionAcceptsDigestPreconditions violated.') 'record' $true @('SpTypeOK', 'Witness_ATransactionAcceptsDigestPreconditions') @() 'none' 1 2 1 0 0 -preconditionMode all
    Write-Config 'SharedPublication-Witness-ATransactionRejectsDigestMismatch' @('TLC must report Witness_ATransactionRejectsDigestMismatch violated.') 'record' $true @('SpTypeOK') @('Witness_ATransactionRejectsDigestMismatch') 'none' 1 2 1 0 0 -preconditionMode all
    Write-Config 'SharedPublication-Witness-ATransactionRejectsExpectedAbsent' @('TLC must report Witness_ATransactionRejectsExpectedAbsent violated.') 'record' $true @('SpTypeOK') @('Witness_ATransactionRejectsExpectedAbsent') 'none' 1 2 1 0 0 -preconditionMode all
    Write-Config 'SharedPublication-Witness-ACrashLeavesTwoTransactionSwapsPrepared' @('TLC must report Witness_ACrashLeavesTwoTransactionSwapsPrepared violated.') 'record' $true @('SpTypeOK', 'Witness_ACrashLeavesTwoTransactionSwapsPrepared') @() 'none' 1 2 1 0 1
    Write-Config 'SharedPublication-Witness-ARecoveryAppliesTheSecondTransactionSwap' @('TLC must report Witness_ARecoveryAppliesTheSecondTransactionSwap violated.') 'record' $true @('SpTypeOK', 'Witness_ARecoveryAppliesTheSecondTransactionSwap') @() 'none' 2 2 2 0 1 -shared $true
    Write-Config 'SharedPublication-Witness-AWriteRemoveCollisionIsRejected' @('TLC must report Witness_AWriteRemoveCollisionIsRejected violated.') 'record' $true @('SpTypeOK') @('Witness_AWriteRemoveCollisionIsRejected') 'none' 1 1 1 0 0
    Write-Config 'SharedPublication-Witness-AWriteBeneathRemovedNodeIsRejected' @('TLC must report Witness_AWriteBeneathRemovedNodeIsRejected violated.') 'record' $true @('SpTypeOK') @('Witness_AWriteBeneathRemovedNodeIsRejected') 'none' 1 1 1 0 0
    Write-Config 'SharedPublication-Witness-ARemovalAtExistingDocumentIsRejected' @('TLC must report Witness_ARemovalAtExistingDocumentIsRejected violated.') 'record' $true @('SpTypeOK') @('Witness_ARemovalAtExistingDocumentIsRejected') 'none' 1 1 1 0 0
    Write-Config 'SharedPublication-Witness-ARemovalBeneathExistingDocumentIsRejected' @('TLC must report Witness_ARemovalBeneathExistingDocumentIsRejected violated.') 'record' $true @('SpTypeOK') @('Witness_ARemovalBeneathExistingDocumentIsRejected') 'none' 1 1 1 0 0
    Write-Config 'SharedPublication-Witness-ATransactionRemovesANode' @('TLC must report Witness_ATransactionRemovesANode violated.') 'record' $true @('SpTypeOK', 'Witness_ATransactionRemovesANode') @() 'none' 1 1 1 0 0
    Write-Config 'SharedPublication-Witness-ARecoveryRemovesANode' @('TLC must report Witness_ARecoveryRemovesANode violated.') 'record' $true @('SpTypeOK', 'Witness_ARecoveryRemovesANode') @() 'none' 2 1 1 1 1
    Write-Config 'SharedPublication-Witness-ADirectoryIntentTears' @('TLC must report Witness_ADirectoryIntentTears violated.') 'record' $true @('SpTypeOK') @('Witness_ADirectoryIntentTears') 'none' 1 1 1 0 0 -initialNodeRemoved $true
    Write-Config 'SharedPublication-Witness-ADirectoryIntentIsRepaired' @('TLC must report Witness_ADirectoryIntentIsRepaired violated.') 'record' $true @('SpTypeOK') @('Witness_ADirectoryIntentIsRepaired') 'none' 1 1 1 0 0 -initialNodeRemoved $true
    Write-Config 'SharedPublication-Witness-ADirectoryIsCreatedAndRenamed' @('TLC must report Witness_ADirectoryIsCreatedAndRenamed violated.') 'record' $true @('SpTypeOK', 'Witness_ADirectoryIsCreatedAndRenamed') @() 'none' 1 1 1 0 0 -initialNodeRemoved $true
    Write-Config 'SharedPublication-Witness-ARecoveryCreatesADirectory' @('TLC must report Witness_ARecoveryCreatesADirectory violated.') 'record' $true @('SpTypeOK', 'Witness_ARecoveryCreatesADirectory') @() 'none' 2 1 1 1 1 -initialNodeRemoved $true
}

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
    @('AHolderTerminatesWithARecordPending', 'record', $false, 'A holder dies with a durable record pending.'),
    @('ARestartLosesTheFenceWithARecordPending', 'record', $false, 'A host crash loses the fence while a durable record is pending.')
)
foreach ($w in $witnesses) {
    Write-Config ('SharedPublication-Witness-' + $w[0]) @("Witness: $($w[3])", "TLC must report Witness_$($w[0]) violated, which shows it is reachable.") $w[1] $w[2] @("Witness_$($w[0])")
}

'wrote {0} configurations' -f @(Get-ChildItem $spec -File | Where-Object { $_.Name -eq "$family.cfg" -or $_.Name.StartsWith("$family-") -and $_.Extension -eq '.cfg' }).Count
