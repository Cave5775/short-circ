# Refresh the copy of docs/FORMAT.md that is inlined at the end of the circuit skill.
# Everything in SKILL.md above the "## Format specification" heading is kept as is.
# Run from anywhere:  powershell -File scripts\sync-skill.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$skill = Join-Path $root '.claude\skills\circuit\SKILL.md'
$spec = Join-Path $root 'docs\FORMAT.md'
$marker = '## Format specification (complete, normative; generated from docs/FORMAT.md)'

$lines = [System.IO.File]::ReadAllLines($skill)
$idx = [Array]::IndexOf($lines, $marker)
if ($idx -lt 0) { throw "marker heading not found in $skill" }
$head = $lines[0..$idx]

# Demote every heading by two levels so the spec nests under the marker heading.
$body = [System.IO.File]::ReadAllLines($spec) | ForEach-Object { if ($_ -match '^#') { '##' + $_ } else { $_ } }

$out = @($head) + @('') + @($body)
[System.IO.File]::WriteAllText($skill, (($out -join "`n") + "`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Host "Updated $skill ($($out.Count) lines)"
