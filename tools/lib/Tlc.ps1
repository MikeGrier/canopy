# Copyright (c) Michael Grier.
# What the tools that run TLC share: what a TLC output file says happened.

# The outcome TLC's output records: PASS, the name of the first violated invariant or
# property, DEADLOCK when it found a state with no successor, NO-OUTPUT when TLC never wrote
# one, or OTHER for anything else (a parse error).
function Get-Outcome([string]$outFile) {
    if (-not (Test-Path -LiteralPath $outFile)) { return 'NO-OUTPUT' }
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
