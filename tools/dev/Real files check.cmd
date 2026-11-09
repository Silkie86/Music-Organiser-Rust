@echo off
rem The real files check: the Rust readers read every FL Studio, Bitwig and Studio One project and MIDI file in the app's folders.
rem It only reads. The summary goes to the logs folder in the app's data folder, where Claude can read it.
where cargo >nul 2>nul || set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0real-files-check.ps1" %*
pause
