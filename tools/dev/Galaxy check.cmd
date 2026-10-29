@echo off
rem Runs the phase 0 galaxy check: 300,000 stars drawn with wgpu inside Slint, timed for 18 seconds.
rem The numbers are written to the logs folder in %LOCALAPPDATA%\MusicOrganiser, where Claude can read them.
cd /d "%~dp0..\.."
cargo run --release -p galaxycheck
pause
