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

function Get-AppVersion {
    $metadataText = & cargo metadata --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE"
    }

    $metadata = $metadataText | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.name -eq "hardware-workbench-app" } | Select-Object -First 1
    if ($null -eq $package -or [string]::IsNullOrWhiteSpace($package.version)) {
        throw "Could not find hardware-workbench-app version in cargo metadata."
    }

    return $package.version
}

$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptRoot
$PackageScript = Join-Path $RepoRoot "package.bat"
$InstallerScript = Join-Path $ScriptRoot "hardware-workbench-app.iss"
$InstallerLanguageFile = Join-Path $ScriptRoot "ChineseSimplified.isl"
$PortableExe = Join-Path $RepoRoot "dist\hardware-workbench-app\hardware-workbench-app.exe"
$InstallerExe = Join-Path $RepoRoot "dist\HardwareWorkbenchSetup.exe"

Push-Location $RepoRoot
try {
    if (-not (Test-Path -LiteralPath $InstallerLanguageFile -PathType Leaf)) {
        throw "Installer language file not found: $InstallerLanguageFile"
    }

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

    $AppVersion = Get-AppVersion
    Write-Host "Compiling installer $AppVersion with $IsccPath..."
    & $IsccPath "/DMyAppVersion=$AppVersion" $InstallerScript
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
