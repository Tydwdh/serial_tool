@echo off
setlocal
cargo build -p hardware-workbench-app --release
set "RESULT=%ERRORLEVEL%"
if not "%RESULT%"=="0" (
  echo Release build failed.
)
exit /b %RESULT%
