@echo off
rem Double-click to run the newest Rust Music Organiser from GitHub (see run-latest.ps1). The first run sets up a Desktop shortcut.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0run-latest.ps1" %*
if errorlevel 1 pause
