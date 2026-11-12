# Copyright (c) Michael Grier.
#
# tools/check-audience-tags.ps1 -- check that every property a spec's configurations check says
# who it is for.
#
# Usage:
#   pwsh tools/check-audience-tags.ps1 -Spec crates/canopy/spec/Coordination.tla
#
# A property is client-visible when a Store user can see its violation as an outcome of the API, and
# internal when only the implementation can. The first line of a property's header comment says
# which, as `\* Audience: client.` or `\* Audience: internal.`; a client one goes on to state the
# guarantee in a user's words and to name the outcome it shows through, as `Shows through: <outcome>.`
# Every invariant and property named by the spec's configurations, other than the Witness_ ones,
# which are evidence that a state is reachable and not guarantees, must carry the tag. Exit code is
# 0 when every one does, 1 when any does not, and 2 when the spec cannot be found.

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$Spec
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib/Probes.ps1')

# The one place this tool writes its report.
function Report([string]$line) { Write-Output $line }

if (-not (Test-Path -LiteralPath $Spec)) {
    Write-Error "spec not found: $Spec" -ErrorAction Continue
    exit 2
}
$specPath = (Resolve-Path -LiteralPath $Spec).Path
$specDir = Split-Path -Parent $specPath
$specName = [System.IO.Path]::GetFileNameWithoutExtension($specPath)
$chain = Get-Chain $specDir $specName

$keywords = 'CONSTANTS', 'CONSTANT', 'SPECIFICATION', 'INIT', 'NEXT', 'INVARIANT', 'INVARIANTS',
    'PROPERTY', 'PROPERTIES', 'CONSTRAINT', 'CONSTRAINTS', 'ACTION_CONSTRAINT', 'ACTION_CONSTRAINTS',
    'SYMMETRY', 'VIEW', 'ALIAS', 'CHECK_DEADLOCK'
$names = @{}
$configs = Get-ChildItem -LiteralPath $specDir -File -Filter '*.cfg' |
    Where-Object { $_.Name -eq "$specName.cfg" -or $_.Name -like "$specName-*.cfg" }
foreach ($config in $configs) {
    $listing = $false
    foreach ($line in (Get-Content -LiteralPath $config.FullName)) {
        $text = $line.Trim()
        if (-not $text -or $text.StartsWith('\*')) { continue }
        $first = ($text -split '\s+')[0]
        if ($first -in $keywords) {
            $listing = $first -in 'INVARIANT', 'INVARIANTS', 'PROPERTY', 'PROPERTIES'
            $rest = $text.Substring($first.Length).Trim()
            if ($listing -and $rest) { $names[($rest -split '\s+')[0]] = $true }
        } elseif ($listing) {
            $names[$first] = $true
        }
    }
}

$problems = @()
$checked = 0
foreach ($name in ($names.Keys | Where-Object { $_ -notlike 'Witness_*' } | Sort-Object)) {
    $checked++
    $found = $null
    foreach ($module in $chain.Keys) {
        $lines = $chain[$module] -split "`n"
        for ($i = 0; $i -lt $lines.Count -and -not $found; $i++) {
            if ($lines[$i] -match ('^' + [regex]::Escape($name) + '\s*(==|\()')) { $found = @{ Module = $module; Lines = $lines; Index = $i } }
        }
        if ($found) { break }
    }
    if (-not $found) { $problems += "$name is named by a configuration and defined nowhere in $specName"; continue }
    $block = @()
    for ($j = $found.Index - 1; $j -ge 0 -and $found.Lines[$j].StartsWith('\*'); $j--) { $block = , $found.Lines[$j] + $block }
    $tags = @($block | Where-Object { $_ -match '^\\\* Audience: (client|internal)\.' })
    if ($tags.Count -ne 1) { $problems += "$name ($($found.Module)) has $($tags.Count) audience tags, and needs one"; continue }
    $prose = ($block | ForEach-Object { $_ -replace '^\\\* ?', '' }) -join ' '
    if ($tags[0] -match 'Audience: client\.' -and $prose -notmatch 'Shows through: ') {
        $problems += "$name ($($found.Module)) is client-visible and names no outcome it shows through"
    }
}
foreach ($problem in $problems) { Report $problem }
Report "checked: $checked, problems: $($problems.Count)"
if ($problems.Count -gt 0) { exit 1 }
exit 0
