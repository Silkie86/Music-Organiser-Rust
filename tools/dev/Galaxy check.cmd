@echo off
rem Runs the phase 0 galaxy check: 300,000 stars drawn with wgpu inside Slint, timed for 18 seconds.
rem The numbers are written to the logs folder in %LOCALAPPDATA%\MusicOrganiser, where Claude can read them.
rem Cargo isn't always on the PATH when started from Explorer; rustup puts it here.
where cargo >nul 2>nul || set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
cd /d "%~dp0..\galaxycheck"
cargo run --release
pause
