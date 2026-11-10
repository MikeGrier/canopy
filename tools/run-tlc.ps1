# Copyright (c) Michael Grier
#
# tools/run-tlc.ps1 -- model-check one TLA+ spec with TLC and fail on any violation.
#
# Usage:
#   pwsh tools/run-tlc.ps1 -Spec crates/canopy/spec/PointerPublication.tla
#   pwsh tools/run-tlc.ps1 -Spec <spec.tla> -Config <other.cfg> -Workers 4
#
# The config defaults to the spec's sibling `.cfg`. TLC's full output goes to
# .scratch/tlc/<config-name>.out and its state files under .scratch/tlc/states,
# so nothing is written beside the spec.
#
# Exit code is TLC's own: 0 when every invariant and property holds, non-zero
# for a violation, a deadlock, or a parse or semantic error -- so a spec check
# can gate a milestone the way `cargo test` does.
#
# Requires a JDK (11+) on PATH and tla2tools.jar; see the "TLA+ / TLC" section
# of .github/copilot-instructions.md for the pinned versions.

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$Spec,
    [string]$Config,
    [string]$Workers = 'auto',
    [string]$Jar = (Join-Path $env:USERPROFILE '.tla\1.8.0\tla2tools.jar')
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Spec)) {
    Write-Error "spec not found: $Spec"
    exit 2
}
if (-not $Config) {
    $Config = [System.IO.Path]::ChangeExtension($Spec, '.cfg')
}
if (-not (Test-Path -LiteralPath $Config)) {
    Write-Error "config not found: $Config"
    exit 2
}
if (-not (Test-Path -LiteralPath $Jar)) {
    Write-Error "tla2tools.jar not found at $Jar; see .github/copilot-instructions.md"
    exit 2
}
if (-not (Get-Command java -ErrorAction SilentlyContinue)) {
    Write-Error 'java not found on PATH; see .github/copilot-instructions.md'
    exit 2
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$outDir = Join-Path $repoRoot '.scratch\tlc'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$name = [System.IO.Path]::GetFileNameWithoutExtension($Config)
$outFile = Join-Path $outDir "$name.out"

$specPath = (Resolve-Path -LiteralPath $Spec).Path
$configPath = (Resolve-Path -LiteralPath $Config).Path

# TLC resolves EXTENDS against the spec's directory, so run from there.
Push-Location (Split-Path -Parent $specPath)
try {
    & java -XX:+UseParallelGC -cp $Jar tlc2.TLC `
        -config $configPath `
        -workers $Workers `
        -metadir (Join-Path $outDir 'states') `
        -cleanup `
        -noGenerateSpecTE `
        $specPath 2>&1 | Tee-Object -FilePath $outFile
    $code = $LASTEXITCODE
}
finally {
    Pop-Location
}

if ($code -eq 0) {
    Write-Host "TLC: no violations ($Config). Output: $outFile"
} else {
    Write-Host "TLC: exit code $code ($Config). Output: $outFile"
}
exit $code
