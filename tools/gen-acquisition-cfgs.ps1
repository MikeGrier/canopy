# Copyright (c) Michael Grier.
#
# tools/gen-acquisition-cfgs.ps1 -- write every crates/canopy/spec/Acquisition*.cfg from the table
# below.
#
# Usage:
#   pwsh tools/gen-acquisition-cfgs.ps1
#
# Each configuration names a scenario, a specification and the properties it checks, with a header
# that states what TLC must report, read by tools/run-tlc-all.ps1. Every one checks the invariants
# and deadlock: the model has no counters to wrap, so safety and liveness are checked together. It
# removes every existing Acquisition*.cfg first, so a configuration dropped from the table leaves
# the spec directory. Change a configuration by changing the table, never the generated file.

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$spec = Join-Path $root 'crates/canopy/spec'
$utf8 = New-Object System.Text.UTF8Encoding($false)

$invariants = @('TypeOK', 'OneWriterAtATime', 'GateIsExclusive', 'TiersAreExclusive')
# Configurations too slow for the work-item loop. Names are without the extension.
$milestoneTier = @()

function Write-Cfg([string]$name, [string[]]$header, [string]$scenario, [string]$specName,
                   [string[]]$properties, [int]$maxUpgrades = 1, [bool]$exclusiveFirst = $false,
                   [string[]]$checkInvariants = $invariants) {
    $lines = @($header | ForEach-Object { "\* $_" })
    if ($milestoneTier -contains $name) { $lines += '\* Tier: milestone.' }
    $lines += @(
        'CONSTANTS',
        "    Scenario = `"$scenario`"",
        "    MaxUpgrades = $maxUpgrades",
        "    ExclusiveFirst = $(if ($exclusiveFirst) { 'TRUE' } else { 'FALSE' })",
        "SPECIFICATION $specName"
    )
    if ($checkInvariants.Count -gt 0) { $lines += 'INVARIANTS'; $lines += @($checkInvariants | ForEach-Object { "    $_" }) }
    if ($properties.Count -gt 0) { $lines += 'PROPERTIES'; $lines += @($properties | ForEach-Object { "    $_" }) }
    $lines += 'CHECK_DEADLOCK TRUE'
    [IO.File]::WriteAllText((Join-Path $spec "$name.cfg"), (($lines -join "`n") + "`n"), $utf8)
}

Get-ChildItem $spec -Filter 'Acquisition*.cfg' | Remove-Item

# ---- the main configurations: every invariant holds, and the store makes progress ----
Write-Cfg 'Acquisition' @('Under the contract''s fairness, with an upgrader unsafe for readers, a writer and a reader,',
    'the store makes progress and an upgrade holding its tiers completes: every invariant and',
    'property holds.') 'UnsafeWriterReader' 'ContractSpec' @('StoreMakesProgress', 'AnUpgradeHoldingItsTiersCompletes')
Write-Cfg 'Acquisition-Writers3' @('Under the contract''s fairness three writers make progress: every invariant and property',
    'holds.') 'Writers3' 'ContractSpec' @('StoreMakesProgress')
Write-Cfg 'Acquisition-Safe' @('Under the contract''s fairness, with an upgrader safe for readers and a reader, the store',
    'makes progress and the upgrade completes: every invariant and property holds.') 'SafeAndReader' 'ContractSpec' @('StoreMakesProgress', 'AnUpgradeHoldingItsTiersCompletes')
Write-Cfg 'Acquisition-SafeWriter' @('Under the contract''s fairness, with an upgrader safe for readers and a writer, which the',
    'reopened gate lets wait for the writer tier during the upgrade, the store makes progress:',
    'every invariant and property holds.') 'SafeAndWriter' 'ContractSpec' @('StoreMakesProgress', 'AnUpgradeHoldingItsTiersCompletes')

# ---- each kind of request under the contract's fairness ----
Write-Cfg 'Acquisition-WriterPassedOver' @('Under the contract''s fairness a writer can be passed over forever by another writer that',
    'is granted exclusion each time it frees. TLC must report AWaitingWriterIsServed violated.') 'Writers2' 'ContractSpec' @('AWaitingWriterIsServed')
Write-Cfg 'Acquisition-UpgraderPassedOver' @('Under the contract''s fairness an upgrader can be passed over forever by a handle that',
    'takes the gate shared each time it frees. TLC must report AWaitingUpgraderIsServed violated.') 'UnsafeAndReader' 'ContractSpec' @('AWaitingUpgraderIsServed')
Write-Cfg 'Acquisition-AttachServed' @('Under the contract''s fairness a handle that waits to attach is admitted, beside writers',
    'that go on attaching and writing: every invariant and property holds.') 'Writers3' 'ContractSpec' @('AnAttachingHandleIsServed')
Write-Cfg 'Acquisition-AttachServedUpgrade' @('Under the contract''s fairness a handle that waits to attach is admitted beside an',
    'upgrader, a writer and a reader: every invariant and property holds.') 'UnsafeWriterReader' 'ContractSpec' @('AnAttachingHandleIsServed')

# ---- strong fairness on each acquisition, and withholding shared admission ----
Write-Cfg 'Acquisition-FairWriter' @('With strong fairness on each acquisition a waiting writer is served: every invariant and',
    'property holds.') 'Writers3' 'FairSpec' @('AWaitingWriterIsServed')
Write-Cfg 'Acquisition-FairUpgrader' @('With strong fairness on each acquisition an upgrader beside one reader is served: every',
    'invariant and property holds.') 'UnsafeAndReader' 'FairSpec' @('AWaitingUpgraderIsServed')
Write-Cfg 'Acquisition-FairUpgraderTwoReaders' @('With strong fairness on each acquisition an upgrader can still be passed over forever by',
    'two readers whose shared holds of the gate overlap, so the gate is never free. TLC must report',
    'AWaitingUpgraderIsServed violated.') 'UnsafeAndTwoReaders' 'FairSpec' @('AWaitingUpgraderIsServed')
Write-Cfg 'Acquisition-ExclusiveFirst' @('Under the contract''s fairness alone, with shared admission withheld while an upgrader',
    'waits for the gate, an upgrader beside two readers is served: every invariant and property',
    'holds.') 'UnsafeAndTwoReaders' 'ContractSpec' @('AWaitingUpgraderIsServed', 'AnAttachingHandleIsServed') 1 $true
Write-Cfg 'Acquisition-ExclusiveFirstWriter' @('With shared admission withheld while an upgrader waits, an upgrader beside a writer and',
    'a reader is served under the contract''s fairness: every invariant and property holds.') 'UnsafeWriterReader' 'ContractSpec' @('AWaitingUpgraderIsServed', 'StoreMakesProgress') 1 $true

# ---- each fairness condition, dropped: the store stops ----
# Name, specification, scenario, what is dropped.
$unfair = @(
    @('WithoutRelease', 'ContractSpecWithoutRelease', 'Writers2', 'a holder need never release'),
    @('WithoutIO', 'ContractSpecWithoutIO', 'UnsafeAndReader', 'an I/O need never complete'),
    @('WithoutLock', 'ContractSpecWithoutLock', 'Writers2', 'a writer need never be granted exclusion that stays free to it'),
    @('WithoutGateS', 'ContractSpecWithoutGateS', 'Writers2', 'a handle need never be granted the gate shared while it stays grantable'),
    @('WithoutGateX', 'ContractSpecWithoutGateX', 'UnsafeAndReader', 'an upgrader need never be granted the gate exclusively while it stays free'),
    @('WithoutReaderTier', 'ContractSpecWithoutReaderTier', 'Writers2', 'a handle need never be granted the reader tier while it stays grantable'),
    @('WithoutWriterTier', 'ContractSpecWithoutWriterTier', 'Writers2', 'a handle need never be granted the writer tier while it stays grantable')
)
foreach ($u in $unfair) {
    Write-Cfg "Acquisition-Control-$($u[0])" @("Negative control: $($u[3]), so the store need not make progress.",
        'TLC must report StoreMakesProgress violated.') $u[2] $u[1] @('StoreMakesProgress')
}

# ---- witnesses ----
Write-Cfg 'Acquisition-Witness-GrantsGoOnForever' @('Witness: two writers are granted exclusion forever. TLC must report',
    'Witness_GrantsGoOnForever violated, which shows it is reachable.') 'Writers2' 'Spec' @('Witness_GrantsGoOnForever')
Write-Cfg 'Acquisition-Witness-AdmissionsGoOnForever' @('Witness: handles are admitted forever. TLC must report Witness_AdmissionsGoOnForever',
    'violated, which shows it is reachable.') 'Writers2' 'Spec' @('Witness_AdmissionsGoOnForever')
Write-Cfg 'Acquisition-Witness-AnUpgradeHoldsItsTiers' @('Witness: an upgrade unsafe for readers holds its tiers beside a writer and a reader. TLC',
    'must report Witness_AnUpgradeHoldsItsTiers violated, which shows it is reachable.') 'UnsafeWriterReader' 'Spec' @() 1 $false ($invariants + 'Witness_AnUpgradeHoldsItsTiers')

$missing = @($milestoneTier | Where-Object { -not (Test-Path (Join-Path $spec "$_.cfg")) })
if ($missing.Count -gt 0) { throw "the milestone tier names no configuration: $($missing -join ', ')" }
'wrote {0} configurations' -f (Get-ChildItem $spec -Filter 'Acquisition*.cfg').Count
