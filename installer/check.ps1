<#
  Tries a setup .exe the way people will use it, on a CI machine (never on Solomon's PC: it plays with the user's
  %LOCALAPPDATA%\MusicOrganiser and puts it back as it was):

    1. A new PC, data where it usually goes: the app, its CLAP plugin and its shortcut are in, the Apps list has it,
       no data note. Nothing is put in a CLAP folder (the app does that itself once it runs).
    2. A new PC, data picked elsewhere: the folder starts there (started-here.txt) and the pointer names it; an
       update after it passes the data page by and changes nothing.
    3. A PC with data: it stays where it is, untouched; picking another place only leaves the app the ask to move.
    4. An update over the top keeps the data and its notes; uninstalling takes the app away and leaves the data.

    installer\check.ps1 -Setup dist\DAWmate-Setup-0.1.0.exe

  Plain ASCII on purpose: Windows PowerShell 5.1 misreads anything else in a file without a BOM.
#>
param([Parameter(Mandatory = $true)][string]$Setup)
$ErrorActionPreference = 'Stop'
# Only on GitHub's throwaway machines: it puts this user's data folder aside while it works (rule 11).
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'installer\check.ps1 runs only in CI: it would touch this PC''s data folder' }
$top = Split-Path -Parent $PSScriptRoot
$name = (Get-Content -LiteralPath (Join-Path $top 'product.toml') | Where-Object { $_ -match '^name = "([^"]+)"' } |
    ForEach-Object { $Matches[1] } | Select-Object -First 1)
$setupFile = (Resolve-Path -LiteralPath $Setup).Path
$local = [Environment]::GetFolderPath('LocalApplicationData')
$first = Join-Path $local 'MusicOrganiser'
$pointer = Join-Path $local 'MusicOrganiser-folder.txt'
$asked = Join-Path $local 'MusicOrganiser-move-asked.txt'
$scratch = Join-Path ([IO.Path]::GetTempPath()) "installer-check-$PID"
$app = Join-Path $scratch 'app'
$failed = 0

function Check([bool]$ok, [string]$what) {
    if ($ok) { Write-Output "ok:   $what" } else { Write-Output "FAIL: $what"; $script:failed++ }
}

function Run-Setup([string[]]$more) {
    $log = Join-Path $scratch "setup-$([guid]::NewGuid().ToString('N').Substring(0, 6)).log"
    $p = Start-Process -FilePath $setupFile -Wait -PassThru `
        -ArgumentList (@('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$app`"", "/LOG=`"$log`"") + $more)
    if ($p.ExitCode -ne 0) { Get-Content -LiteralPath $log -ErrorAction SilentlyContinue; throw "setup stopped with $($p.ExitCode)" }
}

function Uninstall {
    $unins = Get-ChildItem -LiteralPath $app -Filter 'unins*.exe' | Select-Object -First 1
    $p = Start-Process -FilePath $unins.FullName -Wait -PassThru -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
    if ($p.ExitCode -ne 0) { throw "uninstall stopped with $($p.ExitCode)" }
    # The uninstaller finishes removing itself a moment after it returns.
    for ($i = 0; $i -lt 30 -and (Test-Path -LiteralPath $app); $i++) { Start-Sleep -Milliseconds 500 }
}

function Clear-Data {
    foreach ($f in $pointer, $asked) { Remove-Item -LiteralPath $f -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $first -Recurse -Force -ErrorAction SilentlyContinue
}

# The machine's own data, put aside and back afterwards.
$aside = Join-Path ([IO.Path]::GetTempPath()) "installer-check-aside-$PID"
New-Item -ItemType Directory -Force -Path $scratch, $aside | Out-Null
foreach ($f in $first, $pointer, $asked) { if (Test-Path -LiteralPath $f) { Move-Item -LiteralPath $f -Destination $aside } }
try {
    # 1. A new PC, the usual place.
    Run-Setup @()
    Check (Test-Path -LiteralPath (Join-Path $app 'music-organiser.exe')) 'the app is in'
    Check (Test-Path -LiteralPath (Join-Path $app 'plugins\DAWmate.clap')) 'the CLAP plugin is beside it'
    $commonClap = Join-Path $(if ($env:CommonProgramW6432) { $env:CommonProgramW6432 } else { $env:CommonProgramFiles }) 'CLAP\DAWmate.clap'
    Check (-not (Test-Path -LiteralPath $commonClap)) 'no CLAP folder written by the setup'
    $menu = Join-Path ([Environment]::GetFolderPath('Programs')) "$name.lnk"
    Check (Test-Path -LiteralPath $menu) "a Start menu shortcut called $name"
    $apps = Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' |
        Where-Object { $_.PSChildName -like '{*}_is1' -and (Get-ItemProperty $_.PSPath).InstallLocation -like "$app*" }
    Check ($null -ne $apps) 'in the Apps list, for this user only'
    Check (-not (Test-Path -LiteralPath $pointer)) 'no pointer for the usual place'
    Check (-not (Test-Path -LiteralPath $first)) 'nothing written to a data folder'
    Uninstall
    Check (-not (Test-Path -LiteralPath $app)) 'uninstall takes the app away'
    Check (-not (Test-Path -LiteralPath $menu)) 'uninstall takes the shortcut away'
    Clear-Data

    # 1b. Only the launcher's folder in the usual place: still a new PC, so a picked folder starts the data there.
    New-Item -ItemType Directory -Force -Path (Join-Path $first 'run-latest-rust') | Out-Null
    $fresh = Join-Path $scratch 'fresh'
    Run-Setup @("/DATADIR=`"$fresh`"")
    Check (Test-Path -LiteralPath (Join-Path $fresh 'MusicOrganiser\started-here.txt')) 'the launcher alone is not data'
    Uninstall
    Clear-Data

    # 2. A new PC, data picked elsewhere (a drive's root means a folder of the app's own name in it).
    $picked = Join-Path $scratch 'fast drive'
    New-Item -ItemType Directory -Force -Path $picked | Out-Null
    Run-Setup @("/DATADIR=`"$picked`"")
    $started = Join-Path $picked 'MusicOrganiser'
    Check (Test-Path -LiteralPath (Join-Path $started 'started-here.txt')) 'the data folder starts where picked'
    Check ((Test-Path -LiteralPath $pointer) -and ((Get-Content -LiteralPath $pointer -Raw).Trim() -eq $started)) 'the pointer names it'
    $head = [IO.File]::ReadAllBytes($pointer)
    Check (-not ($head.Length -ge 3 -and $head[0] -eq 0xEF -and $head[1] -eq 0xBB -and $head[2] -eq 0xBF)) 'the pointer has no byte-order mark'
    # 2b. An update there (silent, no folder given): the data page is passed by and nothing about the data changes
    # (UP-1). Run-Setup throws when the setup stops.
    $pointerBefore = (Get-FileHash -LiteralPath $pointer).Hash
    $startedBefore = (Get-FileHash -LiteralPath (Join-Path $started 'started-here.txt')).Hash
    Run-Setup @()
    Check ((Get-FileHash -LiteralPath $pointer).Hash -eq $pointerBefore) 'an update leaves the pointer as it was'
    Check ((Get-FileHash -LiteralPath (Join-Path $started 'started-here.txt')).Hash -eq $startedBefore) 'an update leaves started-here.txt as it was'
    Check (-not (Test-Path -LiteralPath $asked)) 'an update asks for no move'
    # 2c. A pointer an older setup wrote, with the byte-order mark at the front: still read as the same folder.
    [IO.File]::WriteAllBytes($pointer, [byte[]](0xEF, 0xBB, 0xBF) + [Text.Encoding]::UTF8.GetBytes($started))
    $elsewhere = Join-Path $scratch 'elsewhere'
    Run-Setup @("/DATADIR=`"$elsewhere`"")
    # Read wrongly, the folder would count as missing, and no ask would be left.
    Check ((Test-Path -LiteralPath $asked) -and ((Get-Content -LiteralPath $asked -Raw).Trim() -eq (Join-Path $elsewhere 'MusicOrganiser'))) 'a pointer with the mark still names the data folder'
    Uninstall
    Check (Test-Path -LiteralPath (Join-Path $started 'started-here.txt')) 'uninstall leaves the data folder'
    Clear-Data

    # 3. A PC with data: it stays put, and its files are never touched.
    New-Item -ItemType Directory -Force -Path $first | Out-Null
    $settings = Join-Path $first 'rust-settings.json'
    Set-Content -LiteralPath $settings -Value '{"kept": true}'
    $before = (Get-FileHash -LiteralPath $settings).Hash
    Run-Setup @()
    Check (-not (Test-Path -LiteralPath $asked)) 'no ask to move when the place is kept'
    Check (-not (Test-Path -LiteralPath $pointer)) 'no pointer when the place is kept'
    $other = Join-Path $scratch 'I'
    Run-Setup @("/DATADIR=`"$other`"")
    Check ((Test-Path -LiteralPath $asked) -and ((Get-Content -LiteralPath $asked -Raw).Trim() -eq (Join-Path $other 'MusicOrganiser'))) 'another place: the app is asked to move it'
    Check (-not (Test-Path -LiteralPath $pointer)) 'the installer never points at an empty folder'
    Check (-not (Test-Path -LiteralPath (Join-Path $other 'MusicOrganiser'))) 'the installer copies nothing'

    # 3b. The pointer names a folder that isn't there (a drive unplugged): nothing about the data changes.
    Remove-Item -LiteralPath $asked -Force
    $gone = Join-Path $scratch 'unplugged\MusicOrganiser'
    Set-Content -LiteralPath $pointer -Value $gone
    Run-Setup @("/DATADIR=`"$other`"")
    Check (-not (Test-Path -LiteralPath $asked)) 'no ask while the data can''t be found'
    Check ((Get-Content -LiteralPath $pointer -Raw).Trim() -eq $gone) 'the pointer is left as it was'
    # An update while it can't be found: the data page doesn't stop it.
    Run-Setup @()
    Check ((Get-Content -LiteralPath $pointer -Raw).Trim() -eq $gone) 'an update leaves the pointer as it was too'
    Remove-Item -LiteralPath $pointer -Force

    # 4. An update over the top, then uninstall: the data stays as it was.
    Run-Setup @()
    Uninstall
    Check ((Get-FileHash -LiteralPath $settings).Hash -eq $before) 'the data is untouched throughout'
    Check (-not (Test-Path -LiteralPath $app)) 'uninstall takes the app away'
}
finally {
    Clear-Data
    foreach ($f in Get-ChildItem -LiteralPath $aside) { Move-Item -LiteralPath $f.FullName -Destination $local }
    Remove-Item -LiteralPath $scratch, $aside -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failed -gt 0) { throw "$failed installer checks failed" }
Write-Output 'Every installer check passed'
