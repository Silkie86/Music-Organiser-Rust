<#
  The real files check: the Rust readers read every FL Studio, Bitwig and Studio One project and MIDI file in the
  app's folders (the library's and the MIDI page's), and it says which files they trip over (a panic: a bug), which
  they can't read, which are slow, and which read differently from the last check though the file hasn't changed.
  Run it before and after a change to a reader to see what the change does to the real library. It only reads: no
  project, sample, MIDI file or database is changed, and nothing leaves this PC. The C# app isn't needed.

  It writes:
    logs\real-files-check-<date>.txt    the summary: counts and parts of a reading only, safe to share with Claude
    logs\real-files-report-<date>.txt   the files behind each count (song names and paths: it stays on this PC)
    twin\real-files-state.txt           what each file read as, for the next check (one file, replaced each time)

  Folders can be given instead of the library's: .\real-files-check.ps1 "D:\Music\FL Projects" "E:\Old saves"
#>
param([string[]]$Folders)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$First = Join-Path $env:LOCALAPPDATA 'MusicOrganiser'
# The launchers stay in the first data folder; the data itself may have moved (Settings, MusicOrganiser-folder.txt).
$Data = $First
$Pointer = Join-Path $env:LOCALAPPDATA 'MusicOrganiser-folder.txt'
if (Test-Path -LiteralPath $Pointer) {
    $moved = ((Get-Content -LiteralPath $Pointer -Raw -Encoding UTF8 -ErrorAction SilentlyContinue) + '').Trim()
    if ($moved -and [IO.Path]::IsPathRooted($moved) -and (Test-Path -LiteralPath (Join-Path $moved 'moved-in.json'))) { $Data = $moved }
}
$Rust = Join-Path $First 'run-latest-rust\app'
$Logs = Join-Path $Data 'logs'
$Twin = Join-Path $Data 'twin'
New-Item -ItemType Directory -Force -Path $Logs | Out-Null
New-Item -ItemType Directory -Force -Path $Twin | Out-Null
if (-not (Test-Path (Join-Path $Rust 'Cargo.toml'))) { throw "The Rust app's code isn't in $Rust. Run the Rust launcher once first." }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH" }

$Stamp = Get-Date -Format 'yyyy-MM-dd-HHmm'
$Summary = Join-Path $Logs "real-files-check-$Stamp.txt"
$Report = Join-Path $Logs "real-files-report-$Stamp.txt"
$State = Join-Path $Twin 'real-files-state.txt'
$From = if ($Folders) { $Folders } else { @('--library', (Join-Path $Data 'library.db')) }

# Below normal priority (rule 21), so a DAW open meanwhile stays quick.
try { (Get-Process -Id $PID).PriorityClass = 'BelowNormal' } catch { }
Write-Host 'Building the check (a few minutes the first time)...'
Push-Location $Rust
# Cargo writes its progress to stderr, which Windows PowerShell 5.1 takes as an error under 'Stop'.
$ErrorActionPreference = 'Continue'
try {
  cargo build --release -p mo-twin
  if ($LASTEXITCODE -ne 0) { throw 'The check did not build.' }
  Write-Host 'Reading every FL Studio, Bitwig and Studio One project and MIDI file...'
  # Set-Content rather than Tee-Object, which writes UTF-16 in Windows PowerShell 5.1.
  $Out = @(cargo run --release -q -p mo-twin -- check @From --state $State --report $Report)
  $Code = $LASTEXITCODE
} finally { Pop-Location }
$ErrorActionPreference = 'Stop'
# Exit code 1: a reader panicked on a file. The summary still counts it.
if ($Code -gt 1) { throw "The check stopped (exit code $Code); its summary wasn't written." }
$Out | Set-Content -Encoding UTF8 -Path $Summary
$Out | Write-Host
Write-Host ''
Write-Host "Summary (safe to share): $Summary"
Write-Host "The files behind each count (stays here): $Report"
