<#
  Builds the DAWmate test plugin (crates/mo-plugin) and puts it where DAWs look for CLAP plugins, so Bitwig lists
  "DAWmate (test)" among its instruments.

  It builds the version the launcher (Run latest) last brought down: pick the plugin's pull request in Run latest
  first, then double-click this. It uses the launcher's own copy of the code and its build folder, so it takes no
  extra room beyond the plugin's own build.

  The plugin goes to C:\Program Files\Common Files\CLAP\DAWmate (test).clap, the usual CLAP folder that
  Bitwig scans. Writing there needs an administrator, so Windows asks once (a "Do you want to allow..." box) just
  for the copy; the build itself runs as you. An older copy in %LOCALAPPDATA%\Programs\Common\CLAP (where the first
  version of this script put it) is removed, so Bitwig doesn't list it twice, and so is "Music Organiser (test).clap"
  in the CLAP folder (the plugin's file before the app was called DAWmate: the same plugin, so the same ID). Bitwig must be closed while it's
  replaced (Windows won't replace a file a program has open). In Bitwig: Settings, Locations, Plug-ins, then rescan
  if it doesn't show.

  The installed DAWmate's own plugin (DAWmate.clap in the CLAP folder) is the same plugin, so it comes out while the
  test one is in. -Remove puts it back from the installed app's folder; else the installed app puts it back itself
  at a start once the test one is gone (Windows asks first).

  Options:  -Repo <folder>   build this copy of the code instead of the launcher's
            -Remove          take the plugin out of the CLAP folder again

  Plain ASCII on purpose: Windows PowerShell 5.1 misreads anything else in a file without a BOM.
#>
param(
    [string]$Repo,
    [switch]$Remove
)

$ErrorActionPreference = 'Continue'   # cargo writes progress to stderr, which Windows PowerShell treats as failure
function Say([string]$Text, [string]$Colour = 'Gray') { Write-Host $Text -ForegroundColor $Colour }

$DataRoot = [Environment]::GetFolderPath('LocalApplicationData')
$Launcher = Join-Path (Join-Path $DataRoot 'MusicOrganiser') 'run-latest-rust'
# The 64-bit Common Files folder, even if this runs in 32-bit PowerShell (there CommonProgramFiles is the x86 one).
$CommonFiles = $env:CommonProgramW6432
if (-not $CommonFiles) { $CommonFiles = $env:CommonProgramFiles }
$ClapDir = Join-Path $CommonFiles 'CLAP'
$Plugin = Join-Path $ClapDir 'DAWmate (test).clap'
# The same plugin under the app's first name: taken out with the copy, as Bitwig would list the one ID twice.
$FirstPlugin = Join-Path $ClapDir 'Music Organiser (test).clap'
$OldPlugin = [IO.Path]::Combine($DataRoot, 'Programs', 'Common', 'CLAP', 'Music Organiser (test).clap')
# The copy the installed DAWmate puts in the CLAP folder: the same plugin (one ID), so it goes as this test plugin goes
# in, and the installed app leaves the plugin alone while the test one is there (mo_app::daw_links).
$InstalledPlugin = Join-Path $ClapDir 'DAWmate.clap'
# The installed DAWmate's own plugin, beside the app in its usual folder: -Remove puts it back in from there.
$AppPlugin = [IO.Path]::Combine($DataRoot, 'Programs', 'DAWmate', 'plugins', 'DAWmate.clap')

# Runs a few lines of PowerShell as administrator (Windows asks first) and says whether they worked.
function AsAdmin([string]$Lines) {
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes("`$ErrorActionPreference = 'Stop'`n$Lines"))
    try {
        $p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ErrorAction Stop `
            -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $encoded
        return ($p.ExitCode -eq 0)
    }
    catch { Say 'Windows didn''t allow it (the box was answered No).' 'Red'; return $false }
}
function Quoted([string]$Path) { "'" + $Path.Replace("'", "''") + "'" }

function RemoveOld {
    if (Test-Path -LiteralPath $OldPlugin) {
        try { Remove-Item -LiteralPath $OldPlugin -ErrorAction Stop; Say "Removed the older copy at $OldPlugin" }
        catch { Say "Couldn't remove the older copy at $OldPlugin : is Bitwig open?" 'Yellow' }
    }
}


if ($Remove) {
    RemoveOld
    $hadFirst = Test-Path -LiteralPath $FirstPlugin
    if ($hadFirst) {
        if (-not ((AsAdmin "Remove-Item -LiteralPath $(Quoted $FirstPlugin)") -and -not (Test-Path -LiteralPath $FirstPlugin))) {
            Say "Couldn't remove $FirstPlugin : is Bitwig open? Close it and try again." 'Red'; exit 1
        }
        Say "Removed $FirstPlugin" 'Green'
    }
    # The installed DAWmate's own plugin goes back in the same step, as putting the test one in took it out.
    $putBack = ''
    if ((Test-Path -LiteralPath $AppPlugin) -and -not (Test-Path -LiteralPath $InstalledPlugin)) {
        $putBack = "`nCopy-Item -LiteralPath $(Quoted $AppPlugin) -Destination $(Quoted $InstalledPlugin) -ErrorAction SilentlyContinue"
    }
    if (Test-Path -LiteralPath $Plugin) {
        if ((AsAdmin "Remove-Item -LiteralPath $(Quoted $Plugin)$putBack") -and -not (Test-Path -LiteralPath $Plugin)) {
            Say "Removed $Plugin" 'Green'
        }
        else { Say "Couldn't remove $Plugin : is Bitwig open? Close it and try again." 'Red'; exit 1 }
        if ($putBack) {
            if (Test-Path -LiteralPath $InstalledPlugin) { Say "Put the installed DAWmate's own plugin back: $InstalledPlugin" 'Green' }
            else { Say "Couldn't put the installed DAWmate's own plugin back. It puts it back itself at a start (Windows asks first)." 'Yellow' }
        }
    }
    elseif (-not $hadFirst) { Say 'The test plugin isn''t installed.' }
    exit 0
}

if (-not $Repo) { $Repo = Join-Path $Launcher 'app' }
if (-not (Test-Path -LiteralPath (Join-Path $Repo 'Cargo.toml'))) {
    Say "No copy of the code at $Repo. Run the launcher (Run latest) once first, and pick the plugin's pull request." 'Red'
    exit 1
}
$CargoExe = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $CargoExe) {
    $homeCargo = [IO.Path]::Combine([Environment]::GetFolderPath('UserProfile'), '.cargo', 'bin', 'cargo.exe')
    if (Test-Path -LiteralPath $homeCargo) { $CargoExe = $homeCargo }
}
if (-not $CargoExe) { Say 'Rust (cargo) isn''t installed: see the launcher''s notes.' 'Red'; exit 1 }

# The launcher's build folder: the one it remembers (-BuildDir), else beside the code.
$TargetDir = Join-Path $Repo 'target'
$BuildChoice = Join-Path $Launcher 'build-folder.txt'
if (Test-Path -LiteralPath $BuildChoice) {
    $remembered = "$(Get-Content -LiteralPath $BuildChoice -TotalCount 1 -Encoding UTF8)".Trim()
    if ($remembered -and (Test-Path -LiteralPath $remembered)) { $TargetDir = Join-Path $remembered 'MusicOrganiser-build' }
}

$branch = (& git -C $Repo rev-parse --abbrev-ref HEAD 2>$null)
Say "Building the test plugin from $Repo ($branch). The first build takes several minutes..." 'Cyan'
& $CargoExe build --manifest-path (Join-Path $Repo 'Cargo.toml') --target-dir $TargetDir -p mo-plugin --release --color never 2>&1 |
    ForEach-Object { Say "$_" 'DarkGray' }
if ($LASTEXITCODE -ne 0) { Say 'The build didn''t work (see above).' 'Red'; exit 1 }

$Built = [IO.Path]::Combine($TargetDir, 'release', 'mo_plugin.dll')
if (-not (Test-Path -LiteralPath $Built)) { Say "The build made no $Built." 'Red'; exit 1 }
Say "Copying it to $ClapDir. Windows asks for an administrator for this step..." 'Cyan'
$copy = "[void][IO.Directory]::CreateDirectory($(Quoted $ClapDir))`nCopy-Item -LiteralPath $(Quoted $Built) -Destination $(Quoted $Plugin) -Force"
$copy += "`nif (Test-Path -LiteralPath $(Quoted $FirstPlugin)) { Remove-Item -LiteralPath $(Quoted $FirstPlugin) }"
$copy += "`nif (Test-Path -LiteralPath $(Quoted $InstalledPlugin)) { Remove-Item -LiteralPath $(Quoted $InstalledPlugin) }"
$worked = AsAdmin $copy
# Checked by its contents, so an old copy left in place (Bitwig had it open) is never taken for the new one.
$same = (Test-Path -LiteralPath $Plugin) -and
    ((Get-FileHash -LiteralPath $Plugin).Hash -eq (Get-FileHash -LiteralPath $Built).Hash)
if (-not $worked -or -not $same) {
    Say "Couldn't put the plugin in $ClapDir : is Bitwig open? Close it and double-click this again." 'Red'
    exit 1
}
if (Test-Path -LiteralPath $FirstPlugin) {
    Say "The new plugin is in, but the older copy $FirstPlugin is still there: is Bitwig open? Close it and double-click this again." 'Red'
    exit 1
}
if (Test-Path -LiteralPath $InstalledPlugin) {
    Say "The test plugin is in, but the installed app's copy $InstalledPlugin is still there: is Bitwig open? Close it and double-click this again." 'Red'
    exit 1
}
RemoveOld
Say "Installed: $Plugin" 'Green'
Say 'While it is in, the installed DAWmate''s own plugin is out (they are the same plugin). Run this with -Remove to take the test one out and put that one back.' 'Green'
Say 'Open Bitwig, add the instrument "DAWmate (test)" to a track, and open its window. A song that had "Music Organiser (test)" in it opens it as DAWmate (test).' 'Green'
