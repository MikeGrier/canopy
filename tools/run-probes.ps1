# Copyright (c) Michael Grier.
#
# tools/run-probes.ps1 -- probe a TLA+ spec: break one rule at a time and check that the property
# written for the rule notices.
#
# Usage:
#   pwsh tools/run-probes.ps1 -Spec crates/canopy/spec/Coordination.tla
#   pwsh tools/run-probes.ps1 -Spec <spec.tla> -Filter 'die-*,*upgrade*' -Parallel 6
#   pwsh tools/run-probes.ps1 -Spec <spec.tla> -Check
#   pwsh tools/run-probes.ps1 -Spec <spec.tla> -List
#
# A probe is a mutation test of a model: a scratch copy of the spec with one rule broken by one
# text edit, run in one configuration against the one property written for the rule. The mutants
# of a spec are the .psd1 files in <spec dir>/probes/<spec name>/, one hash table with a Mutants
# list each, and a mutant has these fields:
#
#   Name       a unique, descriptive name for the rule it breaks
#   Base       the configuration it runs in, a name without .cfg or a list of them
#   Kind       INVARIANTS or PROPERTIES: where Property is checked
#   Property   the one property the run checks; without it the run is the base configuration
#              as it is and any violation counts
#   Expect     'violated' (the default) or 'passes'
#   Overrides  constants of the base configuration to set, as @{ Name = 'text' }
#   Def        the definition the edit is in; Old is looked for only in its body
#   Old, New   the text the edit replaces and the text that replaces it; without them the run is
#              the spec as it is, in the base configuration restricted to Property
#   Edits      for a rule broken in more than one place, a list of @{ Old; New; Def } edits in
#              place of Old, New and Def; each is matched on its own and all are applied
#
# A mutant is CAUGHT when TLC reports Property violated, so the check can fail; it SURVIVED when
# TLC finds no error, which shows a check that cannot fail for that rule. A mutant that says
# Expect = 'passes' is a rule nothing checks, kept on record: it is EQUIVALENT while TLC finds no
# error and UNEXPECTED when it does.
#
# Mutants stay current by how Old is matched. Old must occur exactly once in the spec, or in
# the body of Def when that is given, and the contents of an UNCHANGED tuple are not compared: a
# variable added to a frame leaves a mutant as it was, and where Old and New differ in a tuple
# the variables New drops are removed from the spec's own tuple and the ones it adds are added to
# it. A place whose tuple lacks a variable New drops is not an occurrence, so an edit of a frame
# alone is found by the variables it changes and by its Def. A mutant whose text no longer
# matches is STALE. It is reported before any TLC run and fails the run, to be re-expressed
# against the current text or retired on purpose; it is never skipped. -Check finds the stale
# ones without running TLC.
#
# An edit that changes one component of a derived tuple (coord, store, reader) names the tuple's
# other components in its frame, so it breaks when the tuple gains a component: TLC reports the
# variable as not defined and the run is an ERROR, to be re-expressed like a stale mutant.
#
# TLC's output for a mutant goes where tools/run-tlc.ps1 puts it, .scratch/tlc/probe-<name>.out,
# and its scratch copy of the spec is under .scratch/probes/<name>, beside unchanged copies of the
# modules in the spec directory that the spec extends or instances. Exit code is 0 when every
# probe came out as it should, 1 when any was stale, survived, was unexpected or could not be
# judged, and 2 when the spec, its mutants, java or tla2tools.jar cannot be found. See the
# "TLA+ / TLC" section of .github/copilot-instructions.md for when probes run.

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$Spec,
    [string[]]$Filter = '*',
    [switch]$Check,
    [switch]$List,
    [int]$Parallel = 1,
    [string]$Workers = '',
    [string]$Jar = (Join-Path $env:USERPROFILE '.tla\1.8.0\tla2tools.jar')
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib/Tlc.ps1')
. (Join-Path $PSScriptRoot 'lib/Probes.ps1')

# The one place this tool writes its report; only the script's own flow calls it, so that a
# function's return value is never mixed with report lines.
function Report([string]$line) { Write-Output $line }

# The configuration text of a probe: the base configuration without its comments, with the
# constants the mutant overrides, restricted to the one property when the mutant names one.
function New-ProbeConfig([string]$specDir, [string]$base, $mutant) {
    $path = Join-Path $specDir "$base.cfg"
    if (-not (Test-Path -LiteralPath $path)) { throw "base configuration $base not found" }
    $lines = @(Get-Content -LiteralPath $path | Where-Object { -not $_.StartsWith('\*') })
    $overrides = $mutant.Overrides
    $seen = @{}
    $result = @()
    foreach ($line in $lines) {
        $constant = [regex]::Match($line, '^\s+(\w+) = ')
        if ($overrides -and $constant.Success -and $overrides.ContainsKey($constant.Groups[1].Value)) {
            $name = $constant.Groups[1].Value
            $result += "    $name = $($overrides[$name])"
            $seen[$name] = $true
        } else {
            $result += $line
        }
    }
    if ($overrides) {
        foreach ($key in $overrides.Keys) { if (-not $seen.ContainsKey($key)) { throw "$base has no constant $key to override" } }
    }
    if ($mutant.Property) {
        $head = @()
        foreach ($line in $result) { $head += $line; if ($line -match '^SPECIFICATION\s') { break } }
        $result = $head + @($mutant.Kind, "    $($mutant.Property)", 'CHECK_DEADLOCK FALSE')
    }
    return ($result -join "`n") + "`n"
}

# What an outcome means for a run: the verdict word and what to say about it.
function Get-Verdict($run, [string]$outcome) {
    $mutant = $run.Mutant
    $expect = if ($mutant.Expect) { $mutant.Expect } else { 'violated' }
    if ($outcome -in 'NO-OUTPUT', 'OTHER') { return [pscustomobject]@{ Word = 'ERROR'; Detail = "TLC output: $outcome" } }
    if ($expect -eq 'passes') {
        if ($outcome -eq 'PASS') { return [pscustomobject]@{ Word = 'EQUIVALENT'; Detail = 'TLC found no error, as expected' } }
        return [pscustomobject]@{ Word = 'UNEXPECTED'; Detail = "TLC reported $outcome; the mutant was expected to pass" }
    }
    if ($outcome -eq 'PASS') { return [pscustomobject]@{ Word = 'SURVIVED'; Detail = 'TLC found no error' } }
    if ($mutant.Property -and $outcome -ne $mutant.Property) {
        return [pscustomobject]@{ Word = 'UNEXPECTED'; Detail = "TLC reported $outcome, not $($mutant.Property)" }
    }
    return [pscustomobject]@{ Word = 'CAUGHT'; Detail = "TLC reported $outcome" }
}

if (-not (Test-Path -LiteralPath $Spec)) {
    Write-Error "spec not found: $Spec" -ErrorAction Continue
    exit 2
}
$repoRoot = Split-Path -Parent $PSScriptRoot
$specPath = (Resolve-Path -LiteralPath $Spec).Path
$specDir = Split-Path -Parent $specPath
$specName = [System.IO.Path]::GetFileNameWithoutExtension($specPath)
$corpusDir = Join-Path $specDir "probes/$specName"
if (-not (Test-Path -LiteralPath $corpusDir)) {
    Write-Error "no mutants for ${specName}: $corpusDir not found" -ErrorAction Continue
    exit 2
}
$running = -not ($Check -or $List)
if ($running -and -not (Test-Path -LiteralPath $Jar)) {
    Write-Error "tla2tools.jar not found at $Jar; see .github/copilot-instructions.md" -ErrorAction Continue
    exit 2
}
if ($running -and -not (Get-Command java -ErrorAction SilentlyContinue)) {
    Write-Error 'java not found on PATH; see .github/copilot-instructions.md' -ErrorAction Continue
    exit 2
}

$mutants = @()
foreach ($file in (Get-ChildItem -LiteralPath $corpusDir -Filter '*.psd1' | Sort-Object Name)) {
    foreach ($mutant in @((Import-PowerShellDataFile -LiteralPath $file.FullName).Mutants)) {
        $mutant['File'] = $file.Name
        $mutants += $mutant
    }
}
$problems = @()
$names = @{}
foreach ($mutant in $mutants) {
    if (-not $mutant.Name) { $problems += "$($mutant.File): a mutant has no Name"; continue }
    if ($names.ContainsKey($mutant.Name)) { $problems += "$($mutant.File): $($mutant.Name) is a duplicate name" }
    $names[$mutant.Name] = $true
    if (-not $mutant.Base) { $problems += "$($mutant.Name): no Base" }
    if ($mutant.Property -and $mutant.Kind -notin 'INVARIANTS', 'PROPERTIES') { $problems += "$($mutant.Name): Kind must be INVARIANTS or PROPERTIES" }
    if ($mutant.Expect -and $mutant.Expect -notin 'violated', 'passes') { $problems += "$($mutant.Name): Expect must be violated or passes" }
    if ($mutant.ContainsKey('Old') -ne $mutant.ContainsKey('New')) { $problems += "$($mutant.Name): Old and New go together" }
    if ($mutant.ContainsKey('Edits')) {
        if ($mutant.ContainsKey('Old') -or $mutant.ContainsKey('New') -or $mutant.ContainsKey('Def')) { $problems += "$($mutant.Name): Edits takes the place of Old, New and Def" }
        foreach ($edit in @($mutant.Edits)) {
            if (-not ($edit.ContainsKey('Old') -and $edit.ContainsKey('New'))) { $problems += "$($mutant.Name): every edit has an Old and a New" }
        }
    }
}
if ($problems.Count -gt 0) {
    foreach ($problem in $problems) { Write-Error $problem -ErrorAction Continue }
    exit 2
}

# pwsh -File hands -Filter 'a','b' over as the one string 'a,b', so split on commas as well.
$patterns = @($Filter | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$runs = @(foreach ($mutant in $mutants) {
    foreach ($base in @($mutant.Base)) {
        $name = if (@($mutant.Base).Count -gt 1) { "$($mutant.Name)@$base" } else { $mutant.Name }
        [pscustomobject]@{ Mutant = $mutant; Base = $base; Name = $name }
    }
})
$runs = @($runs | Where-Object { $run = $_; @($patterns | Where-Object { $run.Name -like $_ -or $run.Mutant.Name -like $_ }).Count -gt 0 })
if ($runs.Count -eq 0) {
    Write-Error "no mutant of $specName matches '$($patterns -join ', ')' in $corpusDir" -ErrorAction Continue
    exit 2
}

if ($List) {
    foreach ($run in $runs) {
        Report ('{0,-50} {1,-9} {2}' -f $run.Name, $(if ($run.Mutant.Expect) { $run.Mutant.Expect } else { 'violated' }),
            $(if ($run.Mutant.Property) { $run.Mutant.Property } else { '(any property)' }))
    }
    Report "listed: $($runs.Count)"
    exit 0
}

$tally = [ordered]@{ caught = 0; equivalent = 0; survived = 0; unexpected = 0; stale = 0; error = 0 }
$chain = Get-Chain $specDir $specName
$dependencies = Get-Dependencies $specDir $chain
$definitions = @{}
foreach ($module in $chain.Keys) { $definitions[$module] = Get-Definitions $chain[$module] }

$prepared = @()
foreach ($run in $runs) {
    $edits = @()
    if ($run.Mutant.ContainsKey('Edits')) { $edits = @($run.Mutant.Edits) }
    elseif ($run.Mutant.ContainsKey('Old')) { $edits = @(@{ Old = $run.Mutant.Old; New = $run.Mutant.New; Def = $run.Mutant.Def }) }
    $found = @()
    $problem = $null
    foreach ($edit in $edits) {
        $old = ([string]$edit.Old).Replace("`r`n", "`n")
        $hit = Find-Edit $chain $definitions $old ([string]$edit.New).Replace("`r`n", "`n") $edit.Def
        if (-not $hit.Ok) { $problem = '{0}; {1}' -f $hit.Reason, (Get-StaleHint $chain $old); break }
        $found += $hit
    }
    if (-not $problem) {
        $sorted = @($found | Sort-Object Module, Index)
        for ($i = 1; $i -lt $sorted.Count; $i++) {
            if ($sorted[$i].Module -eq $sorted[$i - 1].Module -and $sorted[$i].Index -lt $sorted[$i - 1].Index + $sorted[$i - 1].Length) { $problem = 'its edits overlap' }
        }
    }
    if ($problem) {
        Report ('{0,-10} {1,-50} {2}' -f 'STALE', $run.Name, $problem)
        $tally.stale++
        continue
    }
    $prepared += [pscustomobject]@{ Run = $run; Edits = $found }
}

if ($Check) {
    Report "checked: $($runs.Count), stale: $($tally.stale)"
    if ($tally.stale -gt 0) { exit 1 }
    exit 0
}

$utf8 = New-Object System.Text.UTF8Encoding($false)
$outDir = Join-Path $repoRoot '.scratch/tlc'
$work = Join-Path $repoRoot '.scratch/probes'
$runTlc = Join-Path $PSScriptRoot 'run-tlc.ps1'
$workers = if ($Workers) { $Workers } elseif ($Parallel -gt 1) { '1' } else { 'auto' }

$ready = @()
foreach ($item in $prepared) {
    $run = $item.Run
    try {
        $dir = Join-Path $work $run.Name
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        foreach ($module in $chain.Keys) {
            $text = $chain[$module]
            foreach ($edit in @($item.Edits | Where-Object { $_.Module -eq $module } | Sort-Object Index -Descending)) {
                $text = $text.Substring(0, $edit.Index) + $edit.Replacement + $text.Substring($edit.Index + $edit.Length)
            }
            [IO.File]::WriteAllText((Join-Path $dir $module), $text, $utf8)
        }
        foreach ($module in $dependencies.Keys) { Copy-Item -LiteralPath $dependencies[$module] -Destination (Join-Path $dir $module) -Force }
        $cfgName = "probe-$($run.Name)"
        [IO.File]::WriteAllText((Join-Path $dir "$cfgName.cfg"), (New-ProbeConfig $specDir $run.Base $run.Mutant), $utf8)
        $outFile = Join-Path $outDir "$cfgName.out"
        # A stale output from an earlier run must never be mistaken for this run's.
        Remove-Item -LiteralPath $outFile -ErrorAction SilentlyContinue
        $ready += [pscustomobject]@{ Run = $run; Spec = (Join-Path $dir "$specName.tla"); Cfg = (Join-Path $dir "$cfgName.cfg"); Out = $outFile }
    } catch {
        Report ('{0,-10} {1,-50} {2}' -f 'ERROR', $run.Name, $_.Exception.Message)
        $tally.error++
    }
}

# Each run is a child process, so that one that cannot start is an error and not the end of the run.
$finished = @()
if ($Parallel -gt 1) {
    $ready | ForEach-Object -ThrottleLimit $Parallel -Parallel {
        & pwsh -NoProfile -File $using:runTlc -Spec $_.Spec -Config $_.Cfg -Workers $using:workers -Jar $using:Jar *> $null
    }
}
foreach ($item in $ready) {
    if ($Parallel -le 1) {
        & pwsh -NoProfile -File $runTlc -Spec $item.Spec -Config $item.Cfg -Workers $workers -Jar $Jar *> $null
    }
    $verdict = Get-Verdict $item.Run (Get-Outcome $item.Out)
    Report ('{0,-10} {1,-50} {2}' -f $verdict.Word, $item.Run.Name, $verdict.Detail)
    $tally[$verdict.Word.ToLowerInvariant()]++
}

Report ('probes: {0}; {1}' -f $runs.Count, (($tally.Keys | ForEach-Object { "$_ $($tally[$_])" }) -join ', '))
if ($tally.survived + $tally.unexpected + $tally.stale + $tally.error -gt 0) { exit 1 }
exit 0
