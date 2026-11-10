@echo off
rem Double-click to build the Music Organiser test plugin and put it where Bitwig finds CLAP plugins (see install-test-plugin.ps1).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install-test-plugin.ps1" %*
pause
