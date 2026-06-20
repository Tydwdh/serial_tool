[CmdletBinding()]
param(
    [switch]$SkipPackage
)

$ErrorActionPreference = "Stop"

function Resolve-IsccPath {
    if ($env:INNOSETUP_ISCC -and (Test-Path -LiteralPath $env:INNOSETUP_ISCC -PathType Leaf)) {
        return $env:INNOSETUP_ISCC
    }

    $command = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }

    $candidates = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    )

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            return $candidate
        }
    }

    return $null
}

$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptRoot
$PackageScript = Join-Path $RepoRoot "package.bat"
$InstallerScript = Join-Path $ScriptRoot "hardware-workbench-app.iss"
$PortableExe = Join-Path $RepoRoot "dist\hardware-workbench-app\hardware-workbench-app.exe"
$InstallerExe = Join-Path $RepoRoot "dist\HardwareWorkbenchSetup.exe"

Push-Location $RepoRoot
try {
    if (-not $SkipPackage) {
        Write-Host "Building portable package..."
        & $PackageScript
        if ($LASTEXITCODE -ne 0) {
            throw "package.bat failed with exit code $LASTEXITCODE"
        }
    }

    if (-not (Test-Path -LiteralPath $PortableExe -PathType Leaf)) {
        throw "Portable executable not found: $PortableExe"
    }

    $IsccPath = Resolve-IsccPath
    if (-not $IsccPath) {
        throw "ISCC.exe was not found. Install Inno Setup 6 or set INNOSETUP_ISCC to the full ISCC.exe path."
    }

    Write-Host "Compiling installer with $IsccPath..."
    & $IsccPath $InstallerScript
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup failed with exit code $LASTEXITCODE"
    }

    if (-not (Test-Path -LiteralPath $InstallerExe -PathType Leaf)) {
        throw "Installer was not created: $InstallerExe"
    }

    Write-Host "Installer created: $InstallerExe"
}
finally {
    Pop-Location
}
