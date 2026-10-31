@echo off
rem The MIDI twin test: every part the C# app saved in midi.db is worked out again by the Rust analyser, and every difference is listed.
rem It only reads (midi.db and library.db are opened read-only). The summary goes to the logs folder in %LOCALAPPDATA%\MusicOrganiser, where Claude can read it.
where cargo >nul 2>nul || set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
set "DATA=%LOCALAPPDATA%\MusicOrganiser"
set "APP=%DATA%\run-latest-rust\app"
if not exist "%APP%\Cargo.toml" (echo The Rust app code is not in %APP%. Run the Rust launcher once first. & pause & exit /b 1)
if not exist "%DATA%\logs" mkdir "%DATA%\logs"
for /f %%i in ('powershell -NoProfile -Command "Get-Date -Format yyyy-MM-dd-HHmm"') do set "STAMP=%%i"
echo Building the MIDI twin test (a few minutes the first time)...
pushd "%APP%"
start "" /belownormal /wait /b cargo build --release -p mo-miditwin
if errorlevel 1 (popd & echo It did not build. & pause & exit /b 1)
start "" /belownormal /wait /b target\release\midi-twin.exe --midi "%DATA%\midi.db" --library "%DATA%\library.db" --summary "%DATA%\logs\midi-twin-%STAMP%.txt" --report "%DATA%\logs\midi-twin-report-%STAMP%.txt"
popd
echo.
type "%DATA%\logs\midi-twin-%STAMP%.txt"
echo.
echo The summary is in %DATA%\logs\midi-twin-%STAMP%.txt (counts only). The full list stays on this PC in midi-twin-report-%STAMP%.txt.
pause
