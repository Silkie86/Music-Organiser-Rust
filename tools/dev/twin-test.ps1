<#
  The twin test on the whole library (spec section 11.2): every FL Studio project in the app's folders is read by the
  C# app's readers and the Rust ones, and every difference is listed. It only reads: no project, sample or database
  is changed, and nothing leaves this PC.

  It needs both apps' code, as the launchers keep it:
    %LOCALAPPDATA%\MusicOrganiser\run-latest\app        the C# app (Run latest)
    %LOCALAPPDATA%\MusicOrganiser\run-latest-rust\app   the Rust app (this script's own copy)
  and the folders come from the C# app's library (library.db, opened read-only).

  It writes to %LOCALAPPDATA%\MusicOrganiser\logs:
    twin-test-<date>.txt     the summary: counts and field names only, safe to share with Claude
    twin-report-<date>.txt   every difference, file by file (song names and paths: it stays on this PC)

  Folders can be given instead of the library's: .\twin-test.ps1 "D:\Music\FL Projects" "E:\Old saves"
#>
param([string[]]$Folders)

$ErrorActionPreference = 'Stop'
# The C# helper and the twin write UTF-8; read them so, whatever the console's code page.
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$Data = Join-Path $env:LOCALAPPDATA 'MusicOrganiser'
$Rust = Join-Path $Data 'run-latest-rust\app'
$Cs = Join-Path $Data 'run-latest\app'
$Logs = Join-Path $Data 'logs'
$Helper = Join-Path $Data 'twin\cs'
New-Item -ItemType Directory -Force -Path $Logs | Out-Null

if (-not (Test-Path (Join-Path $Cs 'cs'))) { throw "The C# app's code isn't in $Cs. Run the C# launcher (Run latest) once first." }
if (-not (Test-Path (Join-Path $Rust 'Cargo.toml'))) { throw "The Rust app's code isn't in $Rust. Run the Rust launcher once first." }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH" }

Write-Host 'Building the C# half...'
dotnet build (Join-Path $Rust 'tools\twin\cs') -c Release -o $Helper "-p:MoOldRepo=$Cs" -p:MoBuildNative=false -p:MoBuildBitwig=false -v quiet -nologo
if ($LASTEXITCODE -ne 0) { throw 'The C# half did not build.' }
$Dll = Join-Path $Helper 'twin-cs.dll'

if (-not $Folders) {
  $Folders = @(dotnet $Dll --roots (Join-Path $Data 'library.db') | Where-Object { $_ -and (Test-Path $_) })
  if (-not $Folders) { throw "The library has no folders. Give some: .\twin-test.ps1 'D:\Music\FL Projects'" }
}

$Stamp = Get-Date -Format 'yyyy-MM-dd-HHmm'
$Summary = Join-Path $Logs "twin-test-$Stamp.txt"
$Report = Join-Path $Logs "twin-report-$Stamp.txt"
Write-Host "Reading every FL Studio project in $($Folders.Count) folder(s) with both apps..."
Push-Location $Rust
# Cargo writes its progress to stderr, which Windows PowerShell 5.1 takes as an error under 'Stop'.
$ErrorActionPreference = 'Continue'
try {
  # Set-Content rather than Tee-Object, which writes UTF-16 in Windows PowerShell 5.1.
  $Out = cargo run --release -q -p mo-twin -- run --cs $Dll @Folders --report $Report
  $Code = $LASTEXITCODE
} finally { Pop-Location }
$ErrorActionPreference = 'Stop'
if ($Code -ne 0) { throw "The twin test stopped (exit code $Code); its summary wasn't written." }
$Out | Set-Content -Encoding UTF8 -Path $Summary
$Out | Write-Host
Write-Host ''
Write-Host "Summary (safe to share): $Summary"
Write-Host "Every difference (stays here): $Report"
