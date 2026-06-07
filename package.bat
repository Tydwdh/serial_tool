@echo off
setlocal enabledelayedexpansion

set NAME=hardware-workbench-app
set OUT_DIR=dist\%NAME%

echo ============================================
echo   Hardware Debug Workbench - Package Script
echo ============================================
echo.

echo [1/4] Building release...
cargo build --release
if %ERRORLEVEL% neq 0 (
    echo Build failed
    exit /b %ERRORLEVEL%
)
echo.

echo [2/4] Creating dist directory...
if exist "%OUT_DIR%" rmdir /s /q "%OUT_DIR%"
mkdir "%OUT_DIR%"

echo [3/4] Copying files...
copy "target\release\%NAME%.exe" "%OUT_DIR%\" >nul
echo   %NAME%.exe

xcopy "assets" "%OUT_DIR%\assets\" /E /I /Q >nul
echo   assets\

if exist "plugins" (
    xcopy "plugins" "%OUT_DIR%\plugins\" /E /I /Q >nul
    echo   plugins\
)

echo.
echo [4/4] Creating zip...
powershell -NoProfile -Command "Compress-Archive -Path '%OUT_DIR%' -DestinationPath 'dist\%NAME%.zip' -Force"

echo.
echo ============================================
echo   Done
echo   Portable: dist\%NAME%\
echo   Zip:      dist\%NAME%.zip
echo ============================================
endlocal
