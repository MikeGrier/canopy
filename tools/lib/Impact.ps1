# Copyright (c) Michael Grier.
# The impact analysis behind the -Changed option of tools/run-tlc-all.ps1 and tools/run-probes.ps1:
# the definitions of a spec each configuration reaches, a digest of what it reaches, and the record
# of the last run of each configuration and probe that came out as its header or mutant requires.
#
# References are resolved by name. The modules a spec extends share one namespace; a module it
# instances has a namespace of its own. A reference I!D or I(args)!D, where I is defined as an
# INSTANCE of M, reaches M's D and I's own definition, which holds the substitutions. A name that a
# LET or a bound variable shadows is still taken as a reference, so what a configuration reaches is
# never less than what TLC evaluates, though it can be more. Comments are not part of a definition,
# so an edit to a comment changes no digest. Everything outside a definition -- EXTENDS,
# VARIABLES, CONSTANTS, ASSUME -- is the namespace's declarations, and a change to them changes
# the digest of every configuration that reaches the namespace.
#
# The record is git-ignored state under .scratch/tlc/record, one file per spec and per tool, and
# never a checked-in document.

# A module's text with comments blanked character for character, and the same with strings
# blanked too, for finding references.
function Get-TlaBlanked([string]$text) {
    $blank = [regex]::Replace($text, '(?s)\(\*.*?\*\)|\\\*[^\n]*', { param($m) $m.Value -replace '[^\n]', ' ' })
    $bare = [regex]::Replace($blank, '"(?:[^"\\\n]|\\.)*"', { param($m) ' ' * $m.Value.Length })
    return [pscustomobject]@{ Blank = $blank; Bare = $bare }
}

# TLA+ names are case-sensitive, and a PowerShell hash table is not.
function New-OrdinalTable { return [System.Collections.Hashtable]::new([StringComparer]::Ordinal) }

function Get-ShortDigest([string]$text) {
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($text)
    $hash = [System.Security.Cryptography.SHA256]::HashData($bytes)
    return [Convert]::ToHexString($hash, 0, 8).ToLowerInvariant()
}

function Get-Normalized([string]$text) { return ($text -replace '\s+', ' ').Trim() }

# One module in $specDir: its definitions (name -> Digest, Bare body, Instance module or $null),
# the modules it extends, and the digest of its declarations. $null when the module is not in
# $specDir, as the standard modules are not.
function Read-TlaModule([string]$specDir, [string]$module) {
    $path = Join-Path $specDir "$module.tla"
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    $text = [IO.File]::ReadAllText($path).Replace("`r`n", "`n")
    $blanked = Get-TlaBlanked $text
    $units = [regex]::Matches($blanked.Blank,
        '(?m)^(?:(?:LOCAL\s+)?(?<name>[A-Za-z_]\w*)\s*(?:\([^)\n]*\)|\[[^\]\n]*\])?\s*==|(?:VARIABLES?|CONSTANTS?|ASSUME|ASSUMPTION|AXIOM|THEOREM|LEMMA|EXTENDS|RECURSIVE|LOCAL\s+INSTANCE|INSTANCE)\b|-{4,}|={4,})')
    $defs = New-OrdinalTable
    $decl = New-Object System.Text.StringBuilder
    $extends = @()
    if ($units.Count -gt 0) { [void]$decl.Append($blanked.Blank.Substring(0, $units[0].Index)) }
    for ($i = 0; $i -lt $units.Count; $i++) {
        $start = $units[$i].Index
        $end = if ($i + 1 -lt $units.Count) { $units[$i + 1].Index } else { $text.Length }
        $unit = $blanked.Blank.Substring($start, $end - $start)
        $name = $units[$i].Groups['name']
        if ($name.Success) {
            $bare = $blanked.Bare.Substring($start, $end - $start)
            $instance = [regex]::Match($bare, '==\s*(?:LOCAL\s+)?INSTANCE\s+(\w+)')
            $defs[$name.Value] = [pscustomobject]@{
                Digest = Get-ShortDigest (Get-Normalized $unit)
                Bare = $bare
                Instance = $(if ($instance.Success) { $instance.Groups[1].Value } else { $null })
            }
        } else {
            [void]$decl.Append($unit)
            $import = [regex]::Match($unit, '^(?:EXTENDS|INSTANCE|LOCAL\s+INSTANCE)\s+([\w\s,]+?)(?:\bWITH\b|$)')
            if ($import.Success) { $extends += @($import.Groups[1].Value -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }) }
        }
    }
    return [pscustomobject]@{ Defs = $defs; Extends = $extends; Decl = Get-ShortDigest (Get-Normalized $decl.ToString()) }
}

# The namespace of $root: the definitions of every module of $specDir in the closure of $root's
# EXTENDS (and of any INSTANCE without a name), with the digest of their declarations together.
function Read-TlaNamespace([string]$specDir, [string]$root) {
    $defs = New-OrdinalTable
    $decls = @()
    $seen = @{}
    $pending = [System.Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($root)
    while ($pending.Count -gt 0) {
        $module = $pending.Dequeue()
        if ($seen.ContainsKey($module)) { continue }
        $seen[$module] = $true
        $parsed = Read-TlaModule $specDir $module
        if (-not $parsed) { continue }
        $decls += "$module=$($parsed.Decl)"
        foreach ($name in $parsed.Defs.Keys) { $defs[$name] = $parsed.Defs[$name] }
        foreach ($next in $parsed.Extends) { $pending.Enqueue($next) }
    }
    return [pscustomobject]@{ Root = $root; Defs = $defs; Decl = Get-ShortDigest (($decls | Sort-Object) -join "`n"); Refs = New-OrdinalTable }
}

# The impact model of the spec whose root module is $root: its namespace and the namespace of every
# module it instances, transitively, keyed by root module.
function Get-ImpactModel([string]$specDir, [string]$root) {
    $spaces = @{}
    $pending = [System.Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($root)
    while ($pending.Count -gt 0) {
        $key = $pending.Dequeue()
        if ($spaces.ContainsKey($key)) { continue }
        $space = Read-TlaNamespace $specDir $key
        $spaces[$key] = $space
        foreach ($def in $space.Defs.Values) { if ($def.Instance) { $pending.Enqueue($def.Instance) } }
    }
    return [pscustomobject]@{ Root = $root; Spaces = $spaces; Reach = New-OrdinalTable }
}

# The names a piece of TLA+ may refer to. A subscript is written against its expression, as
# [][Next]_vars and WF_vars(A), so those words also name what follows the underscore.
function Get-NameTokens([string]$text) {
    foreach ($m in [regex]::Matches($text, '\w+')) {
        $word = $m.Value
        $word
        if ($word.StartsWith('_')) { $word.TrimStart('_') }
        $fairness = [regex]::Match($word, '^(?:WF|SF)_(\w+)$')
        if ($fairness.Success) { $fairness.Groups[1].Value }
    }
}

# The definitions a definition refers to, as "namespace::name" keys.
function Get-DefinitionRefs($model, [string]$space, [string]$name) {
    $ns = $model.Spaces[$space]
    if ($ns.Refs.ContainsKey($name)) { return $ns.Refs[$name] }
    $refs = [System.Collections.Generic.HashSet[string]]::new()
    $bare = $ns.Defs[$name].Bare
    foreach ($m in [regex]::Matches($bare, '\b([A-Za-z_]\w*)\s*(?:\((?:[^()]|\((?:[^()]|\([^()]*\))*\))*\))?\s*!\s*([A-Za-z_]\w*)')) {
        $instance = $ns.Defs[$m.Groups[1].Value]
        if ($instance -and $instance.Instance -and $model.Spaces[$instance.Instance].Defs.ContainsKey($m.Groups[2].Value)) {
            [void]$refs.Add("$($instance.Instance)::$($m.Groups[2].Value)")
        }
    }
    foreach ($word in (Get-NameTokens $bare)) {
        if ($ns.Defs.ContainsKey($word)) { [void]$refs.Add("${space}::$word") }
    }
    $ns.Refs[$name] = $refs
    return $refs
}

# The names a configuration file mentions that are definitions of the root namespace: its
# specification or Init and Next, its invariants and properties, and any definition a constant is
# replaced by.
function Get-ConfigRoots($model, [string]$cfgPath) {
    $text = [IO.File]::ReadAllText($cfgPath).Replace("`r`n", "`n")
    $bare = (Get-TlaBlanked $text).Bare
    $defs = $model.Spaces[$model.Root].Defs
    return @(Get-NameTokens $bare | Where-Object { $defs.ContainsKey($_) } | Sort-Object -Unique)
}

# What a configuration reaches, as a map "namespace::name" -> digest, and the namespaces it
# touches; memoized by the set of names it starts from.
function Get-Reach($model, [string[]]$roots) {
    $memo = ($roots | Sort-Object) -join ','
    if ($model.Reach.ContainsKey($memo)) { return $model.Reach[$memo] }
    $reached = New-OrdinalTable
    $pending = [System.Collections.Generic.Stack[string]]::new()
    foreach ($name in $roots) { $pending.Push("$($model.Root)::$name") }
    while ($pending.Count -gt 0) {
        $key = $pending.Pop()
        if ($reached.ContainsKey($key)) { continue }
        $space, $name = $key -split '::', 2
        $reached[$key] = $model.Spaces[$space].Defs[$name].Digest
        foreach ($ref in (Get-DefinitionRefs $model $space $name)) { if (-not $reached.ContainsKey($ref)) { $pending.Push($ref) } }
    }
    $spaces = @($model.Root) + @($reached.Keys | ForEach-Object { ($_ -split '::', 2)[0] }) | Sort-Object -Unique
    $result = [pscustomobject]@{ Defs = $reached; Spaces = $spaces }
    $model.Reach[$memo] = $result
    return $result
}

# The digest of the jar, which stands for the TLC version; cached per run of a tool.
$script:jarDigests = @{}
function Get-JarDigest([string]$jar) {
    if (-not $script:jarDigests.ContainsKey($jar)) {
        $script:jarDigests[$jar] = if (Test-Path -LiteralPath $jar) { (Get-FileHash -LiteralPath $jar -Algorithm SHA256).Hash.Substring(0, 16).ToLowerInvariant() } else { 'missing' }
    }
    return $script:jarDigests[$jar]
}

# A configuration's footprint: the digest of everything its outcome depends on -- its own text,
# the TLC version, every definition it reaches and the declarations of every namespace it
# touches -- and those parts, for saying why it was selected.
function Get-ConfigFootprint($model, [string]$cfgPath, [string]$jar) {
    $reach = Get-Reach $model (Get-ConfigRoots $model $cfgPath)
    $cfg = Get-ShortDigest ([IO.File]::ReadAllText($cfgPath).Replace("`r`n", "`n"))
    $tlc = Get-JarDigest $jar
    $decls = @{}
    foreach ($space in $reach.Spaces) { $decls[$space] = $model.Spaces[$space].Decl }
    $lines = @("cfg=$cfg", "tlc=$tlc") +
        @($decls.Keys | Sort-Object | ForEach-Object { "decl $_=$($decls[$_])" }) +
        @($reach.Defs.Keys | Sort-Object | ForEach-Object { "def $_=$($reach.Defs[$_])" })
    return [pscustomobject]@{ Digest = Get-ShortDigest ($lines -join "`n"); Cfg = $cfg; Tlc = $tlc; Decls = $decls; Defs = $reach.Defs }
}

# The snapshot of a model: every definition's digest and every namespace's declarations, from which
# a later run says what changed.
function Get-ModelSnapshot($model, [string]$jar) {
    $snapshot = New-OrdinalTable
    $snapshot['#tlc'] = Get-JarDigest $jar
    foreach ($space in $model.Spaces.Keys) {
        $snapshot["$space#decl"] = $model.Spaces[$space].Decl
        foreach ($name in $model.Spaces[$space].Defs.Keys) { $snapshot["${space}::$name"] = $model.Spaces[$space].Defs[$name].Digest }
    }
    return $snapshot
}

# Why a configuration is selected against its record entry, or $null when it is not.
function Get-SelectionReason($footprint, $entry, $snapshots) {
    if (-not $entry) { return 'no run on record' }
    if ($entry.Digest -eq $footprint.Digest) { return $null }
    $reasons = @()
    if ($entry.Cfg -ne $footprint.Cfg) { $reasons += 'its .cfg changed' }
    $snapshot = if ($entry.Snapshot -and $snapshots.ContainsKey($entry.Snapshot)) { $snapshots[$entry.Snapshot] } else { $null }
    if (-not $snapshot) { $reasons += 'its record has no snapshot'; return ($reasons -join '; ') }
    if ($snapshot['#tlc'] -ne $footprint.Tlc) { $reasons += 'TLC changed' }
    foreach ($space in ($footprint.Decls.Keys | Sort-Object)) {
        if ($snapshot["$space#decl"] -ne $footprint.Decls[$space]) { $reasons += "the declarations of $space changed" }
    }
    $changed = @($footprint.Defs.Keys | Sort-Object | Where-Object { $snapshot[$_] -ne $footprint.Defs[$_] } |
        ForEach-Object { $_ -replace '::', '!' })
    if ($changed.Count -gt 0) { $reasons += "changed: $($changed -join ', ')" }
    if ($reasons.Count -eq 0) { $reasons += 'its definitions were reached differently' }
    return ($reasons -join '; ')
}

# Where the record of a spec's runs by one tool lives: under .scratch/tlc/record, named for the
# spec and its directory, so that a scratch copy of a spec never shares a record with the real one.
function Get-RecordPath([string]$repoRoot, [string]$specPath, [string]$tool) {
    $dir = Split-Path -Parent $specPath
    $name = [System.IO.Path]::GetFileNameWithoutExtension($specPath)
    $where = Get-ShortDigest $dir.ToLowerInvariant()
    return Join-Path $repoRoot ".scratch/tlc/record/$name-$where.$tool.json"
}

function Read-ImpactRecord([string]$path) {
    if (Test-Path -LiteralPath $path) {
        $record = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json -AsHashtable
        foreach ($key in 'Snapshots', 'Entries') { if (-not $record.ContainsKey($key)) { $record[$key] = @{} } }
        foreach ($id in @($record.Snapshots.Keys)) {
            $snapshot = New-OrdinalTable
            foreach ($pair in $record.Snapshots[$id].GetEnumerator()) { $snapshot[$pair.Key] = $pair.Value }
            $record.Snapshots[$id] = $snapshot
        }
        return $record
    }
    return @{ Snapshots = @{}; Entries = @{} }
}

# Writes the record, keeping only the snapshots an entry still names.
function Save-ImpactRecord([string]$path, $record) {
    $used = @{}
    foreach ($entry in $record.Entries.Values) { if ($entry.Snapshot) { $used[$entry.Snapshot] = $true } }
    foreach ($id in @($record.Snapshots.Keys)) { if (-not $used.ContainsKey($id)) { $record.Snapshots.Remove($id) } }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($path, ($record | ConvertTo-Json -Depth 6 -Compress), $utf8)
}

# Adds a snapshot to the record and returns its id.
function Add-ImpactSnapshot($record, $snapshot) {
    $id = Get-ShortDigest (($snapshot.Keys | Sort-Object | ForEach-Object { "$_=$($snapshot[$_])" }) -join "`n")
    $record.Snapshots[$id] = $snapshot
    return $id
}

# The number of distinct states TLC's output reports, or $null.
function Get-DistinctStates([string]$outFile) {
    if (-not (Test-Path -LiteralPath $outFile)) { return $null }
    $found = Select-String -LiteralPath $outFile -Pattern '([\d,]+) distinct states found' | Select-Object -Last 1
    if (-not $found) { return $null }
    return [long]($found.Matches[0].Groups[1].Value -replace ',', '')
}
