# Copyright (c) Michael Grier.
#
# tools/gen-coordination-cfgs.ps1 -- write every crates/canopy/spec/Coordination*.cfg from tables.
#
# Usage:
#   pwsh tools/gen-coordination-cfgs.ps1
#
# Each configuration carries the same constants, the invariants and properties its kind requires,
# and a header that states what TLC must report and, for a slow one, `Tier: milestone` or
# `Tier: evidence`; both are
# read by tools/run-tlc-all.ps1. The tables are the parts under tools/gen-coordination-cfgs/ --
# constants, main configurations, controls, witnesses, liveness, callbacks and notifications -- which this script
# runs in that order,
# in its own scope. It removes every existing Coordination*.cfg first, so a configuration dropped
# from a table leaves the spec directory. Change a configuration by changing its table, never the
# generated file; regenerating changes no file when the tables have not changed.

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$spec = Join-Path $root 'crates/canopy/spec'
$utf8 = New-Object System.Text.UTF8Encoding($false)
$parts = Join-Path $PSScriptRoot 'gen-coordination-cfgs'

. (Join-Path $parts 'Constants.ps1')
Get-ChildItem $spec -Filter 'Coordination*.cfg' | Remove-Item
. (Join-Path $parts 'Mains.ps1')
. (Join-Path $parts 'Controls.ps1')
. (Join-Path $parts 'Witnesses.ps1')
. (Join-Path $parts 'Liveness.ps1')
. (Join-Path $parts 'Callbacks.ps1')
. (Join-Path $parts 'Notifications.ps1')

$missing = @($milestoneTier + $evidenceTier | Where-Object { -not (Test-Path (Join-Path $spec "$_.cfg")) })
if ($missing.Count -gt 0) { throw "a tier names no configuration: $($missing -join ', ')" }
'wrote {0} configurations' -f (Get-ChildItem $spec -Filter 'Coordination*.cfg').Count
