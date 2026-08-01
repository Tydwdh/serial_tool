# 硬件调试工作台 - 构建脚本 (PowerShell)
# 在项目根目录执行: .\build_flutter.ps1

$ErrorActionPreference = "Stop"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 硬件调试工作台 - 构建脚本" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# 切换到项目根目录
$ProjectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ProjectRoot

# 1. 构建 Rust 后端
Write-Host "[1/3] 构建 Rust 后端..." -ForegroundColor Yellow
& cargo build -p tool-backend --release
if ($LASTEXITCODE -ne 0) {
    Write-Host "[错误] Rust 后端构建失败!" -ForegroundColor Red
    exit 1
}
Write-Host "OK Rust 后端构建成功" -ForegroundColor Green

# 2. 复制 DLL 到 Flutter 项目
Write-Host "[2/3] 复制 DLL 到 Flutter 项目..." -ForegroundColor Yellow
$RustDll = "target\release\tool_backend.dll"
$FlutterDir = "flutter_app"

if (Test-Path $RustDll) {
    Copy-Item -Path $RustDll -Destination "$FlutterDir\windows\runner\" -Force
    Write-Host "OK DLL 已复制到 $FlutterDir\windows\runner\" -ForegroundColor Green
} else {
    Write-Host "[警告] DLL 未找到: $RustDll" -ForegroundColor Yellow
}

# 3. 构建 Flutter 应用
Write-Host "[3/3] 构建 Flutter 应用..." -ForegroundColor Yellow
Push-Location $FlutterDir
& flutter build windows --release
if ($LASTEXITCODE -ne 0) {
    Write-Host "[错误] Flutter 构建失败!" -ForegroundColor Red
    Pop-Location
    exit 1
}
Pop-Location

# 复制 DLL 到最终输出目录
$BuildOutput = "$FlutterDir\build\windows\x64\runner\Release"
if (Test-Path $RustDll) {
    Copy-Item -Path $RustDll -Destination "$BuildOutput\" -Force
    Write-Host "OK DLL 已复制到 $BuildOutput" -ForegroundColor Green
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 构建完成!" -ForegroundColor Green
Write-Host " 输出目录: $BuildOutput" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Cyan
