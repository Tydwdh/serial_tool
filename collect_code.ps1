$output = "code_dump.txt"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

$includes = @(
    "*.rs",
    "*.lua",
    "*.json",
    "*.toml"
    "*.c"
    "*.h"
)

$excludeDirs = @(
    "target",
    ".git",
    ".claude",
    "assets"
)

Set-Content -Path $output -Value ""

$files = Get-ChildItem -Path $root -Recurse -Include $includes `
    | Where-Object {
        foreach ($dir in $excludeDirs) {
            if ($_.FullName -match "\\$dir\\") { return $false }
        }
        return $true
    } `
    | Sort-Object FullName

foreach ($file in $files) {
    $relPath = $file.FullName.Substring($root.Length + 1) -replace '\\', '/'
    Add-Content -Path $output -Value "`n========================================"
    Add-Content -Path $output -Value "  $relPath"
    Add-Content -Path $output -Value "========================================"
    Add-Content -Path $output -Value (Get-Content -Path $file.FullName -Raw)
}

Write-Host "Done: $($files.Count) files -> $output"
