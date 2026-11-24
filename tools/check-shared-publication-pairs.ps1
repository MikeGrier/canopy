# Copyright (c) Michael Grier.
#
# Check every selected SharedPublicationCheck configuration against its unreduced
# SharedPublication counterpart. Both generated files must promise the same outcome, and
# TLC must produce that same outcome from both models.
#
# An unreduced counterpart whose header carries "Pair closure: boundary." is too large to close
# while the model is revised; without -Closure only its reduced side is checked, and the pair is
# reported as deferred. -Closure runs every unreduced counterpart, for a frozen boundary.
#
# Usage:
#   pwsh tools/check-shared-publication-pairs.ps1 -List
#   pwsh tools/check-shared-publication-pairs.ps1 -Tier item
#   pwsh tools/check-shared-publication-pairs.ps1 -Filter '*Control-NoGuard*' -FailFast
#   pwsh tools/check-shared-publication-pairs.ps1 -Tier all -Closure

[CmdletBinding()]
param(
    [string[]]$Filter = '*',
    [ValidateSet('item', 'milestone', 'all')]
    [string]$Tier = 'item',
    [switch]$FailFast,
    [switch]$List,
    [switch]$Closure,
    [string]$Workers = 'auto',
    [int]$HeapGB = 0,
    [string]$Jar = (Join-Path $env:USERPROFILE '.tla\1.8.0\tla2tools.jar')
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$specDir = Join-Path $repoRoot 'crates/canopy/spec'
$checkSpec = Join-Path $specDir 'SharedPublicationCheck.tla'
$rawSpec = Join-Path $specDir 'SharedPublication.tla'
$runTlc = Join-Path $PSScriptRoot 'run-tlc.ps1'
$outDir = Join-Path $repoRoot '.scratch/tlc'

if (-not (Test-Path -LiteralPath $checkSpec) -or -not (Test-Path -LiteralPath $rawSpec)) {
    Write-Error 'SharedPublicationCheck.tla and SharedPublication.tla are both required' -ErrorAction Continue
    exit 2
}
if (-not (Test-Path -LiteralPath $Jar)) {
    Write-Error "tla2tools.jar not found at $Jar" -ErrorAction Continue
    exit 2
}
if (-not (Get-Command java -ErrorAction SilentlyContinue)) {
    Write-Error 'java not found on PATH' -ErrorAction Continue
    exit 2
}

function Get-Header([string]$path) {
    return (Get-Content -LiteralPath $path |
        Where-Object { $_.StartsWith('\*') } |
        ForEach-Object { $_.Substring(2).Trim() }) -join ' '
}

function Get-Tier([string]$path) {
    $header = Get-Header $path
    if ($header -match '\bTier: milestone\b') { return 'milestone' }
    if ($header -match '\bTier: evidence\b') { return 'evidence' }
    return 'item'
}

function Test-BoundaryClosure([string]$path) {
    return (Get-Header $path) -match '\bPair closure: boundary\.'
}

function Get-Expectation([string]$path) {
    $header = Get-Header $path
    if ($header -match 'must report a deadlock') { return 'DEADLOCK' }
    if ($header -match 'must report (\S+) violated') { return $Matches[1] }
    if ($header -match 'every invariant and property holds') { return 'PASS' }
    return $null
}

. (Join-Path $PSScriptRoot 'lib/Tlc.ps1')

$patterns = @($Filter | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$configs = @(Get-ChildItem -LiteralPath $specDir -File |
    Where-Object { $_.Name -eq 'SharedPublicationCheck.cfg' -or
                   $_.Name.StartsWith('SharedPublicationCheck-') -and $_.Extension -eq '.cfg' } |
    Where-Object {
        $name = $_.BaseName
        @($patterns | Where-Object { $name -like $_ }).Count -gt 0
    } |
    Where-Object { $Tier -eq 'all' -or (Get-Tier $_.FullName) -eq $Tier } |
    Sort-Object Name)

if ($configs.Count -eq 0) {
    Write-Error "no SharedPublicationCheck configuration in tier '$Tier' matches '$($patterns -join ', ')'" -ErrorAction Continue
    exit 2
}

$pairs = foreach ($check in $configs) {
    $rawName = $check.Name -replace '^SharedPublicationCheck', 'SharedPublication'
    $rawPath = Join-Path $specDir $rawName
    if (-not (Test-Path -LiteralPath $rawPath)) {
        Write-Error "missing unreduced counterpart for $($check.Name): $rawName" -ErrorAction Continue
        exit 2
    }
    $expectedCheck = Get-Expectation $check.FullName
    $expectedRaw = Get-Expectation $rawPath
    if (-not $expectedCheck -or -not $expectedRaw) {
        Write-Error "a paired configuration has no outcome in its header: $($check.Name)" -ErrorAction Continue
        exit 2
    }
    [pscustomobject]@{
        Check = $check
        Raw = Get-Item -LiteralPath $rawPath
        ExpectedCheck = $expectedCheck
        ExpectedRaw = $expectedRaw
        Deferred = -not $Closure -and (Test-BoundaryClosure $rawPath)
    }
}

if ($List) {
    foreach ($pair in $pairs) {
        '{0,-9} {1} <=> {2}{3}' -f (Get-Tier $pair.Check.FullName), $pair.Check.Name, $pair.Raw.Name,
            $(if ($pair.Deferred) { ' (closure deferred)' } else { '' })
    }
    "listed: $($pairs.Count) pairs"
    exit 0
}

$mismatched = 0
$ran = 0
$deferred = 0
$notRun = 0
foreach ($pair in $pairs) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $checkOut = Join-Path $outDir ($pair.Check.BaseName + '.out')
    $rawOut = Join-Path $outDir ($pair.Raw.BaseName + '.out')
    Remove-Item -LiteralPath $checkOut, $rawOut -ErrorAction SilentlyContinue

    & pwsh -NoProfile -File $runTlc -Spec $checkSpec -Config $pair.Check.FullName -Workers $Workers -HeapGB $HeapGB -Jar $Jar *> $null
    $checkActual = Get-Outcome $checkOut
    if ($pair.Deferred) {
        $rawActual = 'deferred'
        $deferred++
        $matched = $pair.ExpectedCheck -eq $pair.ExpectedRaw -and $checkActual -eq $pair.ExpectedCheck
    } else {
        & pwsh -NoProfile -File $runTlc -Spec $rawSpec -Config $pair.Raw.FullName -Workers $Workers -HeapGB $HeapGB -Jar $Jar *> $null
        $rawActual = Get-Outcome $rawOut
        $matched = $pair.ExpectedCheck -eq $pair.ExpectedRaw -and
                   $checkActual -eq $pair.ExpectedCheck -and
                   $rawActual -eq $pair.ExpectedRaw -and
                   $checkActual -eq $rawActual
    }
    if (-not $matched) { $mismatched++ }
    $ran++
    '{0} {1,-52} expected {2}; reduced {3}, raw {4} ({5} s)' -f
        $(if ($matched) { 'ok      ' } else { 'MISMATCH' }),
        $pair.Check.BaseName, $pair.ExpectedCheck, $checkActual, $rawActual,
        [int]$clock.Elapsed.TotalSeconds
    if (-not $matched -and $FailFast) {
        $notRun = $pairs.Count - $ran
        break
    }
}

"checked: $ran, mismatched: $mismatched, unreduced closure deferred: $deferred"
if ($notRun -gt 0) { "stopped at the first mismatch: $notRun pair(s) not run" }
if ($mismatched -gt 0) { exit 1 }
exit 0
