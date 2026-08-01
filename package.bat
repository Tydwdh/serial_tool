@echo off
setlocal enabledelayedexpansion

set NAME=hardware-workbench-app
set OUT_ROOT=dist
set OUT_DIR=%OUT_ROOT%\%NAME%
set ZIP_PATH=%OUT_ROOT%\%NAME%.zip

echo ============================================
echo   Hardware Workbench - Package Script
echo ============================================
echo.

echo [1/5] Building Rust release...
cargo build -p %NAME% --release
if %ERRORLEVEL% neq 0 (
    echo Build failed
    exit /b %ERRORLEVEL%
)

echo [2/5] Preparing dist directory...
if not exist "%OUT_ROOT%" mkdir "%OUT_ROOT%"
if exist "%OUT_DIR%" rmdir /s /q "%OUT_DIR%"
if exist "%ZIP_PATH%" del /q "%ZIP_PATH%"
mkdir "%OUT_DIR%"
if %ERRORLEVEL% neq 0 (
    echo Failed to create dist directory
    exit /b %ERRORLEVEL%
)

echo [3/5] Copying runtime files...
copy "target\release\%NAME%.exe" "%OUT_DIR%\" >nul
if %ERRORLEVEL% neq 0 (
    echo Failed to copy executable
    exit /b %ERRORLEVEL%
)

mkdir "%OUT_DIR%\assets" >nul 2>nul
for %%F in (
    JetBrainsMonoNerdFontMono-Regular.ttf
    NotoSansSC-VF.ttf
    app-icon.ico
    FONT_LICENSES.md
    OFL-1.1.txt
) do (
    if exist "assets\%%F" copy "assets\%%F" "%OUT_DIR%\assets\%%F" >nul
)

if exist "docs" xcopy "docs" "%OUT_DIR%\docs\" /E /I /Q /Y >nul
if exist "assets\themes" xcopy "assets\themes" "%OUT_DIR%\themes\" /E /I /Q /Y >nul

for %%F in (README.md CHANGELOG.md LICENSE THIRD_PARTY_NOTICES.md) do (
    if exist "%%F" copy "%%F" "%OUT_DIR%\%%F" >nul
)
mkdir "%OUT_DIR%\licenses" >nul 2>nul
if exist "vendor\egui_tiles\LICENSE-MIT" copy "vendor\egui_tiles\LICENSE-MIT" "%OUT_DIR%\licenses\egui_tiles-LICENSE-MIT" >nul
if exist "installer\ChineseSimplified-LICENSE-MIT" copy "installer\ChineseSimplified-LICENSE-MIT" "%OUT_DIR%\licenses\ChineseSimplified-Translation-LICENSE-MIT" >nul

mkdir "%OUT_DIR%\plugins" >nul 2>nul
if exist "plugins\plugin.schema.json" copy "plugins\plugin.schema.json" "%OUT_DIR%\plugins\plugin.schema.json" >nul
if exist "plugins\.lua" xcopy "plugins\.lua" "%OUT_DIR%\plugins\.lua\" /E /I /Q /Y >nul
if exist ".luarc.json" copy ".luarc.json" "%OUT_DIR%\.luarc.json" >nul

set "EXAMPLES=%OUT_DIR%\examples\plugins"
if exist "plugins\plugin.schema.json" (
    mkdir "%EXAMPLES%" >nul 2>nul
    copy "plugins\plugin.schema.json" "%EXAMPLES%\plugin.schema.json" >nul
)
if exist "plugins\.lua" xcopy "plugins\.lua" "%EXAMPLES%\.lua\" /E /I /Q /Y >nul
for %%P in (template.hello template.serial-chart template.file-tool) do (
    if exist "plugins\%%P" xcopy "plugins\%%P" "%EXAMPLES%\%%P\" /E /I /Q /Y >nul
)
mkdir "%OUT_DIR%\logs" >nul 2>nul

echo [4/5] Creating zip...
powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "Compress-Archive -Path '%OUT_DIR%' -DestinationPath '%ZIP_PATH%' -Force"
if %ERRORLEVEL% neq 0 (
    echo Failed to create zip
    exit /b %ERRORLEVEL%
)

echo [5/5] Done
echo Portable: %OUT_DIR%\
echo Zip:      %ZIP_PATH%
endlocal
