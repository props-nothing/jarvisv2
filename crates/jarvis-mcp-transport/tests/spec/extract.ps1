# Builds a derived slice of the official MCP schema, plus its provenance.
#
# The slice is computed as the transitive `$defs` closure reachable from the result definitions this server
# actually emits, rather than hand-picked: a hand-picked set is how a slice quietly stops covering the field
# that changes. Run from the repository root. The input file is the official schema, downloaded separately so
# the network is touched once and the exact bytes are visible.
param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination,
    [string[]]$Roots = @('ListToolsResult', 'DiscoverResult'),
    [string]$Revision = '2026-07-28'
)

$schema = [System.IO.File]::ReadAllText($Source, [Text.Encoding]::UTF8) | ConvertFrom-Json
$all = $schema.'$defs'
if (-not $all) { throw "the source has no `$defs" }

# Transitive closure over "#/$defs/<Name>" references.
$seen = [System.Collections.Generic.HashSet[string]]::new()
$queue = [System.Collections.Generic.Queue[string]]::new()
foreach ($root in $Roots) {
    if (-not $all.PSObject.Properties.Name.Contains($root)) { throw "the source has no definition $root" }
    [void]$queue.Enqueue($root)
}
while ($queue.Count -gt 0) {
    $name = $queue.Dequeue()
    if (-not $seen.Add($name)) { continue }
    $text = $all.$name | ConvertTo-Json -Depth 100 -Compress
    foreach ($match in [regex]::Matches($text, '#/\$defs/([A-Za-z0-9_-]+)')) {
        $target = $match.Groups[1].Value
        if (-not $seen.Contains($target)) { [void]$queue.Enqueue($target) }
    }
}

$defs = [ordered]@{}
foreach ($name in ($seen | Sort-Object)) { $defs[$name] = $all.$name }

$slice = [ordered]@{
    '$schema'  = 'https://json-schema.org/draft/2020-12/schema'
    '$comment' = 'derived slice of the MCP ' + $Revision + ' schema; see README.md'
    '$defs'    = $defs
}

$json = $slice | ConvertTo-Json -Depth 100
[System.IO.File]::WriteAllText($Destination, $json + "`n", (New-Object System.Text.UTF8Encoding($false)))
Write-Output ("wrote {0} definitions: {1}" -f $seen.Count, (($seen | Sort-Object) -join ', '))
