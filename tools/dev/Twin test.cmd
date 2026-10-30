@echo off
rem The twin test on the whole library: both apps read every FL Studio and Bitwig project and every difference is listed.
rem It only reads. The summary goes to the logs folder in %LOCALAPPDATA%\MusicOrganiser, where Claude can read it.
where cargo >nul 2>nul || set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0twin-test.ps1" %*
pause
