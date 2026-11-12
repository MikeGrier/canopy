# Copyright (c) Michael Grier.
#
# tools/check-run-tlc-all.ps1 -- check that tools/run-tlc-all.ps1 can fail.
#
# Usage:
#   pwsh tools/check-run-tlc-all.ps1
#
# A sweep that cannot report a mismatch would let a control that stops failing, or a witness whose
# state becomes unreachable, pass forever. This builds a scratch copy of the Coordination modules
# under .scratch/sweep-fail with four configurations derived from one real control -- the control
# itself, one whose Control constant no longer breaks the rule, one whose header names another
# property, and one whose header states no outcome -- runs the sweep over them, and checks the exit
# codes and the lines it reports: mismatches and the unchecked configuration are found, a filter
# narrows the run, -FailFast stops at the first mismatch, -Tier and -List select without running,
# and a missing spec, jar or matching configuration is exit 2. Exit code is 0 when every check
# holds, 1 when any does not, and 2 when the real control, java or tla2tools.jar cannot be found.

[CmdletBinding()]
param(
    [string]$Jar = (Join-Path $env:USERPROFILE '.tla\1.8.0\tla2tools.jar')
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$specDir = Join-Path $repoRoot 'crates/canopy/spec'
$tool = Join-Path $PSScriptRoot 'run-tlc-all.ps1'
$baseControl = Join-Path $specDir 'Coordination-Control-ReplaceWithoutFence.cfg'
$property = 'AbandonedRecordNeverReplacedUnfenced'
$otherProperty = 'NormalAcquireFindsAHealthyStore'

if (-not (Test-Path -LiteralPath $baseControl)) {
    Write-Error "the control this check is derived from is missing: $baseControl" -ErrorAction Continue
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

$report = New-Object System.Collections.Generic.List[string]
$failures = 0
function Report([string]$line) { $report.Add($line) }

function Check([string]$what, [bool]$holds, [string]$detail = '') {
    if (-not $holds) { $script:failures++ }
    Report ('{0} {1}{2}' -f $(if ($holds) { 'ok  ' } else { 'FAIL' }), $what, $(if (-not $holds -and $detail) { " -- $detail" } else { '' }))
}

# The sweep run as a child process: its exit code and every line it wrote.
function Run-Sweep([string[]]$Arguments) {
    $lines = @(& pwsh -NoProfile -File $tool @Arguments 2>&1 | ForEach-Object { "$_" })
    return [pscustomobject]@{ Exit = $LASTEXITCODE; Text = ($lines -join "`n") }
}

$dir = Join-Path $repoRoot '.scratch/sweep-fail'
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item (Join-Path $specDir 'Coordination*.tla') $dir

# A configuration made from the real control: its header lines replaced, and optionally its
# Control constant set to something that breaks no rule.
$utf8 = New-Object System.Text.UTF8Encoding($false)
function New-Cfg([string]$name, [string[]]$header, [bool]$stillBreaksTheRule = $true) {
    $body = (Get-Content -LiteralPath $baseControl | Where-Object { -not $_.StartsWith('\*') }) -join "`n"
    if (-not $stillBreaksTheRule) { $body = $body.Replace('Control = "replace-without-fence"', 'Control = "none"') }
    $text = ((($header | ForEach-Object { "\* $_" }) -join "`n") + "`n" + $body + "`n")
    [IO.File]::WriteAllText((Join-Path $dir "$name.cfg"), $text, $utf8)
}

New-Cfg 'Coordination-Control-Correct' @('A correct control.', "TLC must report $property", 'violated.')
New-Cfg 'Coordination-Control-StopsFailing' @('A control that no longer fails.', "TLC must report $property violated.") $false
New-Cfg 'Coordination-Control-WrongProperty' @('A control that fails on another property.', "TLC must report $otherProperty violated.", 'Tier: milestone.')
New-Cfg 'Coordination-NoOutcome' @('A configuration whose header states no outcome.')
New-Cfg 'Coordination-Control-OnDemand' @('A control run only on demand.', "TLC must report $property violated.", 'Tier: evidence.')
$spec = Join-Path $dir 'Coordination.tla'

$all = Run-Sweep @('-Spec', $spec)
Check 'every configuration: exit 1' ($all.Exit -eq 1) "exit $($all.Exit)"
Check 'one configuration is ok' (([regex]::Matches($all.Text, '(?m)^ok\s+Coordination-Control-Correct')).Count -eq 1)
Check 'a control that stops failing is a mismatch' ($all.Text -match "(?m)^MISMATCH Coordination-Control-StopsFailing\.cfg\s+expected $property, got PASS")
Check 'a control that fails on another property is a mismatch' ($all.Text -match "(?m)^MISMATCH Coordination-Control-WrongProperty\.cfg\s+expected $otherProperty, got $property")
Check 'the tally is three checked, two mismatched, one unchecked' ($all.Text -match '(?m)^checked: 3, mismatched: 2, unchecked: 1$')
Check 'a header with no outcome is listed as unchecked' ($all.Text -match '(?m)^unchecked \(header states no outcome\): Coordination-NoOutcome\.cfg$')

$parallel = Run-Sweep @('-Spec', $spec, '-Parallel', '2')
Check '-Parallel reports what the sequential sweep reports' ($parallel.Exit -eq 1 -and $parallel.Text -match '(?m)^checked: 3, mismatched: 2, unchecked: 1$' -and $parallel.Text -match "(?m)^MISMATCH Coordination-Control-StopsFailing\.cfg\s+expected $property, got PASS") "exit $($parallel.Exit)"
$parallelFast = Run-Sweep @('-Spec', $spec, '-Parallel', '2', '-FailFast')
Check '-Parallel with -FailFast: exit 2' ($parallelFast.Exit -eq 2) "exit $($parallelFast.Exit)"

$one = Run-Sweep @('-Spec', $spec, '-Filter', '*Correct*')
Check 'a filter that matches only the correct control: exit 0' ($one.Exit -eq 0) "exit $($one.Exit)"
Check 'that run checked one configuration and none mismatched' ($one.Text -match '(?m)^checked: 1, mismatched: 0, unchecked: 0$')

$two = Run-Sweep @('-Spec', $spec, '-Filter', '*Correct*,*StopsFailing*')
Check 'comma-separated filters both apply' ($two.Exit -eq 1 -and $two.Text -match '(?m)^checked: 2, mismatched: 1, unchecked: 0$') "exit $($two.Exit)"

$fast = Run-Sweep @('-Spec', $spec, '-FailFast')
Check '-FailFast stops at the first mismatch' ($fast.Exit -eq 1 -and $fast.Text -match '(?m)^stopped at the first mismatch: 2 configuration\(s\) not run$') "exit $($fast.Exit)"

$milestone = Run-Sweep @('-Spec', $spec, '-Tier', 'milestone', '-List')
Check '-Tier milestone -List names the tagged configuration only' ($milestone.Exit -eq 0 -and $milestone.Text -match '(?m)^milestone\s+Coordination-Control-WrongProperty\.cfg$' -and $milestone.Text -match '(?m)^listed: 1$')
$item = Run-Sweep @('-Spec', $spec, '-Tier', 'item', '-List')
Check '-Tier item -List names the other three' ($item.Exit -eq 0 -and $item.Text -match '(?m)^listed: 3$' -and $item.Text -notmatch 'WrongProperty')
Check 'the default sweep names the evidence configuration it did not run' ($all.Text -match '(?m)^not run, evidence tier \(-Tier evidence\): Coordination-Control-OnDemand\.cfg$' -and $all.Text -notmatch '(?m)^\S+\s+Coordination-Control-OnDemand')
$evidence = Run-Sweep @('-Spec', $spec, '-Tier', 'evidence')
Check '-Tier evidence runs the evidence configuration only' ($evidence.Exit -eq 0 -and $evidence.Text -match '(?m)^ok\s+Coordination-Control-OnDemand\.cfg' -and $evidence.Text -match '(?m)^checked: 1, mismatched: 0, unchecked: 0$') "exit $($evidence.Exit)"
$everything = Run-Sweep @('-Spec', $spec, '-Tier', 'all', '-List')
Check '-Tier all -List names all five' ($everything.Exit -eq 0 -and $everything.Text -match '(?m)^listed: 5$' -and $everything.Text -match '(?m)^evidence\s+Coordination-Control-OnDemand\.cfg$')
$onlyEvidence = Run-Sweep @('-Spec', $spec, '-Filter', '*OnDemand*')
Check 'a default sweep that matches only evidence configurations: exit 2' ($onlyEvidence.Exit -eq 2) "exit $($onlyEvidence.Exit)"

$none = Run-Sweep @('-Spec', $spec, '-Filter', '*NoSuchConfig*')
Check 'a filter that matches nothing: exit 2' ($none.Exit -eq 2) "exit $($none.Exit)"
$missingSpec = Run-Sweep @('-Spec', (Join-Path $dir 'Missing.tla'))
Check 'a missing spec: exit 2' ($missingSpec.Exit -eq 2) "exit $($missingSpec.Exit)"
$missingJar = Run-Sweep @('-Spec', $spec, '-Jar', (Join-Path $dir 'no-such-tla2tools.jar'))
Check 'a missing jar: exit 2' ($missingJar.Exit -eq 2) "exit $($missingJar.Exit)"

Report ('check-run-tlc-all: {0} failed' -f $failures)
$report
if ($failures -gt 0) { exit 1 }
exit 0
