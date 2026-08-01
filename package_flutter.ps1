# Creates a portable Flutter Windows release archive.
# Run from the repository root: .\package.bat

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ProjectRoot

$DistRoot = Join-Path $ProjectRoot 'dist'
$PackageDir = Join-Path $DistRoot 'hardware-workbench'
$ZipPath = Join-Path $DistRoot 'hardware-workbench-windows.zip'
$ReleaseDir = Join-Path $ProjectRoot 'flutter_app\build\windows\x64\runner\Release'

Write-Host '============================================' -ForegroundColor Cyan
Write-Host ' Hardware Workbench - Flutter portable pack' -ForegroundColor Cyan
Write-Host '============================================' -ForegroundColor Cyan

Write-Host '[1/3] Building Flutter Release...' -ForegroundColor Yellow
& (Join-Path $ProjectRoot 'build_flutter.ps1')
if ($LASTEXITCODE -ne 0) { throw 'Flutter Release 构建失败' }
if (-not (Test-Path (Join-Path $ReleaseDir 'hardware_workbench.exe'))) {
    throw "未找到 Release 输出: $ReleaseDir"
}

Write-Host '[2/3] Preparing portable directory...' -ForegroundColor Yellow
New-Item -ItemType Directory -Force -Path $DistRoot | Out-Null
if (Test-Path $PackageDir) { Remove-Item -LiteralPath $PackageDir -Recurse -Force }
if (Test-Path $ZipPath) { Remove-Item -LiteralPath $ZipPath -Force }
New-Item -ItemType Directory -Path $PackageDir | Out-Null
Copy-Item -Path (Join-Path $ReleaseDir '*') -Destination $PackageDir -Recurse -Force

if (Test-Path (Join-Path $ProjectRoot 'docs')) {
    Copy-Item -Path (Join-Path $ProjectRoot 'docs') -Destination (Join-Path $PackageDir 'docs') -Recurse -Force
}
New-Item -ItemType Directory -Force -Path (Join-Path $PackageDir 'plugins') | Out-Null

Write-Host '[3/3] Creating zip archive...' -ForegroundColor Yellow
Compress-Archive -Path $PackageDir -DestinationPath $ZipPath -Force

Write-Host ''
Write-Host '完成。' -ForegroundColor Green
Write-Host "便携目录: $PackageDir" -ForegroundColor Green
Write-Host "压缩包:   $ZipPath" -ForegroundColor Green
