@echo off
setlocal
cargo build -p hardware-workbench-app
set "RESULT=%ERRORLEVEL%"
if not "%RESULT%"=="0" (
  echo Build failed.
)
exit /b %RESULT%
