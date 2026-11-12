# Copyright (c) Michael Grier.
# The tables and helpers the other parts of tools/gen-coordination-cfgs.ps1 share: the tiers,
# the invariant and property lists, the scenarios, and the functions that write a configuration.
# Configurations that take about a minute or more: the sweep keeps them out of the work-item
# loop (-Tier item) and runs them at the milestone boundary. Names are without the extension.
$milestoneTier = @(
    'Coordination-RecoveryThreeProcs', 'Coordination-ThreeProcsCrashAndRead',
    'Coordination-UpgradeBodyReader', 'Coordination-UpgradeBodyReads', 'Coordination-UpgradeBodyUnsafe',
    'Coordination-RecoveryReads', 'Coordination-Recovery',
    'Coordination-CumulativeLayerPairUnsafeBody', 'Coordination-CumulativeChainBody',
    'Coordination-CumulativeChainFloorBody', 'Coordination-RegistryUpgrade',
    'Coordination-ThreeProcs', 'Coordination-TxnThreeProcs', 'Coordination-AttachmentTwoUpgraders',
    'Coordination-RecoveryUnlogged', 'Coordination-RecoveryMixed'
)

# Configurations that take a quarter of an hour to hours: the sweep leaves them out unless asked
# (-Tier evidence), and they are run when a change touches what their finding rests on -- the
# liveness module, or the steps that take and release writer exclusion, the gate and the tiers.
$evidenceTier = @(
    'Coordination-Live-StoreMakesProgress', 'Coordination-Live-StoreMakesProgressUpgrade',
    'Coordination-Live-AnUpgradeHoldingItsTiersCompletes', 'Coordination-Live-ADrivenRecoveryCompletes',
    'Coordination-Live-AWaitingWriterIsServed', 'Coordination-Live-AnAttachingHandleIsServed',
    'Coordination-Live-AnAttachingHandleIsServedUpgrade',
    'Coordination-Live-AWaitingWriterIsServedFair', 'Coordination-Live-AWaitingUpgraderIsServedFair',
    'Coordination-Live-AWaitingUpgraderIsServedExclusiveFirst'
)

$exclusionInvariants = @('TypeOK', 'AtMostOneHolder', 'LockMatchesHolders', 'OwnerRecordNamesItsHolder',
    'AbandonmentIsNeverForgotten', 'UnresolvedWorkIsNeverInvisible', 'RequestStateMatchesProcess',
    'OutcomeIsJustified', 'CountIsOddWhileAnOperationIsUnresolved', 'AFinishedOperationLeavesTheStoreSettled',
    'GenerationMatchImpliesAnAccurateCache', 'AHolderCacheIsAccurate', 'ACachedSampleIsNeverStale',
    'ReadersAreConsistent', 'PreparedRecordCarriesItsSwapSet', 'ABodyIsStagedBeforeItIsPrepared',
    'StagingIsAlwaysAnnouncedByTheLog', 'TransactionStateMatchesTheLog',
    'AMigrationStepIsNeverUnannounced',
    'NoHandleThatSpeaksOnlyTheSourceHoldsExclusionDuringAnUpgrade',
    'OldestCoversTheDataFloor', 'AnAttachedHandleCanReadTheData', 'TheOtherLayersKeepTheirOrdinaryRules')
$exclusionProperties = @('AbandonedRecordNeverReplacedUnfenced', 'NormalAcquireFindsAHealthyStore', 'ToldOnlyWithCause',
    'NoFalseAbandonmentReport', 'StoreDataChangesOnlyByAHealthyHolder', 'DeathLeavesTheFilesystemAsItWas',
    'CrashLeavesNoLiveProcess', 'EveryRequestEndsInExactlyOneOutcome', 'CountIsRaisedOnlyByItsHolder',
    'EffectsAreBracketedByTheCount', 'FailedOpeningPublicationHasNoEffect',
    'FailedOperationsLeaveTheCountOddAndFence', 'GenerationAdvancesOnEveryAcquisitionAndOnlyThen',
    'SettledReadSawNoPartOfAnOperation', 'TheNamespaceMovesOnlyAfterPrepare',
    'AnEndRecordFollowsEverySwap', 'AResetNeverDiscardsAPreparedTransaction',
    'TheLogChangesOnlyUnderExclusion', 'ANewTransactionStartsFromASettledStore',
    'AFailureBeforePrepareLeavesNothingBehind', 'TheFenceIsClearedOnlyByAFinishedRecovery',
    'TrustIsRestoredOnlyByAFinishedRecovery', 'RecoveryPublishesTheCountOnlyOnceTheStoreIsSettled',
    'RecoveryClearsTheFenceAtItsEnd',
    'DiscoveryReportsRequiredOnlyWithCause', 'DiscoveryReportsAbandonedWorkWhenNobodyHoldsExclusion',
    'DiscoveryMutatesNothing', 'AnUpgraderFindsExclusionFree', 'UpgradeProgressNeverGoesBack',
    'TheVersionAdvancesOnlyAfterEveryStep', 'TheOldestReaderSchemaMovesWithTheSchema',
    'TheIntentClearsOnlyAfterTheSchemaAdvances', 'UpgradeEffectsAreMadeUnderExclusionAndTheCount',
    'AResumptionIsMadeOnlyByAHandleThatSpeaksTheTarget', 'ABodyEndsServedOrRefused',
    'AnUpgradeIsJudgedAgainstTheOldestSchema', 'AnUpgradeOfOneLayerLeavesTheOthersAlone')
# What cumulative reader compatibility adds, for configurations that carry only the attachment lists.
$layerInvariants = @('OldestCoversTheDataFloor', 'AnAttachedHandleCanReadTheData', 'TheOtherLayersKeepTheirOrdinaryRules')
$layerProperties = @('AnUpgradeIsJudgedAgainstTheOldestSchema', 'AnUpgradeOfOneLayerLeavesTheOthersAlone')
$attachInvariants = @('AnExclusiveGateHolderCoexistsWithNoOtherAdmission', 'TierHoldersAreCompatible',
    'OrdinaryAdmissionHoldsTheGateOnlyUntilItHoldsTheReaderTier', 'NoHandleWaitsForTheGateWhileHoldingATier',
    'NothingIsAcquiredAgainstTheOrder', 'ExclusionIsHeldOnlyByAttachedWriters', 'AttachedHandlesStayAdmissible',
    'AttachmentMatchesTheProcess', 'AttachRequestStateMatchesProcess', 'NoLeaseIsHeldOnceAConversionBegins',
    'NoDrainWaitsOnItsOwnCaller', 'AnOperationInFlightHoldsTheTiers', 'EntriesFollowAttachment',
    'AttachedHandlesAreRegisteredOrAnnounced', 'ARegisteredHandleOwnsItsEntry',
    'AnUnregisteredHandleIsAnnounced', 'AFreeClaimShowsTheTokenOfTheLivenessHolder',
    'RegistryLocksAreHeldOnlyByLiveProcesses')
$attachProperties = @('AdmittedHandlesAreAdmissible', 'AttachRequestsEndInExactlyOneOutcome',
    'AnUpgradeBeginsFromTheSchemaItExpects', 'SchemasMoveOnlyWhenAnUpgradeCompletes',
    'AnUpgradeNeverStartsFromInsideAnOperation', 'NoOperationStartsWhileTheHandleConverts',
    'ACancelledDrainLeavesTheHandleAttached', 'ACancelledOrFailedConversionLeavesTheHandleDetached',
    'EntryChangesAreMadeUnderTheClaimLock', 'ACrashDiscardsTheRegistry',
    'EntryInspectionsAreMadeUnderTheClaimLock', 'LiveReportIsJustified',
    'AListingSaysWhenItMayBeIncomplete')

# Scenario: the name the spec gives a set of roles and speaks, since a configuration
# file cannot state a sequence.
$scenarioNames = @{
    'writer,writer|1,1' = 'Plain2'
    'writer,writer,writer|1,1,1' = 'Plain3'
    'reader,writer|1,2' = 'ReaderAndWriter'
    'reader,writer|3,3' = 'ReaderAndWriterTooNew'
    'safeUpgrader,reader|2,1' = 'SafeAndReader'
    'safeUpgrader,reader|2,3' = 'SafeAndTargetReader'
    'unsafeUpgrader,reader|2,1' = 'UnsafeAndReader'
    'safeUpgrader,writer|2,2' = 'SafeAndWriter'
    'safeUpgrader,writer|2,3' = 'SafeAndTargetWriter'
    'unsafeUpgrader,writer|2,2' = 'UnsafeAndWriter'
    'safeUpgrader,safeUpgrader|2,2' = 'TwoSafe'
    'unsafeUpgrader,reader,reader|2,1,1' = 'UnsafeAndTwoReaders'
    'safeUpgrader,writer,reader|2,2,1' = 'SafeWriterReader'
    'unsafeUpgrader,writer,reader|2,2,1' = 'UnsafeWriterReader'
    'safeUpgrader,unsafeUpgrader,writer|2,2,2' = 'SafeUnsafeWriter'
    'safeUpgrader,safeUpgrader,writer|2,2,2' = 'TwoSafeWriter'
    'safeUpgrader,writer,reader|2,3,1' = 'SafeTargetWriterReader'
    'unsafeUpgrader,writer,reader|2,3,1' = 'UnsafeTargetWriterReader'
    'safeUpgrader,writer,writer|2,3,3' = 'SafeTwoTargetWriters'
}
function Upgrade($roles, $speaks) {
    $key = ($roles -join ',') + '|' + ($speaks -join ',')
    if (-not $scenarioNames.ContainsKey($key)) { throw "no scenario $key" }
    return @{ Name = $scenarioNames[$key]; Procs = $roles.Count; Upgraders = @($roles | Where-Object { $_ -like '*Upgrader' }).Count }
}
function Plain($n) { return Upgrade (@('writer') * $n) (@(1) * $n) }

# Scenarios of upgrades in a row or of several layers, whose floors the roles and speaks of a
# one-layer scenario cannot say. Name, processes, upgraders.
$layered = @{
    ChainSafe = @{ Name = 'ChainSafe'; Procs = 3; Upgraders = 2 }
    ChainFloor = @{ Name = 'ChainFloor'; Procs = 3; Upgraders = 2 }
    TwoLayersSafe = @{ Name = 'TwoLayersSafe'; Procs = 3; Upgraders = 1 }
    TwoLayersUnsafe = @{ Name = 'TwoLayersUnsafe'; Procs = 3; Upgraders = 1 }
    TwoLayerUpgradesSafe = @{ Name = 'TwoLayerUpgradesSafe'; Procs = 3; Upgraders = 2 }
    TwoLayerUpgradesUnsafe = @{ Name = 'TwoLayerUpgradesUnsafe'; Procs = 3; Upgraders = 2 }
    TwoLayerUpgradesFloor = @{ Name = 'TwoLayerUpgradesFloor'; Procs = 3; Upgraders = 2 }
}

function Cfg($s, $requests, $deaths, $crashes, $reads, $control, $escapes, $deadlock, $invariants, $properties, $leases = 0, $slots = 0, $listings = 0, $txn = $false, $discoveries = 0, $upSteps = 0, $unloggedRecovery = $false, $mixedWrites = $false, $spec = 'Spec', $cyclic = $false) {
    $n = $s.Procs
    $procs = '{' + ((1..$n) -join ', ') + '}'
    $lines = @(
        'CONSTANTS',
        "    Procs = $procs",
        "    MaxRequests = $requests",
        "    MaxDeaths = $deaths",
        "    MaxCrashes = $crashes",
        "    MaxReads = $reads",
        "    MaxLeases = $leases",
        "    NSlots = $slots",
        "    MaxListings = $listings",
        "    MaxDiscoveries = $discoveries",
        "    MaxUpgrades = $(if ($upSteps -gt 0) { $s.Upgraders } else { 0 })",
        "    Escapes = $(if ($escapes) { 'TRUE' } else { 'FALSE' })",
        "    Txn = $(if ($txn) { 'TRUE' } else { 'FALSE' })",
        "    UnloggedRecovery = $(if ($unloggedRecovery) { 'TRUE' } else { 'FALSE' })",
        "    MixedWrites = $(if ($mixedWrites) { 'TRUE' } else { 'FALSE' })",
        "    Cyclic = $(if ($cyclic) { 'TRUE' } else { 'FALSE' })",
        "    CycleMod = $(if ($cyclic) { 4 } else { 0 })",
        "    UpSteps = $upSteps",
        "    Scenario = `"$($s.Name)`"",
        "    Control = `"$control`"",
        "SPECIFICATION $spec"
    )
    if ($invariants.Count -gt 0) { $lines += 'INVARIANTS'; $lines += ($invariants | ForEach-Object { "    $_" }) }
    if ($properties.Count -gt 0) { $lines += 'PROPERTIES'; $lines += ($properties | ForEach-Object { "    $_" }) }
    $lines += "CHECK_DEADLOCK $(if ($deadlock) { 'TRUE' } else { 'FALSE' })"
    return ($lines -join "`n") + "`n"
}

function Header($lines) {
    return ((@('\* Copyright (c) Michael Grier.') + ($lines | ForEach-Object { "\* $_" })) -join "`n") + "`n"
}

# Where a property is checked: as an invariant or as a temporal property.
function One($kind, $name) {
    if ($kind -eq 'INVARIANTS') { return @(@($name), @()) }
    return @(@(), @($name))
}

function Write-Cfg($file, $header, $text) {
    if ($milestoneTier -contains [IO.Path]::GetFileNameWithoutExtension($file)) { $header += "\* Tier: milestone.`n" }
    if ($evidenceTier -contains [IO.Path]::GetFileNameWithoutExtension($file)) { $header += "\* Tier: evidence.`n" }
    [IO.File]::WriteAllText((Join-Path $spec $file), $header + $text, $utf8)
}
