# Copyright (c) Michael Grier.
#
# tools/gen-timeline-replays.ps1 -- write the TLC replays of the timelines in
# crates/canopy/DESIGN-NOTES.md from their scenario tables.
#
# Usage:
#   pwsh tools/gen-timeline-replays.ps1
#   pwsh tools/run-tlc-all.ps1 -Spec crates/canopy/spec/ReplayCoordination.tla
#
# A timeline is replayed against the current spec so that it cannot go on describing a behaviour
# the model no longer has (DESIGN-NOTES "A timeline is replayed by TLC from a scenario table"). Its
# scenario table is crates/canopy/spec/timelines/<Spec>/<Timeline>.psd1, a hash table with:
#
#   Timeline   the timeline's number, such as T4
#   Base       the configuration of <Spec> the timeline was read from; the replay takes its
#              constants
#   Overrides  constants of Base to set, as @{ Name = 'text' } (optional)
#   Before     the actions that bring the model to the state the timeline starts from, such as the
#              attachments its status line says have happened (optional)
#   Steps      the timeline's steps in order, each @{ Id = 'REQ1'; Actions = @('Request(1)') }; a
#              step that is a state rather than an event, such as a deadlock, has no actions.
#              Actions the model needs and the timeline leaves out -- a handle its status line
#              says has attached, where the current model needs that attachment later -- are a
#              step of their own marked Omitted = $true, named for what they do, and the table's
#              Note says why
#   Projected  the ids of steps the timeline marks projected that the replay takes (optional)
#   Note       anything else the table's reader needs (optional)
#
# An action is written as TLC names it in a trace, with its arguments, or as any action expression
# of the spec; tools/trace-to-scenario.ps1 drafts a table from a counterexample. For each spec with
# tables this writes crates/canopy/spec/Replay<Spec>.tla, which extends the spec with a step
# counter and a next-state relation that allows only the timeline's next action, and one
# Replay<Spec>-<Timeline>.cfg per table, a witness that must report the timeline's end reached;
# each timeline has a specification of its own there, so its configuration reaches only the
# actions it names and an edit to an action selects only the replays that take it. A
# replay that stops early finds no error, which tools/run-tlc-all.ps1 reports as a mismatch, and
# TLC's "depth of the complete state graph search" is then one more than the number of actions it
# took, which the comments in the replay module turn into the step that failed.
#
# Generated files are written with LF endings and only when their text changes, so regenerating
# changes no file when no table changed.

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$specDir = Join-Path $repoRoot 'crates/canopy/spec'
$tableRoot = Join-Path $specDir 'timelines'

# What each spec calls its initial predicate and its variables, and a conjunct every step of it
# carries beside its action.
$specs = [ordered]@{
    Coordination       = @{ Init = 'Init'; Vars = 'vars'; Step = $null }
    Creation           = @{ Init = 'Init'; Vars = 'vars'; Step = $null }
    PointerPublication = @{ Init = 'Init'; Vars = 'vars'; Step = $null }
    SharedPublication  = @{ Init = 'SpInit'; Vars = 'spvars'; Step = 'Kinds' }
}

$utf8 = New-Object System.Text.UTF8Encoding($false)
$written = 0
function Write-Generated([string]$path, [string]$text) {
    $text = $text.Replace("`r`n", "`n")
    if ((Test-Path -LiteralPath $path) -and [IO.File]::ReadAllText($path).Replace("`r`n", "`n") -eq $text) { return }
    [IO.File]::WriteAllText($path, $text, $utf8)
    $script:written++
    "wrote $([IO.Path]::GetFileName($path))"
}

# The CONSTANTS block of a configuration, with the overrides applied.
function Get-Constants([string]$cfgPath, $overrides, [string]$where) {
    $lines = @(Get-Content -LiteralPath $cfgPath)
    $start = [array]::IndexOf($lines, 'CONSTANTS')
    if ($start -lt 0) { throw "${where}: $cfgPath has no CONSTANTS block" }
    $block = @()
    $seen = @{}
    for ($i = $start + 1; $i -lt $lines.Count -and $lines[$i] -match '^\s+(\w+) = '; $i++) {
        $name = $Matches[1]
        if ($overrides -and $overrides.ContainsKey($name)) { $block += "    $name = $($overrides[$name])"; $seen[$name] = $true }
        else { $block += $lines[$i] }
    }
    if ($overrides) { foreach ($key in $overrides.Keys) { if (-not $seen.ContainsKey($key)) { throw "${where}: $cfgPath has no constant $key" } } }
    return $block
}

$problems = @()
foreach ($spec in $specs.Keys) {
    $dir = Join-Path $tableRoot $spec
    if (-not (Test-Path -LiteralPath $dir)) { continue }
    $tables = @(Get-ChildItem -LiteralPath $dir -Filter '*.psd1' | ForEach-Object {
        $table = Import-PowerShellDataFile -LiteralPath $_.FullName
        $table['File'] = "timelines/$spec/$($_.Name)"
        $table
    } | Sort-Object { [int]($_.Timeline -replace '\D', '') })
    $settings = $specs[$spec]
    $module = "Replay$spec"
    $stepConjunct = if ($settings.Step) { " /\ $($settings.Step)" } else { '' }
    $defs = @()
    foreach ($table in $tables) {
        $where = $table.File
        $t = $table.Timeline
        if ($t -notmatch '^T\d+$') { $problems += "${where}: Timeline must be T<n>"; continue }
        if ([IO.Path]::GetFileNameWithoutExtension($where) -ne $t) { $problems += "${where}: the file must be named $t.psd1"; continue }
        $basePath = Join-Path $specDir "$($table.Base).cfg"
        if (-not (Test-Path -LiteralPath $basePath)) { $problems += "${where}: base configuration $($table.Base) not found"; continue }
        $ids = @{}
        $actions = @()
        foreach ($action in @($table.Before)) { if ($action) { $actions += [pscustomobject]@{ Action = $action; Id = 'before the timeline' } } }
        foreach ($step in @($table.Steps)) {
            if (-not $step.Id) { $problems += "${where}: a step has no Id"; continue }
            if ($ids.ContainsKey($step.Id)) { $problems += "${where}: step $($step.Id) is listed twice" }
            $ids[$step.Id] = $true
            $label = if ($step.Omitted) { "$($step.Id), which the timeline omits" } else { $step.Id }
            foreach ($action in @($step.Actions)) { if ($action) { $actions += [pscustomobject]@{ Action = $action; Id = $label } } }
        }
        foreach ($id in @($table.Projected)) { if ($id -and -not $ids.ContainsKey($id)) { $problems += "${where}: projected step $id is not a step" } }
        if ($actions.Count -eq 0) { $problems += "${where}: no actions"; continue }

        $def = @("\* $t, from $where.", '\* Each disjunct is one action, numbered from 0, with the step it belongs to.', "Replay$t ==")
        for ($i = 0; $i -lt $actions.Count; $i++) {
            $def += '    \/ tstep = {0} /\ {1}  \* {2}' -f $i, $actions[$i].Action, $actions[$i].Id
        }
        $def += @('', "ReplayNext$t == Replay$t$stepConjunct /\ tstep' = tstep + 1", '',
            "ReplaySpec$t == ReplayInit /\ [][ReplayNext$t]_<<$($settings.Vars), tstep>>", '',
            "\* $t's last action has been taken.", "Witness_${t}Replayed == tstep # $($actions.Count)")
        $defs += ($def -join "`n")

        $header = @(
            '\* Copyright (c) Michael Grier.',
            "\* Generated by tools/gen-timeline-replays.ps1 from $where; do not edit.",
            "\* Replays timeline $t of DESIGN-NOTES.md in the constants of $($table.Base).",
            "\* TLC must report Witness_${t}Replayed violated."
        )
        $cfg = $header + @('CONSTANTS') + (Get-Constants $basePath $table.Overrides $where) +
            @("SPECIFICATION ReplaySpec$t", 'INVARIANT', "    Witness_${t}Replayed", 'CHECK_DEADLOCK FALSE')
        Write-Generated (Join-Path $specDir "$module-$t.cfg") (($cfg -join "`n") + "`n")
    }
    if ($tables.Count -eq 0) { continue }

    $text = @"
\* Copyright (c) Michael Grier.
\* Generated by tools/gen-timeline-replays.ps1 from timelines/$spec/; do not edit.
---- MODULE $module ----
(***************************************************************************)
(* Timelines of DESIGN-NOTES.md, each replayed from its scenario table: a  *)
(* step counter, and a next-state relation that allows only the timeline's *)
(* next action. A timeline's witness is violated once its last action is   *)
(* taken, so a replay the model still allows ends in that violation, and   *)
(* one it no longer allows finds no error. Each timeline has a             *)
(* specification of its own, so a configuration reaches only the actions   *)
(* its timeline names.                                                     *)
(***************************************************************************)
EXTENDS $spec

VARIABLE tstep

ReplayInit == $($settings.Init) /\ tstep = 0

$($defs -join "`n`n")
====
"@
    Write-Generated (Join-Path $specDir "$module.tla") ($text + "`n")
}

if ($problems.Count -gt 0) {
    foreach ($problem in $problems) { Write-Error $problem -ErrorAction Continue }
    exit 1
}
"generated; $written file(s) changed"
