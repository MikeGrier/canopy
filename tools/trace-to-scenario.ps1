# Copyright (c) Michael Grier.
#
# tools/trace-to-scenario.ps1 -- draft a timeline's scenario table from a TLC counterexample.
#
# Usage:
#   pwsh tools/trace-to-scenario.ps1 -Path .scratch/tlc/<config-name>.out -Timeline T31
#
# Prints a scenario table (see tools/gen-timeline-replays.ps1) with one step per action of the
# trace, each named as TLC names it, with its arguments, and the configuration the trace came from
# as its base. Every Id is left as '?': grouping the actions into the timeline's steps, and moving
# the ones that come before its first step into Before, is the author's work, since only the
# timeline says where its steps begin and end. A trace from one worker (tools/run-tlc.ps1 -Workers
# 1) is the shortest and has no incidental steps.

param(
    [Parameter(Mandatory)][string]$Path,
    [string]$Timeline = 'T?'
)

$ErrorActionPreference = 'Stop'
$base = [System.IO.Path]::GetFileNameWithoutExtension($Path)
$actions = @(Select-String -LiteralPath $Path -Pattern '^State \d+: <(.+?) line \d+, col \d+ to line \d+, col \d+ of module \w+>' |
    ForEach-Object { $_.Matches[0].Groups[1].Value })
if ($actions.Count -eq 0) {
    Write-Error "no trace in $Path" -ErrorAction Continue
    exit 1
}

"@{"
"    Timeline = '$Timeline'"
"    Base = '$base'"
"    Before = @()"
"    Steps = @("
foreach ($action in $actions) { "        @{ Id = '?'; Actions = @('$($action.Replace("'", "''"))') }" }
"    )"
"}"
