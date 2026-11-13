# Copyright (c) Michael Grier.
#
# tools/gen-creation-cfgs.ps1 -- write every crates/canopy/spec/Creation*.cfg from tables.
#
# Usage:
#   pwsh tools/gen-creation-cfgs.ps1
#
# Each configuration states in its header what TLC must report, which tools/run-tlc-all.ps1 reads.
# It removes every existing Creation*.cfg first, so a configuration dropped from a table leaves the
# spec directory. Change a configuration by changing its table, never the generated file;
# regenerating changes no file when the tables have not changed. Output is LF and 7-bit ASCII.

$ErrorActionPreference = 'Stop'
$spec = Join-Path (Split-Path -Parent $PSScriptRoot) 'crates/canopy/spec'
$utf8 = New-Object System.Text.UTF8Encoding($false)
Get-ChildItem $spec -Filter 'Creation*.cfg' | Remove-Item

$sizes = @{
    OneCreator = 1; TwoCreators = 2; TwoImplementations = 2; DifferentInputs = 2
    ThreeCreators = 3; ThreeDifferent = 3; CreatorAndOpeners = 3; TwoCreatorsAndOpener = 3
    FourCreators = 4
}

$allInvariants = @(
    'TypeOK', 'NeverObservedPartial', 'ReservationPrecedesEveryOtherArtifact',
    'MarkerCompletesTheStore', 'ArtifactsCarryTheReservedInputs', 'OneCreationAtATime',
    'ExclusionMatchesHolders', 'ACreatorUsesTheReservedInputs', 'NoCreationAfterTheMarker',
    'AnOpenedStoreKeepsItsMarker', 'OutcomeMatchesProcess')
$allProperties = @(
    'ReservationIsPublishedOnlyIfAbsent', 'ThePublisherHoldsItsExclusion',
    'AnOpenSucceedsOnlyThroughTheRecordedImplementation', 'ARefusalChangesNothing',
    'CreationBeginsOnlyInAnUnoccupiedRoot',
    'DeathLeavesTheRootAsItWas', 'ACrashKeepsEverythingDurable')

function Write-Config([string]$name, [string[]]$header, [string]$scenario, [int]$art,
                      [int]$deaths, [int]$crashes, [string]$control, [string[]]$invariants,
                      [string[]]$properties, [bool]$deadlock) {
    $n = $sizes[$scenario]
    $procs = (1..$n) -join ', '
    $lines = @('\* Copyright (c) Michael Grier.')
    foreach ($h in $header) { $lines += '\* ' + $h }
    $lines += 'CONSTANTS'
    $lines += "    Procs = {$procs}"
    $lines += "    NArt = $art"
    $lines += "    MaxDeaths = $deaths"
    $lines += "    MaxCrashes = $crashes"
    $lines += "    Intrusion = $(if ($script:intrusion) { 'TRUE' } else { 'FALSE' })"
    $lines += "    Scenario = `"$scenario`""
    $lines += "    Control = `"$control`""
    $lines += 'SPECIFICATION Spec'
    if ($invariants.Count -gt 0) { $lines += 'INVARIANTS'; $lines += ($invariants | ForEach-Object { "    $_" }) }
    if ($properties.Count -gt 0) { $lines += 'PROPERTIES'; $lines += ($properties | ForEach-Object { "    $_" }) }
    $lines += $(if ($deadlock) { 'CHECK_DEADLOCK TRUE' } else { 'CHECK_DEADLOCK FALSE' })
    $text = ($lines -join "`n") + "`n"
    [System.IO.File]::WriteAllText((Join-Path $spec "$name.cfg"), $text, $utf8)
}

# Configurations that must hold.
$script:intrusion = $true
$pass = @(
    @('Creation', 'ThreeCreators', 2, 1, 1, 'The contract with three creators, two implementations, one death, one host crash and', 'something foreign under the root: every invariant and property holds.'),
    @('Creation-OneCreator', 'OneCreator', 3, 1, 2, 'A lone creator through every step of creation, with a death and two host crashes:', 'every invariant and property holds.'),
    @('Creation-TwoCreators', 'TwoCreators', 3, 2, 2, 'Two creators of one implementation with the same inputs, a resumer among them:', 'every invariant and property holds.'),
    @('Creation-TwoImplementations', 'TwoImplementations', 2, 2, 2, 'Two implementations whose creation exclusions do not exclude each other:', 'every invariant and property holds.'),
    @('Creation-DifferentInputs', 'DifferentInputs', 2, 2, 2, 'Two creators of one implementation with different inputs:', 'every invariant and property holds.'),
    @('Creation-ThreeDifferent', 'ThreeDifferent', 2, 1, 1, 'Three creators, two implementations, two sets of inputs:', 'every invariant and property holds.'),
    @('Creation-CreatorAndOpeners', 'CreatorAndOpeners', 2, 2, 2, 'A creator and an opener through each of two implementations:', 'every invariant and property holds.'),
    @('Creation-TwoCreatorsAndOpener', 'TwoCreatorsAndOpener', 2, 2, 1, 'Two creators and an opener of one implementation:', 'every invariant and property holds.'),
    @('Creation-Deep', 'ThreeCreators', 3, 2, 2, 'Three creators with three artifacts, two deaths and two host crashes:', 'every invariant and property holds.'),
    @('Creation-Four', 'FourCreators', 3, 2, 1, 'Four creators, two implementations, two sets of inputs, two deaths and a host crash:', 'every invariant and property holds.')
)
foreach ($c in $pass) {
    Write-Config $c[0] @($c[5], $c[6]) $c[1] $c[2] $c[3] $c[4] 'none' $allInvariants $allProperties $false
}

# No deadlock: nothing gives up or dies, so only every process ended has no successor.
Write-Config 'Creation-NoDeadlock-ThreeCreators' @('No deadlock with three creators, none of which dies, and no host crash: every invariant', 'and property holds, and no state but every process ended has no successor.') 'ThreeCreators' 3 0 0 'none' $allInvariants $allProperties $true
Write-Config 'Creation-NoDeadlock-CreatorAndOpeners' @('No deadlock with a creator and two openers, none of which dies, and no host crash: every invariant', 'and property holds, and no state but every process ended has no successor.') 'CreatorAndOpeners' 3 0 0 'none' $allInvariants $allProperties $true
Write-Config 'Creation-NoDeadlock-WithFailures' @('No deadlock with three creators, a death and a host crash: every invariant and property holds, and no', 'state but every process ended has no successor.') 'ThreeCreators' 2 1 1 'none' $allInvariants $allProperties $true

# Negative controls: name, control, scenario, art, deaths, crashes, target, kind, description.
$script:intrusion = $false
$controls = @(
    @('ReservationInTwoWrites', 'reservation-in-two-writes', 'TwoCreators', 2, 0, 0, 'NeverObservedPartial', 'inv', 'The reservation is published as an empty file and then filled in, and another creator reads it between the two writes.'),
    @('ReservationNotSynced', 'reservation-not-synced', 'OneCreator', 2, 0, 0, 'ReservationPrecedesEveryOtherArtifact', 'inv', 'The artifacts begin before the reservation is durable.'),
    @('BodyNotSyncedBeforeMarker', 'body-not-synced-before-marker', 'OneCreator', 2, 0, 0, 'MarkerCompletesTheStore', 'inv', 'An artifact is not made durable before the next is made, so the marker can outlive the artifact before it.'),
    @('PublishReplaces', 'publish-replaces', 'TwoImplementations', 2, 0, 0, 'ReservationIsPublishedOnlyIfAbsent', 'prop', 'Publishing replaces a reservation that is there instead of finding it.'),
    @('PublishReplacesRunsTwoCreations', 'publish-replaces', 'TwoImplementations', 2, 0, 0, 'OneCreationAtATime', 'inv', 'Publishing replaces a reservation that is there, so creators of two implementations both create.'),
    @('PublishWithoutExclusion', 'publish-without-exclusion', 'OneCreator', 2, 0, 0, 'ThePublisherHoldsItsExclusion', 'prop', 'A creator publishes the reservation without holding its creation exclusion.'),
    @('ResumeWithoutExclusion', 'resume-without-exclusion', 'TwoCreators', 2, 0, 0, 'OneCreationAtATime', 'inv', 'A resumer does not take the creation exclusion, and resumes a creation that is still running.'),
    @('ResumeWithoutExclusionHolders', 'resume-without-exclusion', 'TwoCreators', 2, 0, 0, 'ExclusionMatchesHolders', 'inv', 'A resumer does not take the creation exclusion, so it creates without holding it.'),
    @('ResumeWithoutReread', 'resume-without-reread', 'TwoCreators', 2, 0, 0, 'NoCreationAfterTheMarker', 'inv', 'A resumer that waited for the exclusion does not read the root again, and creates over a store whose creation has finished.'),
    @('ResumeIgnoresInputs', 'resume-ignores-inputs', 'DifferentInputs', 2, 1, 0, 'ACreatorUsesTheReservedInputs', 'inv', 'A resumer does not compare its inputs with the reservation''s, and resumes a creation with different inputs.'),
    @('ResumeIgnoresInputsArtifacts', 'resume-ignores-inputs', 'DifferentInputs', 2, 1, 0, 'ArtifactsCarryTheReservedInputs', 'inv', 'A resumer does not compare its inputs with the reservation''s, and makes the marker from different inputs.'),
    @('OpenIgnoresImplementation', 'open-ignores-implementation', 'CreatorAndOpeners', 2, 0, 0, 'AnOpenSucceedsOnlyThroughTheRecordedImplementation', 'prop', 'An open succeeds through an implementation the reservation does not name.'),
    @('OpenTrustsALiveMarker', 'open-trusts-a-live-marker', 'CreatorAndOpeners', 2, 1, 0, 'AnOpenedStoreKeepsItsMarker', 'inv', 'An open relies on a marker its creator died before making durable, which a host crash can then take away.'),
    @('CreationIgnoresForeignContent', 'creation-ignores-foreign-content', 'OneCreator', 2, 0, 0, 'CreationBeginsOnlyInAnUnoccupiedRoot', 'prop', 'A creator does not refuse a root that holds something creation does not make.')
)
foreach ($c in $controls) {
    $script:intrusion = ($c[0] -eq 'CreationIgnoresForeignContent')
    $inv = @(); $prop = @()
    if ($c[7] -eq 'inv') { $inv = @($c[6]) } else { $prop = @($c[6]) }
    Write-Config ('Creation-Control-' + $c[0]) @("Negative control: $($c[8])", "TLC must report $($c[6]) violated.") $c[2] $c[3] $c[4] $c[5] $c[1] $inv $prop $false
}

# Witnesses: name, scenario, art, deaths, crashes, kind, description.
$witnesses = @(
    @('ACreationCompletes', 'OneCreator', 2, 0, 0, 'inv', 'A creator creates the store.'),
    @('AnInterruptedCreationIsResumed', 'TwoCreators', 2, 1, 1, 'inv', 'A creation that was interrupted is resumed and completes.'),
    @('AResumeFollowsAProcessDeath', 'TwoCreators', 2, 1, 0, 'inv', 'A creation interrupted by the death of its creator is resumed.'),
    @('AResumeFollowsAHostCrash', 'TwoCreators', 2, 0, 1, 'inv', 'A creation interrupted by a host crash is resumed.'),
    @('ACrashLosesTheReservation', 'OneCreator', 2, 0, 1, 'inv', 'A host crash loses a reservation that was not yet durable.'),
    @('ACrashLeavesOnlyTheReservation', 'OneCreator', 2, 0, 1, 'inv', 'A host crash leaves a durable reservation and no other artifact.'),
    @('ACrashLeavesTheArtifactsWithoutTheMarker', 'OneCreator', 2, 0, 1, 'inv', 'A host crash leaves every artifact but the marker.'),
    @('ACreatorWaitsForTheExclusion', 'TwoCreators', 2, 0, 0, 'inv', 'A creator waits for its implementation''s creation exclusion.'),
    @('ACreatorStopsAtAnotherImplementationsReservation', 'TwoImplementations', 2, 0, 0, 'inv', 'A creator finds a reservation that names another implementation and stops.'),
    @('ACreatorStopsAtDifferentInputs', 'DifferentInputs', 2, 0, 0, 'inv', 'A creator finds a reservation made from different inputs and stops.'),
    @('ACreatorLosesTheRace', 'TwoCreators', 2, 0, 0, 'inv', 'A creator finds a reservation when it publishes its own and stops.'),
    @('ACreatorFindsTheStoreCreated', 'TwoCreators', 2, 0, 0, 'prop', 'A creator that starts after the marker is in place stops.'),
    @('AResumerFindsTheStoreCreatedWhileItWaited', 'TwoCreators', 2, 0, 0, 'prop', 'A resumer waits for the exclusion while the creation finishes, and then stops.'),
    @('AnOpenOpens', 'CreatorAndOpeners', 2, 0, 0, 'inv', 'An opener opens the store.'),
    @('AnOpenIsRefusedForAnotherImplementation', 'CreatorAndOpeners', 2, 0, 0, 'inv', 'An opener of another implementation is refused.'),
    @('AnOpenFindsNoStore', 'CreatorAndOpeners', 2, 0, 0, 'inv', 'An opener that reads the root before any creation finds no store.'),
    @('AnOpenFindsACreationInProgress', 'CreatorAndOpeners', 2, 0, 0, 'inv', 'An opener of another implementation finds a reservation and no marker.'),
    @('AnOpenMakesTheMarkerDurable', 'CreatorAndOpeners', 2, 1, 0, 'prop', 'An opener meets a marker whose creator died before making it durable, and makes it durable.'),
    @('ACreatorStopsAtAnOccupiedRoot', 'OneCreator', 2, 0, 0, 'inv', 'A creator finds something creation does not make under the root and stops.'),
    @('ACreatorFindsTheRootOccupiedUnderTheExclusion', 'OneCreator', 2, 0, 0, 'prop', 'Something appears under the root after a creator read it, and the creator stops when it reads it again under the exclusion.'),
    @('AResumerFindsTheRootOccupiedUnderTheExclusion', 'TwoCreators', 2, 1, 0, 'prop', 'Something appears under the root after a resumer read it, and the resumer stops when it reads it again under the exclusion.')
)
foreach ($c in $witnesses) {
    $script:intrusion = ($c[0] -like '*Occupied*')
    $inv = @(); $prop = @()
    if ($c[5] -eq 'inv') { $inv = @("Witness_$($c[0])") } else { $prop = @("Witness_$($c[0])") }
    Write-Config ('Creation-Witness-' + $c[0]) @("Witness: $($c[6])", "TLC must report Witness_$($c[0]) violated, which shows it is reachable.") $c[1] $c[2] $c[3] $c[4] 'none' $inv $prop $false
}
'wrote {0} configurations' -f (Get-ChildItem $spec -Filter 'Creation*.cfg').Count
