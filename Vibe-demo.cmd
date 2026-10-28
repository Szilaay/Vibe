@echo off
setlocal
cd /d "%~dp0"
if not exist "%~dp0VIBE.exe" (
    echo VIBE.exe missing. Build first: powershell -ExecutionPolicy Bypass -File scripts\package.ps1
    pause
    exit /b 1
)
start "" "%~dp0VIBE.exe" --demo %*
