# Copyright (c) Michael Grier.
#
# tools/gen-doorbell-cfgs.ps1 -- write every crates/canopy/spec/Doorbell.cfg and Doorbell-*.cfg from
# tables.
#
# Usage:
#   pwsh tools/gen-doorbell-cfgs.ps1
#
# Each configuration states in its header what TLC must report, which tools/run-tlc-all.ps1 reads.
# It removes every existing Doorbell.cfg and Doorbell-*.cfg first, so a configuration dropped from a
# table leaves the spec directory. Change a configuration by changing its table, never the
# generated file; regenerating changes no file when the tables have not changed. Output is LF and
# 7-bit ASCII.

$ErrorActionPreference = 'Stop'
$spec = Join-Path (Split-Path -Parent $PSScriptRoot) 'crates/canopy/spec'
$utf8 = New-Object System.Text.UTF8Encoding($false)
Get-ChildItem $spec -Filter 'Doorbell.cfg' | Remove-Item
Get-ChildItem $spec -Filter 'Doorbell-*.cfg' | Remove-Item

function Write-Config([string]$name, [string[]]$header, [int]$producers, [int]$maxPublish,
                      [string]$control, [string]$specification, [string[]]$invariants,
                      [string[]]$properties = @()) {
    $lines = @('\* Copyright (c) Michael Grier.')
    foreach ($h in $header) { $lines += '\* ' + $h }
    $lines += 'CONSTANTS'
    $lines += "    Producers = {$((1..$producers) -join ', ')}"
    $lines += "    MaxPublish = $maxPublish"
    $lines += "    Control = `"$control`""
    $lines += "SPECIFICATION $specification"
    if ($invariants.Count -gt 0) { $lines += 'INVARIANTS'; $lines += ($invariants | ForEach-Object { "    $_" }) }
    if ($properties.Count -gt 0) { $lines += 'PROPERTIES'; $lines += ($properties | ForEach-Object { "    $_" }) }
    $lines += 'CHECK_DEADLOCK FALSE'
    $text = ($lines -join "`n") + "`n"
    [System.IO.File]::WriteAllText((Join-Path $spec "$name.cfg"), $text, $utf8)
}

$properties = @('ContractRefined', 'WorkIsTaken', 'AWaiterIsWoken')

# The mechanism refines the contract: name, producers, items each, description.
$mains = @(
    @('Doorbell', 2, 2, 'Two producers ringing one consumer'),
    @('Doorbell-OneProducer', 1, 3, 'One producer ringing one consumer'),
    @('Doorbell-ThreeProducers', 3, 2, 'Three producers ringing one consumer, as a completion queue''s engine and coordination notifications do')
)
foreach ($m in $mains) {
    Write-Config $m[0] @("$($m[3]): the mechanism refines the contract, and every", 'invariant and property holds.') $m[1] $m[2] 'none' 'FairSpec' @('TypeOK') $properties
}

# Negative controls: name, control, description.
$controls = @(
    @('ClearBeforeReset', 'clear-before-reset', 'The consumer clears the record before it lowers the signal.'),
    @('SkipCheck', 'skip-check', 'The consumer waits after clearing without checking again.'),
    @('CheckBeforeClear', 'check-before-clear', 'The consumer checks for work before it clears the record.'),
    @('TestBeforePublish', 'test-before-publish', 'A producer tests the record before it publishes its work.')
)
foreach ($c in $controls) {
    Write-Config "Doorbell-Control-$($c[0])" @("Negative control: $($c[2])", 'A wake is lost, so TLC must report AWaiterIsWoken violated.') 2 2 $c[1] 'FairSpec' @('TypeOK') @('AWaiterIsWoken')
}

# Witnesses: name, description.
$witnesses = @(
    @('ARingSkipsItsRaise', 'A ring finds the record set and skips its raise.'),
    @('TheConsumerWaitsWhenAllIsTaken', 'The consumer waits with every producer done and nothing left to take.'),
    @('ABusyConsumerTakesAgainWithoutASystemCall', 'The consumer takes work again without lowering the signal or clearing the record.')
)
foreach ($w in $witnesses) {
    Write-Config "Doorbell-Witness-$($w[0])" @("Witness: $($w[1])", "TLC must report Witness_$($w[0]) violated, which shows it is reachable.") 2 2 'none' 'Spec' @("Witness_$($w[0])")
}

'wrote {0} configurations' -f ((Get-ChildItem $spec -Filter 'Doorbell.cfg').Count + (Get-ChildItem $spec -Filter 'Doorbell-*.cfg').Count)
