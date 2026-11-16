# Copyright (c) Michael Grier.
#
# tools/export-trace-corpus.ps1 -- write the trace corpus, crates/canopy/tests/traces/, from TLC.
#
# Usage:
#   pwsh tools/export-trace-corpus.ps1
#   pwsh tools/export-trace-corpus.ps1 -Filter 'PointerPublication/*'
#   pwsh tools/export-trace-corpus.ps1 -Filter 'Coordination/T1,Coordination/sim-*'
#
# The corpus is what the replay harness runs against the code (DESIGN-NOTES.md "The trace
# corpus"). It holds, for each spec, one file per behaviour:
#
#   <Spec>/T<n>.json             every timeline's replay, from the scenario tables under
#                                crates/canopy/spec/timelines/ (tools/gen-timeline-replays.ps1
#                                writes the replays this runs)
#   <Spec>/sim-<Config>-s<n>.json  a random behaviour of a configuration of the protocol as built,
#                                from TLC's simulation mode with seed n
#   <Spec>/cex-<Defect>.json     the error trace of the configuration that reproduces a named
#                                defect
#
# The simulations and counterexamples are listed in crates/canopy/spec/corpus.psd1. Each file is
# a JSON object with:
#
#   format         1
#   spec           the spec's module
#   configuration  the configuration the behaviour is read from
#   origin         {kind: "timeline", timeline} | {kind: "simulation", seed, depth, walks[, select]}
#                  | {kind: "counterexample", defect}
#   constants      the configuration's constants, as TLA+ text
#   derived        the values corpus.psd1's Derived names for the spec, evaluated in the
#                  configuration (absent for a spec with none)
#   departs        the step at which the code as built leaves the behaviour, or null when the code
#                  follows it to the end
#   initial        the initial state: every variable, in TLC's JSON encoding of values
#   steps          one object per action, in order: step (the timeline's step id, "before" for an
#                  action ahead of its first step, or #<n> for the n-th action of a behaviour
#                  without step ids), omitted (true for a step the timeline leaves out), action
#                  (the action as TLA+ text), name and args (the action's operator and its
#                  arguments: a number or a string as JSON, anything else as {"expr": text}),
#                  at (for a behaviour TLC named, the module and line of the action TLC named),
#                  and changes (each variable the action changes, with its new value)
#
# TLC names a step by the operator whose body it took, so a step through a disjunct that is not an
# operator of its own (SharedPublication's SpNext wraps every action; Coordination's Next has a
# conjunction among its disjuncts) is named by the enclosing operator, and its at says where; a
# timeline's steps are named from its table. A simulation keeps the longest of Walks random walks
# of TLC's simulation mode for each seed. TLC runs single-worker, so a run is reproducible, and
# files are written with LF endings and only when their text changes, so re-exporting changes no
# file when neither the specs nor the tables changed. With no -Filter, a corpus file nothing wrote is deleted.
#
# Requires a JDK (11+) on PATH and tla2tools.jar; see the "TLA+ / TLC" section of
# .github/copilot-instructions.md for the pinned versions.

[CmdletBinding()]
param(
    [string]$Filter = '*',
    [string]$Jar = (Join-Path $env:USERPROFILE '.tla\1.8.0\tla2tools.jar')
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$specDir = Join-Path $repoRoot 'crates/canopy/spec'
$tableRoot = Join-Path $specDir 'timelines'
$corpusDir = Join-Path $repoRoot 'crates/canopy/tests/traces'
$workDir = Join-Path $repoRoot '.scratch/tlc/corpus'

if (-not (Test-Path -LiteralPath $Jar)) {
    Write-Error "tla2tools.jar not found at $Jar; see .github/copilot-instructions.md"
    exit 2
}
if (-not (Get-Command java -ErrorAction SilentlyContinue)) {
    Write-Error 'java not found on PATH; see .github/copilot-instructions.md'
    exit 2
}
New-Item -ItemType Directory -Force -Path $workDir | Out-Null

$patterns = @($Filter -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
function Test-Selected([string]$name) {
    foreach ($pattern in $patterns) { if ($name -like $pattern) { return $true } }
    return $false
}

$utf8 = New-Object System.Text.UTF8Encoding($false)
$written = 0
$produced = @{}
function Write-Generated([string]$path, [string]$text) {
    $text = $text.Replace("`r`n", "`n")
    $script:produced[[IO.Path]::GetFullPath($path)] = $true
    if ((Test-Path -LiteralPath $path) -and [IO.File]::ReadAllText($path).Replace("`r`n", "`n") -eq $text) { return }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
    [IO.File]::WriteAllText($path, $text, $utf8)
    $script:written++
    "wrote $([IO.Path]::GetRelativePath($repoRoot, $path))"
}

# JSON with object keys in ordinal order and no whitespace, so equal values have equal text.
function ConvertTo-Canonical($value) {
    if ($null -eq $value) { return 'null' }
    if ($value -is [string]) { return (ConvertTo-Json -InputObject $value -Compress) }
    if ($value -is [bool]) { if ($value) { return 'true' } else { return 'false' } }
    if ($value -is [System.Collections.IDictionary]) {
        $keys = [string[]]@($value.Keys)
        [Array]::Sort($keys, [StringComparer]::Ordinal)
        $parts = @(foreach ($key in $keys) { (ConvertTo-Json -InputObject $key -Compress) + ':' + (ConvertTo-Canonical $value[$key]) })
        return '{' + ($parts -join ',') + '}'
    }
    if ($value -is [System.Collections.IList]) {
        $parts = @(foreach ($item in $value) { ConvertTo-Canonical $item })
        return '[' + ($parts -join ',') + ']'
    }
    if ($value -is [int] -or $value -is [long] -or $value -is [decimal] -or $value -is [double] -or $value -is [System.Numerics.BigInteger]) {
        return [string]::Format([cultureinfo]::InvariantCulture, '{0}', $value)
    }
    throw "cannot encode a $($value.GetType().FullName)"
}

# A TLC value as TLA+ text, for an action's arguments.
function Format-Tla($value) {
    if ($value -is [string]) { return '"' + $value + '"' }
    if ($value -is [bool]) { if ($value) { return 'TRUE' } else { return 'FALSE' } }
    return (ConvertTo-Canonical $value)
}

# An action written as TLA+ text, split into its operator and its arguments.
function Split-Action([string]$text) {
    $text = $text.Trim()
    $open = $text.IndexOf('(')
    if ($open -lt 0 -or -not $text.EndsWith(')')) { return @{ Name = $text; Args = @() } }
    $inner = $text.Substring($open + 1, $text.Length - $open - 2)
    $texts = @()
    $depth = 0
    $quoted = $false
    $start = 0
    for ($i = 0; $i -lt $inner.Length; $i++) {
        $c = $inner[$i]
        if ($quoted) { if ($c -eq '"') { $quoted = $false }; continue }
        if ($c -eq '"') { $quoted = $true }
        elseif ('({[<'.Contains($c)) { $depth++ }
        elseif (')}]>'.Contains($c)) { $depth-- }
        elseif ($c -eq ',' -and $depth -eq 0) { $texts += $inner.Substring($start, $i - $start).Trim(); $start = $i + 1 }
    }
    $texts += $inner.Substring($start).Trim()
    $values = @(foreach ($arg in $texts) {
        if ($arg -match '^-?\d+$') { [long]$arg }
        elseif ($arg -match '^"([^"]*)"$') { $Matches[1] }
        else { [ordered]@{ expr = $arg } }
    })
    return @{ Name = $text.Substring(0, $open).Trim(); Args = $values }
}

# A configuration's constants, as name -> TLA+ text.
function Get-Constants([string]$cfgPath) {
    $lines = @(Get-Content -LiteralPath $cfgPath)
    $start = [array]::IndexOf($lines, 'CONSTANTS')
    $constants = [ordered]@{}
    if ($start -lt 0) { return $constants }
    for ($i = $start + 1; $i -lt $lines.Count -and $lines[$i] -match '^\s+(\w+) = (.*)$'; $i++) {
        $constants[$Matches[1]] = $Matches[2].Trim()
    }
    return $constants
}

# Run TLC on one configuration and return the error trace it dumps as JSON.
function Invoke-Trace([string]$module, [string]$cfgPath, [string]$name, [string[]]$extra, [string]$library) {
    $dump = Join-Path $workDir "$name.json"
    $log = Join-Path $workDir "$name.out"
    if (Test-Path -LiteralPath $dump) { Remove-Item -LiteralPath $dump }
    $jvm = @('-XX:+UseParallelGC')
    if ($library) { $jvm += "-DTLA-Library=$library" }
    Push-Location (Split-Path -Parent $module)
    try {
        & java @jvm -cp $Jar tlc2.TLC -config $cfgPath -workers 1 -metadir (Join-Path $workDir 'states') `
            -cleanup -noGenerateSpecTE -dumpTrace json $dump @extra $module *> $log
        $code = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }
    if (-not (Test-Path -LiteralPath $dump)) {
        throw "TLC wrote no trace for $name (exit code $code); see $log"
    }
    $json = Get-Content -LiteralPath $dump -Raw | ConvertFrom-Json -AsHashtable
    $states = @(foreach ($entry in $json.counterexample.state) { $entry[1] })
    $actions = @(foreach ($entry in $json.counterexample.action) { $entry[1] })
    if ($states.Count -ne $actions.Count + 1) { throw "${name}: $($states.Count) states for $($actions.Count) actions" }
    return @{ States = $states; Actions = $actions }
}

# Write one corpus file from a trace and, per action, its step id and TLA+ text.
function Write-Corpus([string]$path, [hashtable]$header, $states, $steps, [string[]]$drop) {
    foreach ($state in $states) { foreach ($key in $drop) { $state.Remove($key) } }
    $lines = @('{')
    foreach ($key in 'format', 'spec', 'configuration', 'origin', 'constants', 'derived', 'departs') {
        if ($key -eq 'derived' -and $header[$key].Count -eq 0) { continue }
        $lines += '  ' + (ConvertTo-Json -InputObject $key -Compress) + ': ' + (ConvertTo-Canonical $header[$key]) + ','
    }
    $lines += '  "initial": ' + (ConvertTo-Canonical $states[0]) + ','
    $lines += '  "steps": ['
    for ($i = 0; $i -lt $steps.Count; $i++) {
        $before = $states[$i]
        $after = $states[$i + 1]
        $changes = [ordered]@{}
        foreach ($key in $after.Keys) {
            if ((ConvertTo-Canonical $before[$key]) -ne (ConvertTo-Canonical $after[$key])) { $changes[$key] = $after[$key] }
        }
        $split = Split-Action $steps[$i].Action
        $step = [ordered]@{ step = $steps[$i].Step; action = $steps[$i].Action; name = $split.Name; args = @($split.Args); changes = $changes }
        if ($steps[$i].Omitted) { $step['omitted'] = $true }
        if ($steps[$i].At) { $step['at'] = $steps[$i].At }
        $comma = if ($i -lt $steps.Count - 1) { ',' } else { '' }
        $lines += '    ' + (ConvertTo-Canonical $step) + $comma
    }
    $lines += '  ]'
    $lines += '}'
    Write-Generated $path (($lines -join "`n") + "`n")
}

# The steps of a behaviour TLC names action by action.
function Get-NamedSteps($actions) {
    $n = 0
    return @(foreach ($action in $actions) {
        $n++
        $text = $action.name
        if ($action.parameters) {
            $text += '(' + ((@($action.parameters) | ForEach-Object { Format-Tla $action.context[$_] }) -join ', ') + ')'
        }
        $at = if ($action.location) { "$($action.location.module):$($action.location.beginLine)" } else { $null }
        [pscustomobject]@{ Step = "#$n"; Action = $text; At = $at; Omitted = $false }
    })
}

# What each spec calls its initial predicate.
$initOf = @{ SharedPublication = 'SpInit' }

# The CONSTANTS block of a configuration, as its lines.
function Get-ConstantLines([string]$cfgPath) {
    $lines = @(Get-Content -LiteralPath $cfgPath)
    $start = [array]::IndexOf($lines, 'CONSTANTS')
    if ($start -lt 0) { return @() }
    $block = @($lines[$start])
    for ($i = $start + 1; $i -lt $lines.Count -and $lines[$i] -match '^\s+\w+ = '; $i++) { $block += $lines[$i] }
    return $block
}

# The values corpus.psd1 derives for the spec, evaluated by TLC in the configuration's constants:
# a one-state behaviour whose extra variable holds them, which TLC dumps as the error trace of an
# invariant that is false.
$derivedCache = @{}
function Get-Derived([string]$spec, [string]$cfgPath) {
    $table = $corpus.Derived[$spec]
    if (-not $table) { return [ordered]@{} }
    $key = [IO.Path]::GetFileNameWithoutExtension($cfgPath)
    if ($derivedCache.ContainsKey($key)) { return $derivedCache[$key] }
    $dir = Join-Path $workDir "derived-$key"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $init = if ($initOf[$spec]) { $initOf[$spec] } else { 'Init' }
    $names = [string[]]@($table.Keys)
    [Array]::Sort($names, [StringComparer]::Ordinal)
    $fields = ($names | ForEach-Object { "$_ |-> $($table[$_])" }) -join ', '
    $module = Join-Path $dir 'CorpusDerived.tla'
    $moduleText = @(
        '---- MODULE CorpusDerived ----',
        "EXTENDS $spec",
        'VARIABLE corpusDerived',
        "CorpusDerivedInit == $init /\ corpusDerived = [$fields]",
        'CorpusDerivedSpec == CorpusDerivedInit /\ [][FALSE]_corpusDerived',
        'CorpusDerivedNever == corpusDerived # corpusDerived',
        '===='
    ) -join "`n"
    [IO.File]::WriteAllText($module, $moduleText + "`n", $utf8)
    $cfg = Join-Path $dir 'CorpusDerived.cfg'
    $cfgText = (Get-ConstantLines $cfgPath) + @('SPECIFICATION CorpusDerivedSpec', 'INVARIANT', '    CorpusDerivedNever')
    [IO.File]::WriteAllText($cfg, ($cfgText -join "`n") + "`n", $utf8)
    $trace = Invoke-Trace $module $cfg "$spec-derived-$key" @() $specDir
    $value = $trace.States[0]['corpusDerived']
    $derivedCache[$key] = $value
    return $value
}

$problems = @()
$corpus = Import-PowerShellDataFile -LiteralPath (Join-Path $specDir 'corpus.psd1')

# Timelines: each replay's trace, its actions named from the table.
foreach ($specFolder in @(Get-ChildItem -LiteralPath $tableRoot -Directory)) {
    $spec = $specFolder.Name
    foreach ($file in @(Get-ChildItem -LiteralPath $specFolder.FullName -Filter '*.psd1')) {
        $table = Import-PowerShellDataFile -LiteralPath $file.FullName
        $t = $table.Timeline
        if (-not (Test-Selected "$spec/$t")) { continue }
        $where = "timelines/$spec/$($file.Name)"
        $steps = @()
        foreach ($action in @($table.Before)) { if ($action) { $steps += [pscustomobject]@{ Step = 'before'; Action = $action; Omitted = $false } } }
        $ids = @{}
        foreach ($step in @($table.Steps)) {
            $ids[$step.Id] = $true
            foreach ($action in @($step.Actions)) { if ($action) { $steps += [pscustomobject]@{ Step = $step.Id; Action = $action; Omitted = [bool]$step.Omitted } } }
        }
        if ($table.Departs -and -not $ids.ContainsKey($table.Departs)) { $problems += "${where}: Departs names no step"; continue }
        $cfgPath = Join-Path $specDir "Replay$spec-$t.cfg"
        if (-not (Test-Path -LiteralPath $cfgPath)) { $problems += "${where}: $cfgPath not found; run tools/gen-timeline-replays.ps1"; continue }
        $trace = Invoke-Trace (Join-Path $specDir "Replay$spec.tla") $cfgPath "$spec-$t" @() $null
        if ($trace.Actions.Count -ne $steps.Count) { $problems += "${where}: the replay took $($trace.Actions.Count) of $($steps.Count) actions"; continue }
        $header = @{
            format = 1; spec = $spec; configuration = $table.Base
            origin = [ordered]@{ kind = 'timeline'; timeline = $t }
            constants = Get-Constants $cfgPath
            derived = Get-Derived $spec $cfgPath
            departs = if ($table.Departs) { $table.Departs } else { $null }
        }
        Write-Corpus (Join-Path $corpusDir "$spec/$t.json") $header $trace.States $steps @('tstep')
    }
}

# Simulations: TLC's simulation mode writes each random walk as a module of numbered states, with
# the action that reached each state in a comment above it. The walk chosen of a seed's walks is
# replayed state by state, so that TLC dumps it in the JSON encoding the rest of the corpus uses.
function Get-ControlKinds([string[]]$lines, [string[]]$fields) {
    $previous = $null
    $current = $null
    $kinds = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($line in @($lines) + @('STATE_END ==')) {
        if ($line -match '^STATE_(\d+|END) ==') {
            if ($current -and $previous) {
                $changed = [ordered]@{}
                foreach ($field in $fields) {
                    if ($previous[$field] -ne $current[$field]) { $changed[$field] = @($previous[$field], $current[$field]) }
                }
                if ($changed.Count -gt 0) { [void]$kinds.Add((ConvertTo-Canonical $changed)) }
            }
            $previous = $current
            $current = @{}
        } elseif ($null -ne $current -and $line -match '^\s*/\\\s+(\w+) = (.*)$' -and $Matches[1] -in $fields) {
            $current[$Matches[1]] = $Matches[2]
        }
    }
    return $kinds.Count
}

foreach ($sim in @($corpus.Simulations)) {
    $spec = $sim.Spec
    $basePath = Join-Path $specDir "$($sim.Config).cfg"
    if (-not (Test-Path -LiteralPath $basePath)) { $problems += "corpus.psd1: configuration $($sim.Config) not found"; continue }
    $walks = if ($sim.Walks) { [int]$sim.Walks } else { 50 }
    $simDir = Join-Path $workDir "sim-$($sim.Config)"
    New-Item -ItemType Directory -Force -Path $simDir | Out-Null
    $cfgLines = @()
    $skipping = $false
    foreach ($line in @(Get-Content -LiteralPath $basePath)) {
        if ($line -match '^\\\*') { continue }
        if ($line -match '^[A-Z_]+') { $skipping = $line -match '^(INVARIANTS?|PROPERTY|PROPERTIES)\b' }
        if (-not $skipping) { $cfgLines += $line }
    }
    $walkCfg = Join-Path $simDir 'walk.cfg'
    [IO.File]::WriteAllText($walkCfg, ($cfgLines -join "`n") + "`n", $utf8)
    $constantLines = @()
    $start = [array]::IndexOf($cfgLines, 'CONSTANTS')
    if ($start -ge 0) {
        for ($i = $start; $i -lt $cfgLines.Count -and ($i -eq $start -or $cfgLines[$i] -match '^\s+\w+ = '); $i++) { $constantLines += $cfgLines[$i] }
    }
    foreach ($seed in 1..[int]$sim.Seeds) {
        $name = "sim-$($sim.Config)-s$seed"
        if (-not (Test-Selected "$spec/$name")) { continue }
        $seedDir = Join-Path $simDir "s$seed"
        if (Test-Path -LiteralPath $seedDir) { Remove-Item -LiteralPath $seedDir -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $seedDir | Out-Null
        $log = Join-Path $workDir "$spec-$name-walks.out"
        Push-Location $specDir
        try {
            & java -XX:+UseParallelGC -cp $Jar tlc2.TLC -config $walkCfg -workers 1 -metadir (Join-Path $workDir 'states') `
                -cleanup -noGenerateSpecTE -simulate "num=$walks,file=$(Join-Path $seedDir 'walk')" `
                -depth $sim.Depth -seed $seed (Join-Path $specDir "$spec.tla") *> $log
            $code = $LASTEXITCODE
        }
        finally {
            Pop-Location
        }
        if ($code -ne 0) { $problems += "${name}: TLC's simulation failed with exit code $code; see $log"; continue }

        # The longest walk, or under Select = 'distinct' the walk that takes the most kinds of action
        # and the longest of those; the first of equal rank, in the order TLC wrote them. A kind is
        # the action's operator, or for an action TLC names by its enclosing operator, its location.
        $best = $null
        foreach ($file in @(Get-ChildItem -LiteralPath $seedDir -Filter 'walk_*' | Sort-Object { [int]($_.Name -replace '^walk_\d+_', '') })) {
            $lines = @(Get-Content -LiteralPath $file.FullName)
            $count = @($lines | Where-Object { $_ -match '^STATE_\d+ ==' }).Count
            $kinds = 0
            if ($sim.Select -eq 'distinct') {
                $kinds = if ($sim.KindState) { Get-ControlKinds $lines $sim.KindState } else { @($lines | ForEach-Object {
                        if ($_ -match '^\\\* <(\w+)(?:\([^)]*\))? (line \d+, col \d+)') { "$($Matches[1])@$($Matches[2])" }
                    } | Sort-Object -Unique).Count }
            }
            if (-not $best -or $kinds -gt $best.Kinds -or ($kinds -eq $best.Kinds -and $count -gt $best.Count)) {
                $best = @{ Lines = $lines; Count = $count; Kinds = $kinds }
            }
        }
        if (-not $best -or $best.Count -eq 0) { $problems += "${name}: TLC wrote no walk; see $log"; continue }

        # TLC writes each state as a conjunction of x = value, a variable to a line; a state after the
        # first becomes an action by priming each variable, since TLC does not evaluate a primed
        # conjunction of equalities as an assignment.
        $body = @()
        $steps = @()
        $pending = $null
        $primed = $false
        foreach ($line in $best.Lines) {
            if ($line -match '^-+ MODULE ' -or $line -match '^=+\s*$') { continue }
            if ($line -match '^\\\* <(.+?) line (\d+), col \d+ to line \d+, col \d+ of module (\w+)>') {
                $pending = [pscustomobject]@{ Action = $Matches[1]; At = "$($Matches[3]):$($Matches[2])" }
                continue
            }
            if ($line -match '^STATE_(\d+) ==') {
                $primed = [int]$Matches[1] -gt 1
                if ($primed) {
                    $steps += [pscustomobject]@{ Step = "#$($steps.Count + 1)"; Action = $pending.Action; At = $pending.At; Omitted = $false }
                }
            }
            elseif ($primed -and $line -match '^(\s*/\\\s+\w+) = ') {
                $line = $Matches[1] + "' = " + $line.Substring($Matches[0].Length)
            }
            $body += $line
        }
        $n = $best.Count
        $next = @(for ($i = 1; $i -lt $n; $i++) { "    \/ cwstep = $i /\ STATE_$($i + 1) /\ cwstep' = $($i + 1)" })
        if ($next.Count -eq 0) { $next = @('    FALSE') }
        $module = Join-Path $seedDir "CorpusWalk$spec.tla"
        $moduleText = @("---- MODULE CorpusWalk$spec ----", "EXTENDS $spec, TLC", 'VARIABLE cwstep') + $body + @(
            'CorpusWalkInit == STATE_1 /\ cwstep = 1',
            'CorpusWalkNext ==') + $next + @(
            'CorpusWalkSpec == CorpusWalkInit /\ [][CorpusWalkNext]_cwstep',
            "CorpusWalkNotEnded == cwstep # $n",
            '====')
        [IO.File]::WriteAllText($module, ($moduleText -join "`n") + "`n", $utf8)
        $cfgPath = Join-Path $seedDir "CorpusWalk$spec.cfg"
        $cfgText = $constantLines + @('SPECIFICATION CorpusWalkSpec', 'INVARIANT', '    CorpusWalkNotEnded', 'CHECK_DEADLOCK FALSE')
        [IO.File]::WriteAllText($cfgPath, ($cfgText -join "`n") + "`n", $utf8)
        $trace = Invoke-Trace $module $cfgPath "$spec-$name" @() $specDir
        if ($trace.Actions.Count -ne $steps.Count) { $problems += "${name}: the replay took $($trace.Actions.Count) of $($steps.Count) actions"; continue }
        $header = @{
            format = 1; spec = $spec; configuration = $sim.Config
            origin = [ordered]@{ kind = 'simulation'; seed = [long]$seed; depth = [long]$sim.Depth; walks = [long]$walks }
            constants = Get-Constants $basePath
            derived = Get-Derived $spec $basePath
            departs = $null
        }
        if ($sim.Select) { $header.origin.select = $sim.Select }
        Write-Corpus (Join-Path $corpusDir "$spec/$name.json") $header $trace.States $steps @('cwstep')
    }
}

foreach ($witness in @($corpus.Witnesses | Where-Object { $_ })) {
    $spec = $witness.Spec
    $name = "witness-$($witness.Name)"
    if (-not (Test-Selected "$spec/$name")) { continue }
    $cfgPath = Join-Path $specDir "$($witness.Config).cfg"
    if (-not (Test-Path -LiteralPath $cfgPath)) { $problems += "corpus.psd1: configuration $($witness.Config) not found"; continue }
    $trace = Invoke-Trace (Join-Path $specDir "$spec.tla") $cfgPath "$spec-$name" @() $null
    $steps = Get-NamedSteps $trace.Actions
    $header = @{
        format = 1; spec = $spec; configuration = $witness.Config
        origin = [ordered]@{ kind = 'witness'; witness = $witness.Name }
        constants = Get-Constants $cfgPath
        derived = Get-Derived $spec $cfgPath
        departs = $null
    }
    Write-Corpus (Join-Path $corpusDir "$spec/$name.json") $header $trace.States $steps @()
}

# Counterexamples: the configuration's own error trace.
foreach ($cex in @($corpus.Counterexamples)) {
    $spec = $cex.Spec
    $name = "cex-$($cex.Name)"
    if (-not (Test-Selected "$spec/$name")) { continue }
    $cfgPath = Join-Path $specDir "$($cex.Config).cfg"
    if (-not (Test-Path -LiteralPath $cfgPath)) { $problems += "corpus.psd1: configuration $($cex.Config) not found"; continue }
    $trace = Invoke-Trace (Join-Path $specDir "$spec.tla") $cfgPath "$spec-$name" @() $null
    $steps = Get-NamedSteps $trace.Actions
    if ($cex.Departs -and -not ($steps | Where-Object { $_.Step -eq $cex.Departs })) { $problems += "corpus.psd1: $name departs at $($cex.Departs), which is not a step"; continue }
    $header = @{
        format = 1; spec = $spec; configuration = $cex.Config
        origin = [ordered]@{ kind = 'counterexample'; defect = $cex.Name }
        constants = Get-Constants $cfgPath
        derived = Get-Derived $spec $cfgPath
        departs = if ($cex.Departs) { $cex.Departs } else { $null }
    }
    Write-Corpus (Join-Path $corpusDir "$spec/$name.json") $header $trace.States $steps @()
}

if ($patterns.Count -eq 1 -and $patterns[0] -eq '*' -and (Test-Path -LiteralPath $corpusDir)) {
    foreach ($file in @(Get-ChildItem -LiteralPath $corpusDir -Recurse -Filter '*.json')) {
        if (-not $produced.ContainsKey($file.FullName)) {
            Remove-Item -LiteralPath $file.FullName
            "deleted $([IO.Path]::GetRelativePath($repoRoot, $file.FullName))"
        }
    }
}

if ($problems.Count -gt 0) {
    foreach ($problem in $problems) { Write-Error $problem -ErrorAction Continue }
    exit 1
}
"exported; $written file(s) changed"
