# Copyright (c) Michael Grier.
# What the tools that run TLC share: what a TLC output file says happened.

# The outcome TLC's output records: PASS, the name of the first violated invariant or
# property, DEADLOCK when it found a state with no successor, NO-OUTPUT when TLC never wrote
# one, or OTHER for anything else (a parse error).
function Get-Outcome([string]$outFile) {
    if (-not (Test-Path -LiteralPath $outFile)) { return 'NO-OUTPUT' }
    # TLC names a violated action property by its source location, not its name.
    $located = Select-String -LiteralPath $outFile -Pattern '^Error: (?:Action )?[Pp]roperty line (\d+), col \d+ to line \d+, col \d+ of module (\S+) is violated' |
        Select-Object -First 1
    if ($located) { return Get-DefinitionAt $outFile $located.Matches[0].Groups[2].Value ([int]$located.Matches[0].Groups[1].Value) }
    $violated = Select-String -LiteralPath $outFile -Pattern '^Error: .* (\S+) is violated' |
        Select-Object -First 1
    if ($violated) { return $violated.Matches[0].Groups[1].Value }
    $temporal = Select-String -LiteralPath $outFile -Pattern '^Error: Temporal property (\S+) was violated' |
        Select-Object -First 1
    if ($temporal) { return $temporal.Matches[0].Groups[1].Value }
    $deadlock = Select-String -LiteralPath $outFile -Pattern '^Error: Deadlock reached\.' |
        Select-Object -First 1
    if ($deadlock) { return 'DEADLOCK' }
    $clean = Select-String -LiteralPath $outFile -Pattern 'No error has been found' |
        Select-Object -First 1
    if ($clean) { return 'PASS' }
    return 'OTHER'
}

# The name of the definition in module $module that holds line $line, read from the file TLC
# parsed for it; the module's name when that file cannot be found.
function Get-DefinitionAt([string]$outFile, [string]$module, [int]$line) {
    $parsed = Select-String -LiteralPath $outFile -Pattern ('^Parsing file (.*[\\/]' + [regex]::Escape($module) + '\.tla)$') |
        Select-Object -First 1
    if (-not $parsed) { return $module }
    $path = $parsed.Matches[0].Groups[1].Value
    if (-not (Test-Path -LiteralPath $path)) { return $module }
    $lines = @(Get-Content -LiteralPath $path)
    for ($i = [Math]::Min($line, $lines.Count) - 1; $i -ge 0; $i--) {
        if ($lines[$i] -match '^([A-Za-z_]\w*)\s*(\(.*\))?\s*==') { return $Matches[1] }
    }
    return $module
}
