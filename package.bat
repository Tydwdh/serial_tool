@echo off
setlocal enabledelayedexpansion

set NAME=hardware-workbench-app
set OUT_ROOT=dist
set OUT_DIR=%OUT_ROOT%\%NAME%
set ZIP_PATH=%OUT_ROOT%\%NAME%.zip

echo ============================================
echo   Hardware Debug Workbench - Package Script
echo ============================================
echo.

echo [1/5] Building release...
cargo build --release
if %ERRORLEVEL% neq 0 (
    echo Build failed
    exit /b %ERRORLEVEL%
)
echo.

echo [2/5] Preparing dist directory...
if not exist "%OUT_ROOT%" mkdir "%OUT_ROOT%"

if exist "%OUT_DIR%" (
    rmdir /s /q "%OUT_DIR%"
    if %ERRORLEVEL% neq 0 (
        echo Failed to remove old dist directory
        exit /b %ERRORLEVEL%
    )
)

if exist "%ZIP_PATH%" (
    del /q "%ZIP_PATH%"
    if %ERRORLEVEL% neq 0 (
        echo Failed to remove old zip file
        exit /b %ERRORLEVEL%
    )
)

mkdir "%OUT_DIR%"
if %ERRORLEVEL% neq 0 (
    echo Failed to create dist directory
    exit /b %ERRORLEVEL%
)

echo.

echo [3/5] Copying runtime files...

copy "target\release\%NAME%.exe" "%OUT_DIR%\" >nul
if %ERRORLEVEL% neq 0 (
    echo Failed to copy executable
    exit /b %ERRORLEVEL%
)
echo   %NAME%.exe

if exist "assets" (
    xcopy "assets" "%OUT_DIR%\assets\" /E /I /Q /Y >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy assets
        exit /b %ERRORLEVEL%
    )
    echo   assets\
) else (
    echo   WARNING: assets\ not found
)

mkdir "%OUT_DIR%\plugins" >nul 2>nul
echo   plugins\ runtime plugin directory

if exist "plugins\plugin.schema.json" (
    copy "plugins\plugin.schema.json" "%OUT_DIR%\plugins\plugin.schema.json" >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy plugin schema
        exit /b %ERRORLEVEL%
    )
    echo   plugins\plugin.schema.json
)

if exist "plugins\.lua" (
    xcopy "plugins\.lua" "%OUT_DIR%\plugins\.lua\" /E /I /Q /Y >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy Lua plugin stubs
        exit /b %ERRORLEVEL%
    )
    echo   plugins\.lua\
)

if exist ".luarc.json" (
    copy ".luarc.json" "%OUT_DIR%\.luarc.json" >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy LuaLS config
        exit /b %ERRORLEVEL%
    )
    echo   .luarc.json
)

set "EXAMPLE_PLUGINS_DIR=%OUT_DIR%\examples\plugins"
if exist "plugins\plugin.schema.json" (
    if not exist "%EXAMPLE_PLUGINS_DIR%" mkdir "%EXAMPLE_PLUGINS_DIR%"
    copy "plugins\plugin.schema.json" "%EXAMPLE_PLUGINS_DIR%\plugin.schema.json" >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy example plugin schema
        exit /b %ERRORLEVEL%
    )
    echo   examples\plugins\plugin.schema.json
)

if exist "plugins\.lua" (
    if not exist "%EXAMPLE_PLUGINS_DIR%" mkdir "%EXAMPLE_PLUGINS_DIR%"
    xcopy "plugins\.lua" "%EXAMPLE_PLUGINS_DIR%\.lua\" /E /I /Q /Y >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy example Lua plugin stubs
        exit /b %ERRORLEVEL%
    )
    echo   examples\plugins\.lua\
)

for %%P in (template.hello template.serial-chart template.file-tool) do (
    if exist "plugins\%%P" (
        if not exist "%EXAMPLE_PLUGINS_DIR%" mkdir "%EXAMPLE_PLUGINS_DIR%"
        xcopy "plugins\%%P" "%EXAMPLE_PLUGINS_DIR%\%%P\" /E /I /Q /Y >nul
        if !ERRORLEVEL! neq 0 (
            echo Failed to copy example plugin %%P
            exit /b !ERRORLEVEL!
        )
        echo   examples\plugins\%%P\
    ) else (
        echo   WARNING: plugins\%%P not found
    )
)

if exist "docs" (
    xcopy "docs" "%OUT_DIR%\docs\" /E /I /Q /Y >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy docs
        exit /b %ERRORLEVEL%
    )
    echo   docs\
) else (
    echo   WARNING: docs\ not found
)

if exist "tools" (
    xcopy "tools" "%OUT_DIR%\tools\" /E /I /Q /Y >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy tools
        exit /b %ERRORLEVEL%
    )
    echo   tools\
)


if exist "workspace.json.example" (
    copy "workspace.json.example" "%OUT_DIR%\workspace.json" >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy workspace.json.example
        exit /b %ERRORLEVEL%
    )
    echo   workspace.json from workspace.json.example
) else if exist "workspace.release.json" (
    copy "workspace.release.json" "%OUT_DIR%\workspace.json" >nul
    if %ERRORLEVEL% neq 0 (
        echo Failed to copy workspace.release.json
        exit /b %ERRORLEVEL%
    )
    echo   workspace.json from workspace.release.json
) else (
    echo   workspace.json skipped
)

mkdir "%OUT_DIR%\logs" >nul 2>nul
echo   logs\

echo.

echo [4/5] Creating zip...
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
    "Compress-Archive -Path '%OUT_DIR%' -DestinationPath '%ZIP_PATH%' -Force"

if %ERRORLEVEL% neq 0 (
    echo Failed to create zip
    exit /b %ERRORLEVEL%
)

echo.

echo [5/5] Package summary...
echo ============================================
echo   Done
echo   Portable: %OUT_DIR%\
echo   Zip:      %ZIP_PATH%
echo ============================================

endlocal
