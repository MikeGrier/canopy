# Copyright (c) Michael Grier.
#
# tools/fmt-header.ps1 -- print spec header-comment lines at the box width, to paste into a .tla file.
#
# Usage:
#   pwsh tools/fmt-header.ps1 -Text '...'                    a paragraph
#   pwsh tools/fmt-header.ps1 -Control 'name' -Text '...'    a control entry
#
# Every line is `(* `, the text padded to 71 columns and ` *)`, the box the header comments of the
# Coordination modules use. A control entry puts its quoted name in the first 29 columns and wraps
# its description in the rest; a name too long for them takes a line of its own.

param([string]$Text, [string]$Control = '')

$width = 71
$indent = 29
$lines = New-Object System.Collections.Generic.List[string]

function Add-Box([string]$s) { $lines.Add('(* ' + $s.PadRight($width) + ' *)') }

# Greedy word wrap into lines of at most $limit characters.
function Wrap([string[]]$words, [int]$limit) {
    $wrapped = @()
    $line = ''
    foreach ($word in $words) {
        if (($line + ' ' + $word).Trim().Length -gt $limit) { $wrapped += $line; $line = $word }
        else { $line = ($line + ' ' + $word).Trim() }
    }
    return $wrapped + $line
}

if ($Control -eq '') {
    foreach ($line in (Wrap $Text.Split(' ') $width)) { Add-Box $line }
} else {
    $name = '  "' + $Control + '"'
    $wrapped = @(Wrap $Text.Split(' ') ($width - $indent - 1))
    if ($name.Length + 1 -gt $indent) {
        Add-Box $name
        foreach ($line in $wrapped) { Add-Box ((' ' * $indent) + $line) }
    } else {
        Add-Box ($name.PadRight($indent) + $wrapped[0])
        foreach ($line in ($wrapped | Select-Object -Skip 1)) { if ($line) { Add-Box ((' ' * $indent) + $line) } }
    }
}
$lines
