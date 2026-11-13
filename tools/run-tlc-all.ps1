# Copyright (c) Michael Grier
#
# tools/run-tlc-all.ps1 -- run every configuration of a TLA+ spec and compare each
# outcome with the one the configuration says it must have.
#
# Usage:
#   pwsh tools/run-tlc-all.ps1 -Spec crates/canopy/spec/Coordination.tla
#   pwsh tools/run-tlc-all.ps1 -Spec <spec.tla> -Filter '*Witness*'
#   pwsh tools/run-tlc-all.ps1 -Spec <spec.tla> -Filter 'Coordination-Txn','*Cumulative*' -FailFast
#   pwsh tools/run-tlc-all.ps1 -Spec <spec.tla> -Tier item
#   pwsh tools/run-tlc-all.ps1 -Spec <spec.tla> -Tier evidence -Filter '*Live*'
#   pwsh tools/run-tlc-all.ps1 -Spec <spec.tla> -Tier evidence -Parallel 6
#   pwsh tools/run-tlc-all.ps1 -Spec <spec.tla> -Filter '*Cumulative*' -List
#   pwsh tools/run-tlc-all.ps1 -Spec <spec.tla> -Changed -List
#   pwsh tools/run-tlc-all.ps1 -Spec <spec.tla> -Changed -FailFast
#
# -Filter takes wildcard patterns on a configuration's name, repeated or comma-separated; a
# configuration runs when any of them matches. -FailFast stops after the first mismatch. -List
# prints what would run, with each tier, and stops.
#
# -Parallel N runs N configurations at once and reports them in the usual order once all have
# run; it cannot be combined with -FailFast. TLC's liveness check uses about one thread per
# branch, so liveness configurations run side by side use cores a single run leaves idle. Each
# run then gets an equal share of the processors as its workers, unless -Workers says otherwise,
# and an equal share of three quarters of physical memory as its heap, unless -HeapGB does.
#
# A configuration whose header carries `Tier: milestone` is slow enough to be kept out of the
# work-item loop, and one whose header carries `Tier: evidence` is so slow that it is run only
# on demand, when a change touches what its finding rests on; every other one is in the item
# tier. -Tier item, milestone or evidence runs that tier, the default (-Tier sweep) runs the
# item and milestone tiers and names the evidence configurations it left out, and -Tier all
# runs every tier. A configuration in the item tier that runs slowly is reported at the end,
# so the tags stay honest.
#
# A spec's configurations are the sibling files <Name>.cfg and <Name>-*.cfg. Each states its
# outcome in its header comment (the leading `\*` lines), in one of three forms:
#
#   TLC must report <Name> violated      a negative control or a witness: TLC must stop
#                                        reporting that invariant or property violated
#   TLC must report a deadlock           a negative control for a wait that never ends: TLC
#                                        must find a state with no successor
#   every invariant and property holds   TLC must finish with no error
#
# The phrase may wrap across header lines. A configuration whose header says neither is listed
# as unchecked: it is run by tools/run-tlc.ps1 as before but nothing here can judge it.
#
# TLC's output for each configuration goes where tools/run-tlc.ps1 puts it,
# .scratch/tlc/<config-name>.out. Exit code is 0 when every checked configuration matched,
# 1 when any did not (including one TLC produced no output for), and 2 when the spec, its
# configurations, java or tla2tools.jar cannot be found. See the "TLA+ / TLC" section of
# .github/copilot-instructions.md for the pinned toolchain.
#
# This is what keeps a negative control honest: a control that stops failing, or a witness
# whose state becomes unreachable, shows up here as a mismatch rather than as a pass.
#
# Every configuration that comes out as its header requires is recorded, with a digest of what
# its outcome depends on: its own text, the TLC jar, and every definition and declaration it
# reaches (tools/lib/Impact.ps1 says how). -Changed runs, of the configurations the filter and the
# tier select, only those whose digest differs from their record, and says why each was selected:
# its .cfg, TLC or a namespace's declarations changed, or the definitions it reaches that did. A
# run without -Changed ignores the record and runs everything it selects. A configuration whose
# outcome is a pass explores its whole state graph, so its distinct-state count is recorded too,
# and a count that differs from the record is reported: an edit meant to leave a configuration
# exploring the same states is checked that way. The record is git-ignored state under
# .scratch/tlc/record; -Record names another file.

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$Spec,
    [string[]]$Filter = '*',
    [ValidateSet('sweep', 'all', 'item', 'milestone', 'evidence')]
    [string]$Tier = 'sweep',
    [switch]$FailFast,
    [switch]$List,
    [switch]$Changed,
    [string]$Record,
    [int]$Parallel = 1,
    [string]$Workers = 'auto',
    [int]$HeapGB = 0,
    [string]$Jar = (Join-Path $env:USERPROFILE '.tla\1.8.0\tla2tools.jar')
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Spec)) {
    Write-Error "spec not found: $Spec" -ErrorAction Continue
    exit 2
}
if (-not (Test-Path -LiteralPath $Jar)) {
    Write-Error "tla2tools.jar not found at $Jar; see .github/copilot-instructions.md" -ErrorAction Continue
    exit 2
}
if (-not (Get-Command java -ErrorAction SilentlyContinue)) {
    Write-Error 'java not found on PATH; see .github/copilot-instructions.md' -ErrorAction Continue
    exit 2
}

$runTlc = Join-Path $PSScriptRoot 'run-tlc.ps1'
$repoRoot = Split-Path -Parent $PSScriptRoot
$specPath = (Resolve-Path -LiteralPath $Spec).Path
$specDir = Split-Path -Parent $specPath
$specName = [System.IO.Path]::GetFileNameWithoutExtension($specPath)
$outDir = Join-Path $repoRoot '.scratch\tlc'
# A configuration in the item tier that takes this long probably belongs in the milestone tier.
$slowSeconds = 60

# pwsh -File hands -Filter 'a','b' over as the one string 'a,b', so split on commas as well.
$patterns = @($Filter | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$configs = @(Get-ChildItem -LiteralPath $specDir -File |
    Where-Object { $_.Name -like "$specName.cfg" -or $_.Name -like "$specName-*.cfg" } |
    Where-Object { $name = $_.BaseName; @($patterns | Where-Object { $name -like $_ }).Count -gt 0 } |
    Sort-Object Name)

if ($configs.Count -eq 0) {
    Write-Error "no configuration of $specName matches '$($patterns -join ', ')' in $specDir" -ErrorAction Continue
    exit 2
}

# A configuration's header comment, the leading `\*` lines, as one line.
function Get-Header([string]$cfgPath) {
    return (Get-Content -LiteralPath $cfgPath |
        Where-Object { $_.StartsWith('\*') } |
        ForEach-Object { $_.Substring(2).Trim() }) -join ' '
}

# The tier a configuration's header gives it; one that gives none is in the item tier.
function Get-Tier([string]$cfgPath) {
    $header = Get-Header $cfgPath
    if ($header -match '\bTier: milestone\b') { return 'milestone' }
    if ($header -match '\bTier: evidence\b') { return 'evidence' }
    return 'item'
}

# The outcome a configuration's header requires, or $null when it states none.
function Get-Expectation([string]$cfgPath) {
    $header = Get-Header $cfgPath
    if ($header -match 'must report a deadlock') { return 'DEADLOCK' }
    if ($header -match 'must report (\S+) violated') { return $Matches[1] }
    if ($header -match 'every invariant and property holds') { return 'PASS' }
    return $null
}

. (Join-Path $PSScriptRoot 'lib/Tlc.ps1')

$skippedEvidence = @()
if ($Tier -eq 'sweep') {
    $skippedEvidence = @($configs | Where-Object { (Get-Tier $_.FullName) -eq 'evidence' })
    $configs = @($configs | Where-Object { (Get-Tier $_.FullName) -ne 'evidence' })
    if ($configs.Count -eq 0) {
        Write-Error "every configuration of $specName matching '$($patterns -join ', ')' is in the evidence tier; run it with -Tier evidence" -ErrorAction Continue
        exit 2
    }
} elseif ($Tier -ne 'all') {
    $configs = @($configs | Where-Object { (Get-Tier $_.FullName) -eq $Tier })
    if ($configs.Count -eq 0) {
        Write-Error "no configuration of $specName in the $Tier tier matches '$($patterns -join ', ')' in $specDir" -ErrorAction Continue
        exit 2
    }
}

. (Join-Path $PSScriptRoot 'lib/Impact.ps1')
$model = Get-ImpactModel $specDir $specName
$recordPath = if ($Record) { $Record } else { Get-RecordPath $repoRoot $specPath 'configs' }
$history = Read-ImpactRecord $recordPath
$footprints = @{}
foreach ($config in $configs) { $footprints[$config.Name] = Get-ConfigFootprint $model $config.FullName $Jar }
$reasons = @{}
if ($Changed) {
    $configs = @(foreach ($config in $configs) {
        $reason = Get-SelectionReason $footprints[$config.Name] $history.Entries[$config.Name] $history.Snapshots
        if ($reason) { $reasons[$config.Name] = $reason; $config }
    })
    if ($configs.Count -eq 0) {
        'selected: 0; every configuration is as it was when it last came out as its header requires'
        foreach ($config in $skippedEvidence) { "not run, evidence tier (-Tier evidence): $($config.Name)" }
        exit 0
    }
}

if ($List) {
    foreach ($config in $configs) {
        if ($Changed) { '{0,-9} {1} -- {2}' -f (Get-Tier $config.FullName), $config.Name, $reasons[$config.Name] }
        else { '{0,-9} {1}' -f (Get-Tier $config.FullName), $config.Name }
    }
    "listed: $($configs.Count)"
    foreach ($config in $skippedEvidence) { "not listed, evidence tier: $($config.Name)" }
    exit 0
}

if ($Parallel -gt 1 -and $FailFast) {
    Write-Error '-Parallel cannot be combined with -FailFast' -ErrorAction Continue
    exit 2
}
$workers = $Workers
$heap = $HeapGB
if ($Parallel -gt 1) {
    if ($workers -eq 'auto') { $workers = [string][math]::Max(1, [math]::Floor([Environment]::ProcessorCount / $Parallel)) }
    if ($heap -le 0) {
        $ramGB = (Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB
        $heap = [int][math]::Max(1, [math]::Floor($ramGB * 0.75 / $Parallel))
    }
}

$mismatched = 0
$ran = 0
$green = @()
$notRun = 0
$slow = @()
$unchecked = @()
$checked = @()
foreach ($config in $configs) {
    $expected = Get-Expectation $config.FullName
    if (-not $expected) { $unchecked += $config.Name; continue }
    $outFile = Join-Path $outDir "$($config.BaseName).out"
    # A stale output from an earlier run must never be mistaken for this run's.
    Remove-Item -LiteralPath $outFile -ErrorAction SilentlyContinue
    $checked += [pscustomobject]@{ Config = $config; Expected = $expected; Out = $outFile; Seconds = 0 }
}

# Each run is a child process, so that one that cannot start is a mismatch, not the end of the sweep.
if ($Parallel -gt 1) {
    $checked | ForEach-Object -ThrottleLimit $Parallel -Parallel {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        & pwsh -NoProfile -File $using:runTlc -Spec $using:specPath -Config $_.Config.FullName -Workers $using:workers -HeapGB $using:heap -Jar $using:Jar *> $null
        $_.Seconds = [int]$clock.Elapsed.TotalSeconds
    }
}
foreach ($item in $checked) {
    $config = $item.Config
    $expected = $item.Expected
    $outFile = $item.Out
    $seconds = $item.Seconds
    if ($Parallel -le 1) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        & pwsh -NoProfile -File $runTlc -Spec $specPath -Config $config.FullName -Workers $workers -HeapGB $heap -Jar $Jar *> $null
        $seconds = [int]$clock.Elapsed.TotalSeconds
    }
    $actual = Get-Outcome $outFile
    $ran++
    $matched = $actual -eq $expected
    if (-not $matched) { $mismatched++ }
    if ($matched) { $green += [pscustomobject]@{ Name = $config.Name; Expected = $expected; Out = $outFile } }
    if ($seconds -ge $slowSeconds -and (Get-Tier $config.FullName) -eq 'item') { $slow += "$($config.Name) ($seconds s)" }
    '{0} {1,-60} expected {2}, got {3} ({4} s)' -f $(if ($matched) { 'ok      ' } else { 'MISMATCH' }),
        $config.Name, $expected, $actual, $seconds
    if ($Changed) { "         selected: $($reasons[$config.Name])" }
    if (-not $matched -and $FailFast) { $notRun = $configs.Count - 1 - [array]::IndexOf($configs, $config); break }
}

$statesChanged = @()
if ($green.Count -gt 0) {
    $snapshot = Add-ImpactSnapshot $history (Get-ModelSnapshot $model $Jar)
    foreach ($item in $green) {
        $footprint = $footprints[$item.Name]
        $states = if ($item.Expected -eq 'PASS') { Get-DistinctStates $item.Out } else { $null }
        $previous = $history.Entries[$item.Name]
        if ($null -ne $states -and $previous -and $null -ne $previous.States -and [long]$previous.States -ne $states) {
            $statesChanged += "$($item.Name) (was $($previous.States), now $states)"
        }
        $history.Entries[$item.Name] = @{ Digest = $footprint.Digest; Cfg = $footprint.Cfg; Snapshot = $snapshot; States = $states }
    }
    Save-ImpactRecord $recordPath $history
}

"checked: $ran, mismatched: $mismatched, unchecked: $($unchecked.Count)"
foreach ($entry in $statesChanged) { "distinct states differ from the record: $entry" }
if ($notRun -gt 0) { "stopped at the first mismatch: $notRun configuration(s) not run" }
foreach ($name in $unchecked) { "unchecked (header states no outcome): $name" }
foreach ($entry in $slow) { "slow for the item tier, consider Tier: milestone: $entry" }
foreach ($config in $skippedEvidence) { "not run, evidence tier (-Tier evidence): $($config.Name)" }
if ($mismatched -gt 0) { exit 1 }
exit 0
