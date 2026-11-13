# Copyright (c) Michael Grier.
#
# tools/trace-summary.ps1 -- print a TLC counterexample compactly: each state's action and the
# variables that changed in it.
#
# Usage:
#   pwsh tools/trace-summary.ps1 -Path .scratch/tlc/<config-name>.out
#   pwsh tools/trace-summary.ps1 -Path <output> -Vars 'pc,att,lockHolder'
#
# -Vars lists the variables to follow, comma-separated; the default is the writer exclusion and
# transaction variables of Coordination.tla. A trace from one worker (tools/run-tlc.ps1 -Workers 1)
# is the shortest and has no incidental steps.

param([Parameter(Mandatory)][string]$Path, [string]$Vars = 'pc,lockHolder,owner,fence,trusted,count,a,b,log,stage,ptgt,disc,diedIn')
$want = $Vars -split ','
$lines = Get-Content -LiteralPath $Path
$state = $null
$rows = @()
$cur = $null
foreach ($l in $lines) {
    if ($l -match '^State (\d+): (.*)$') {
        $n = [int]$Matches[1]
        $act = $Matches[2]
        if ($act -match '^<(\w+)') { $act = $Matches[1] }
        if ($cur) { $rows += $cur }
        $cur = [ordered]@{ N = $n; Action = $act; V = [ordered]@{} }
        continue
    }
    if ($cur -and $l -match '^/\\ (\w+) = (.*)$') {
        $name = $Matches[1]
        $val = $Matches[2]
        if ($want -contains $name) { $cur.V[$name] = $val }
    }
}
if ($cur) { $rows += $cur }
$prev = @{}
foreach ($r in $rows) {
    $diff = @()
    foreach ($k in $r.V.Keys) {
        if ($prev[$k] -ne $r.V[$k]) { $diff += "$k=$($r.V[$k])" }
        $prev[$k] = $r.V[$k]
    }
    '{0,2} {1,-14} {2}' -f $r.N, $r.Action, ($diff -join '  ')
}
