# Copyright (c) Michael Grier.
# The text edits of tools/run-probes.ps1: finding where a mutant's Old text occurs in a spec, and
# building the module text that replacing it gives.

# The variable names of the inside of a tuple: 'a, b, c' -> a, b, c.
function Split-Vars([string]$list) {
    return @($list -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
}

# The modules of the spec $specName in $specDir -- $specName.tla and every $specName*.tla -- as
# an ordered name -> text map with LF endings.
function Get-Chain([string]$specDir, [string]$specName) {
    $chain = [ordered]@{}
    $files = Get-ChildItem -LiteralPath $specDir -File | Where-Object { $_.Name -like "$specName*.tla" } | Sort-Object Name
    foreach ($file in $files) {
        $chain[$file.Name] = [IO.File]::ReadAllText($file.FullName).Replace("`r`n", "`n")
    }
    return $chain
}

# The modules in $specDir that the chain extends or instances and that are not part of it, and
# the ones those depend on in turn, as an ordered name -> path map. A probe copies them unchanged
# beside the mutated chain so the scratch copy parses; its edits never apply to them.
function Get-Dependencies([string]$specDir, $chain) {
    $deps = [ordered]@{}
    $pending = [System.Collections.Generic.Queue[string]]::new()
    foreach ($text in $chain.Values) { $pending.Enqueue($text) }
    while ($pending.Count -gt 0) {
        $text = $pending.Dequeue()
        $names = @()
        foreach ($m in [regex]::Matches($text, '(?m)^\s*EXTENDS\s+([^\r\n]+)')) {
            $names += @($m.Groups[1].Value -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
        }
        foreach ($m in [regex]::Matches($text, '\bINSTANCE\s+(\w+)')) { $names += $m.Groups[1].Value }
        foreach ($name in $names) {
            $file = "$name.tla"
            $path = Join-Path $specDir $file
            if ($chain.Contains($file) -or $deps.Contains($file) -or -not (Test-Path -LiteralPath $path)) { continue }
            $deps[$file] = $path
            $pending.Enqueue([IO.File]::ReadAllText($path))
        }
    }
    return $deps
}

# The top-level definitions of a module, each with the character range from its `Name ==` line
# to the next one. Comments are blanked, character for character, before the headers are found,
# so a definition-shaped line inside a comment is not one.
function Get-Definitions([string]$text) {
    $blank = [regex]::Replace($text, '(?s)\(\*.*?\*\)|\\\*[^\n]*', { param($m) $m.Value -replace '[^\n]', ' ' })
    $headers = [regex]::Matches($blank, '(?m)^(?:LOCAL\s+)?([A-Za-z_]\w*)(?:\([^)\n]*\)|\[[^\]\n]*\])?\s*==')
    $moduleEnd = [regex]::Match($blank, '(?m)^={4,}')
    $limit = if ($moduleEnd.Success) { $moduleEnd.Index } else { $text.Length }
    $definitions = @()
    for ($i = 0; $i -lt $headers.Count; $i++) {
        $stop = if ($i + 1 -lt $headers.Count) { $headers[$i + 1].Index } else { $limit }
        $start = $headers[$i].Index
        $definitions += [pscustomobject]@{ Name = $headers[$i].Groups[1].Value; Start = $start; End = [Math]::Max($stop, $start) }
    }
    return $definitions
}

# The name of the definition that contains character $index of a module, or $null.
function Get-EnclosingDefinition($definitions, [int]$index) {
    foreach ($definition in $definitions) {
        if ($definition.Start -le $index -and $index -lt $definition.End) { return $definition.Name }
    }
    return $null
}

# A pattern for $old in which each UNCHANGED tuple matches whatever its contents are, so that a
# variable added to a frame never makes the text stale.
function ConvertTo-EditPattern([string]$old) {
    $pattern = New-Object System.Text.StringBuilder
    foreach ($part in [regex]::Split($old, '(UNCHANGED\s*<<[^>]*>>)')) {
        if ($part -match '^UNCHANGED\s*<<[^>]*>>$') { [void]$pattern.Append('UNCHANGED\s*<<([^>]*)>>') }
        else { [void]$pattern.Append([regex]::Escape($part)) }
    }
    return [regex]$pattern.ToString()
}

# The text that replaces a match of $old's pattern: $new, except that where it has an UNCHANGED
# tuple that pairs with one of $old's, the spec's own tuple has the variables $new drops removed
# and the ones it adds appended. Returns Text, or Error when a variable to remove is not in the
# spec's tuple.
function Get-Replacement($match, [string]$old, [string]$new) {
    $oldTuples = @([regex]::Matches($old, 'UNCHANGED\s*<<([^>]*)>>') | ForEach-Object { , (Split-Vars $_.Groups[1].Value) })
    $text = New-Object System.Text.StringBuilder
    $k = 0
    foreach ($part in [regex]::Split($new, '(UNCHANGED\s*<<[^>]*>>)')) {
        $tuple = [regex]::Match($part, '^UNCHANGED\s*<<([^>]*)>>$')
        if (-not $tuple.Success) { [void]$text.Append($part); continue }
        if ($k -ge $oldTuples.Count) { [void]$text.Append($part); $k++; continue }
        $newVars = Split-Vars $tuple.Groups[1].Value
        $oldVars = $oldTuples[$k]
        $actual = Split-Vars $match.Groups[$k + 1].Value
        $removed = @($oldVars | Where-Object { $_ -notin $newVars })
        $added = @($newVars | Where-Object { $_ -notin $oldVars })
        foreach ($variable in $removed) {
            if ($variable -notin $actual) { return [pscustomobject]@{ Text = $null; Error = "the spec's frame has no $variable to remove" } }
        }
        $kept = @($actual | Where-Object { $_ -notin $removed }) + @($added | Where-Object { $_ -notin $actual })
        [void]$text.Append('UNCHANGED <<' + ($kept -join ', ') + '>>')
        $k++
    }
    return [pscustomobject]@{ Text = $text.ToString(); Error = $null }
}

# Where $old occurs exactly once: in the modules of $chain (name -> text), or in the body of the
# definition $def when one is named. A place whose UNCHANGED tuple lacks a variable $new drops is
# not an occurrence, so an edit of a frame is found by the variables it changes. $definitions maps
# each module name to its Get-Definitions. Returns Ok = $true with the Module, the Index and Length
# of the match and the Replacement text, or Ok = $false with a Reason.
function Find-Edit($chain, $definitions, [string]$old, [string]$new, [string]$def) {
    $pattern = ConvertTo-EditPattern $old
    $hits = @()
    if ($def) {
        $homes = @(foreach ($module in $chain.Keys) {
            foreach ($d in $definitions[$module]) { if ($d.Name -eq $def) { [pscustomobject]@{ Module = $module; Definition = $d } } }
        })
        if ($homes.Count -eq 0) { return [pscustomobject]@{ Ok = $false; Reason = "definition $def not found" } }
        if ($homes.Count -gt 1) { return [pscustomobject]@{ Ok = $false; Reason = "definition $def is defined $($homes.Count) times" } }
        $d = $homes[0].Definition
        $region = $chain[$homes[0].Module].Substring($d.Start, $d.End - $d.Start)
        foreach ($m in $pattern.Matches($region)) {
            $hits += [pscustomobject]@{ Module = $homes[0].Module; Index = $d.Start + $m.Index; Match = $m }
        }
    } else {
        foreach ($module in $chain.Keys) {
            foreach ($m in $pattern.Matches($chain[$module])) { $hits += [pscustomobject]@{ Module = $module; Index = $m.Index; Match = $m } }
        }
    }
    if ($hits.Count -eq 0) { return [pscustomobject]@{ Ok = $false; Reason = 'text not found' } }
    $usable = @()
    $firstError = $null
    foreach ($hit in $hits) {
        $replacement = Get-Replacement $hit.Match $old $new
        if ($replacement.Error) { if (-not $firstError) { $firstError = $replacement.Error } }
        else { $usable += [pscustomobject]@{ Hit = $hit; Replacement = $replacement } }
    }
    if ($usable.Count -eq 0) { return [pscustomobject]@{ Ok = $false; Reason = $firstError } }
    if ($usable.Count -gt 1) { return [pscustomobject]@{ Ok = $false; Reason = "text found $($usable.Count) times" } }
    $hit = $usable[0].Hit
    return [pscustomobject]@{
        Ok = $true; Module = $hit.Module; Index = $hit.Index; Length = $hit.Match.Length; Replacement = $usable[0].Replacement.Text
    }
}

# For a mutant whose text is not found, where its first line is, so that it can be re-expressed.
function Get-StaleHint($chain, [string]$old) {
    $first = $old -split "`n" | ForEach-Object { $_.Trim() } |
        Where-Object { $_ -and $_ -notmatch '^UNCHANGED' } | Select-Object -First 1
    if (-not $first) { return '' }
    foreach ($module in $chain.Keys) {
        $lines = $chain[$module] -split "`n"
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if ($lines[$i].Trim() -eq $first) { return "its first line is at ${module}:$($i + 1)" }
        }
    }
    return 'its first line is not in the spec'
}
