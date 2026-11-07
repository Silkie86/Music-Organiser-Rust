<#
  Runs the newest Music Organiser (the Rust app) straight from GitHub, and keeps it up to date while it runs. The
  Rust twin of the C# app's launcher: same choices, same rules, its own folder and its own Desktop shortcut, so the
  two never get in each other's way.

  The first run sets itself up in %LOCALAPPDATA%\MusicOrganiser\run-latest-rust: a copy of this script, its own copy
  of the code (separate from anything you edit) and a "Music Organiser Rust (latest)" shortcut on the Desktop. After
  that, double-click the shortcut.

  1. Asks which version to run: each open pull request (most recently changed first), or main. Enter picks the one
     used last time.
  2. Brings that version down from GitHub, builds it and opens the app.
  3. Every 20 seconds, checks GitHub for new pushes to that version. When there are some, it builds them while the
     app stays open, then closes the app and opens the new build. A build that fails leaves the app as it was.
     An open F12 note is never closed: new code waits until it's saved or cancelled.

  Press Ctrl+C in its window, or close the window, to stop. Closing just the app leaves it waiting: the next change
  from GitHub opens it again.

  Options:  -Branch <name>    run that branch without asking
            -PullEvery <s>    seconds between checks for new pushes (0 turns checking off)
            -NoWatch          build and run once, without checking for new pushes
            -Repo <folder>    use this clone instead of the script's own (for testing the script)
            -BuildDir <folder>  build on another drive, like -BuildDir D:\Builds (a drive letter, not a network
                              folder, and not inside code kept with git): the builds go in a "MusicOrganiser-build"
                              folder inside it. Remembered, so it's only needed once (the Desktop shortcut then uses
                              it). -BuildDir default goes back to building beside the code. With -Repo it's for that
                              run only, and the remembered one isn't used.
            -Release          a release build: what Solomon will get in the end, quicker to run but slower to
                              build (several minutes after each push, as cargo keeps no incremental cache for it).
                              The "Music Organiser Rust (release)" shortcut on the Desktop runs it.

  Plain ASCII on purpose: Windows PowerShell 5.1 misreads anything else in a file without a BOM.
#>
param(
    [string]$Branch,
    [int]$PullEvery = 20,
    [switch]$NoWatch,
    [string]$Repo,
    [string]$BuildDir,
    [switch]$Release
)

# Raised whenever this script changes, so an older branch's copy never replaces a newer installed one.
$LauncherVersion = 9

$ErrorActionPreference = 'Continue'   # git and cargo write progress to stderr, which Windows PowerShell treats as failure
$GitHubRepo = 'Silkie86/Music-Organiser-Rust'
$AppName = 'music-organiser'
$OnWindows = [Environment]::OSVersion.Platform -eq 'Win32NT'
$Exe = if ($OnWindows) { "$AppName.exe" } else { $AppName }
$DataRoot = [Environment]::GetFolderPath('LocalApplicationData')
$Home_ = Join-Path (Join-Path $DataRoot 'MusicOrganiser') 'run-latest-rust'
# The app's data folder: the first one, or the one Settings moved it to (named in MusicOrganiser-folder.txt beside the
# first, which the app writes as UTF-8). Looked up each time: the app switches at a start the launcher makes. The
# launcher itself stays in the first one.
function Get-AppData {
    $first = Join-Path $DataRoot 'MusicOrganiser'
    $pointer = Join-Path $DataRoot 'MusicOrganiser-folder.txt'
    if (Test-Path -LiteralPath $pointer) {
        $moved = ((Get-Content -LiteralPath $pointer -Raw -Encoding UTF8 -ErrorAction SilentlyContinue) + '').Trim()
        if ($moved -and [IO.Path]::IsPathRooted($moved) -and (Test-Path -LiteralPath (Join-Path $moved 'moved-in.json'))) { return $moved }
    }
    return $first
}
$Installed = Join-Path $Home_ 'run-latest.ps1'
# Which build cargo makes, and the folder it puts it in inside the build folder.
$BuildKind = if ($Release) { 'release' } else { 'debug' }
# (Built up step by step: an if that gives back an array gives back its one item, or nothing, instead.)
$KindArgs = @()
if ($Release) { $KindArgs += '--release' }
$LastChoice = Join-Path $Home_ 'last-branch.txt'
# Each build is copied here before it runs, so the next build never has to replace a file the open app is using.
$RunDir = Join-Path $Home_ 'run'
# A test run on another clone (-Repo) leaves the remembered build folder alone, so it can't move Solomon's builds.
$RepoGiven = [bool]$Repo
if (-not $Repo) { $Repo = Join-Path $Home_ 'app' }
# Where the builds go when they're not beside the code (-BuildDir): remembered in this file.
$BuildChoice = Join-Path $Home_ 'build-folder.txt'

function Say([string]$Text, [string]$Colour = 'Gray') { Write-Host $Text -ForegroundColor $Colour }

function Stop-WithMessage([string]$Text) {
    Say ''
    Say $Text 'Red'
    exit 1
}

# --- The tools it needs -----------------------------------------------------------------------------------------
# Git: the one on the PATH, or the copy Visual Studio brings with it.
$GitExe = (Get-Command git -ErrorAction SilentlyContinue).Source
if (-not $GitExe -and $OnWindows) {
    $GitExe = Get-ChildItem -Path "$env:ProgramFiles\Microsoft Visual Studio\*\*\Common7\IDE\CommonExtensions\Microsoft\TeamFoundation\Team Explorer\Git\cmd\git.exe" -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty FullName
}
if ($GitExe) {
    # On the PATH for the build too: it reads the commit with git, for the version line in the log and in F12 notes.
    $env:PATH = (Split-Path $GitExe) + [IO.Path]::PathSeparator + $env:PATH
}
else {
    Stop-WithMessage 'Git isn''t installed. Get it from https://git-scm.com/download/win (the standard choices are fine), then run this again.'
}
# Cargo: on the PATH, or where rustup puts it (a window opened before Rust was installed doesn't have it on the PATH).
$CargoExe = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $CargoExe) {
    $home_cargo = Join-Path (Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cargo') (Join-Path 'bin' $(if ($OnWindows) { 'cargo.exe' } else { 'cargo' }))
    if (Test-Path $home_cargo) { $CargoExe = $home_cargo }
}
if (-not $CargoExe) {
    Stop-WithMessage 'Rust isn''t installed. Download rustup-init.exe from https://rustup.rs and run it with the default choices, then run this again.'
}

# --- Where it builds -------------------------------------------------------------------------------------------
# The build folder grows to many GB, so it can go on a drive with more room (-BuildDir). The builds always go in a
# "MusicOrganiser-build" folder of their own inside the one named, and clearing a build folder only ever removes what
# cargo makes there (see Clear-BuildFolder), so nothing else in that folder can be touched.
function Resolve-BuildDir([string]$Path, [switch]$Create) {
    # The folder -BuildDir names, as a whole path, or $null (saying why) when it can't be used. With -Create (a folder
    # just typed) it's made if need be; without it (the remembered one) it must still be there. The build folder inside
    # it is left for cargo to make: cargo marks a folder as its own (with a CACHEDIR.TAG) only when it makes it.
    $p = "$Path".Trim().Trim('"').Trim()
    if ($OnWindows -and $p -match '^[A-Za-z]:$') { $p += '\' }
    if ($OnWindows -and $p -match '^[\\/]{2}') {
        Say "The build folder can't be a network folder like '$p': the launcher can't check how much room it has. Use a drive letter, like D:\Builds." 'Red'
        return $null
    }
    $whole = if ($OnWindows) { $p -match '^[A-Za-z]:[\\/]' } else { $p.StartsWith('/') }
    if (-not $whole) {
        Say "The build folder has to be a whole path, like D:\Builds, not '$p'." 'Red'
        return $null
    }
    try { $p = [IO.Path]::GetFullPath($p) }
    catch {
        Say "'$p' can't be a folder name ($($_.Exception.Message))." 'Red'
        return $null
    }
    $drive = [IO.Path]::GetPathRoot($p)
    if (-not [IO.Directory]::Exists($drive)) {
        Say "The drive $drive for the build folder isn't there (is it plugged in?)." 'Yellow'
        return $null
    }
    # Inside the app's own code, git would see the builds as new files and put them aside on every start.
    $code = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Repo).TrimEnd('\', '/')
    $sep = [IO.Path]::DirectorySeparatorChar
    if (($p.TrimEnd('\', '/') + $sep).StartsWith($code + $sep, [StringComparison]::OrdinalIgnoreCase)) {
        Say "The build folder can't be inside the app's own code ($code)." 'Red'
        return $null
    }
    # Nor inside any other copy of code kept with git, which would see the builds as new files too. Asking git also
    # catches a way into the app's code that the name doesn't show: a link, or a second drive letter for the same place.
    $probe = $p
    while ($probe -and -not [IO.Directory]::Exists($probe)) { $probe = [IO.Path]::GetDirectoryName($probe) }
    if ($probe) {
        $top = & $GitExe -C $probe rev-parse --show-toplevel 2>$null
        if ($LASTEXITCODE -eq 0) {
            Say "The build folder can't be inside a folder of code kept with git ($top): git would see the builds as new files." 'Red'
            return $null
        }
    }
    if ($Create) {
        try { [void][IO.Directory]::CreateDirectory($p) }
        catch {
            Say "Couldn't make the folder $p ($($_.Exception.Message))." 'Red'
            return $null
        }
    }
    elseif (-not [IO.Directory]::Exists($p)) {
        Say "The build folder $p isn't there (is its drive plugged in?)." 'Yellow'
        return $null
    }
    return $p
}

$ChosenBuildDir = $null
if ("$BuildDir".Trim() -eq 'default') {
    if (-not $RepoGiven) { Remove-Item -Force -LiteralPath $BuildChoice -ErrorAction SilentlyContinue }
}
elseif ($BuildDir) {
    $ChosenBuildDir = Resolve-BuildDir $BuildDir -Create
    if (-not $ChosenBuildDir) {
        Stop-WithMessage 'Nothing was changed. Run it again with another folder, like -BuildDir D:\Builds'
    }
    if (-not $RepoGiven) {
        New-Item -ItemType Directory -Force -Path $Home_ | Out-Null
        Set-Content -LiteralPath $BuildChoice -Value $ChosenBuildDir -Encoding UTF8
    }
}
elseif (-not $RepoGiven -and (Test-Path -LiteralPath $BuildChoice)) {
    $rememberedDir = "$(Get-Content -LiteralPath $BuildChoice -TotalCount 1 -Encoding UTF8)".Trim()
    if ($rememberedDir) {
        $ChosenBuildDir = Resolve-BuildDir $rememberedDir
        if (-not $ChosenBuildDir) { Say 'So this time it builds beside the code instead.' 'Yellow' }
    }
}
$TargetDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath((Join-Path $Repo 'target'))
if ($ChosenBuildDir) {
    $TargetDir = [IO.Path]::Combine($ChosenBuildDir, 'MusicOrganiser-build')
    Say "Building in $TargetDir (-BuildDir default builds beside the code again)." 'DarkGray'
}

function Git {
    & $GitExe -C $Repo @args
    if ($LASTEXITCODE -ne 0) { throw "git $($args -join ' ') didn't work (see above)." }
}

function Get-LauncherVersion([string]$Path) {
    $line = Select-String -Path $Path -Pattern '^\$LauncherVersion\s*=\s*(\d+)' -List -ErrorAction SilentlyContinue
    if ($line) { [int]$line.Matches[0].Groups[1].Value } else { 0 }
}

# A Desktop shortcut to the launcher, given its name and what to pass it.
$LauncherCmd = Join-Path $Home_ 'Run latest.cmd'
function Set-Shortcut([string]$Name, [string]$Arguments, [string]$Description) {
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Desktop')) "$Name.lnk"))
    $link.TargetPath = $LauncherCmd
    $link.Arguments = $Arguments
    $link.WorkingDirectory = $Home_
    $link.Description = $Description
    $link.Save()
}

# --- First run: set up home -------------------------------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $Home_ | Out-Null
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
# An older copy of this script (say, from an old branch) never replaces a newer installed launcher.
if ($PSCommandPath -ne $Installed -and
    (-not (Test-Path $Installed) -or (Get-LauncherVersion $Installed) -le $LauncherVersion)) {
    Copy-Item -Force $PSCommandPath $Installed
    Set-Content -Path $LauncherCmd -Encoding Ascii -Value "@echo off`r`npowershell -NoProfile -ExecutionPolicy Bypass -File `"%~dp0run-latest.ps1`" %*`r`nif errorlevel 1 pause"
    if ($OnWindows) {
        try {
            Set-Shortcut 'Music Organiser Rust (latest)' '' 'Runs the newest Rust Music Organiser from GitHub and keeps it up to date'
            Say 'Set up. From now on, double-click "Music Organiser Rust (latest)" on your Desktop.' 'Green'
        }
        catch { Say "Set up in $Home_ (couldn't put a shortcut on the Desktop: $($_.Exception.Message))." 'Yellow' }
    }
}
# The release shortcut came with launcher 8, so an older set-up gets it on its first run of this one.
$ReleaseLink = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Music Organiser Rust (release).lnk'
if ($OnWindows -and (Test-Path -LiteralPath $LauncherCmd) -and -not (Test-Path -LiteralPath $ReleaseLink)) {
    try {
        Set-Shortcut 'Music Organiser Rust (release)' '-Release' 'Runs the newest Rust Music Organiser from GitHub as a release build (quicker to run, slower to build)'
        Say 'Added "Music Organiser Rust (release)" to your Desktop: it runs a release build.' 'Green'
    }
    catch { Say "Couldn't put the release shortcut on the Desktop ($($_.Exception.Message)): 'Run latest.cmd -Release' does the same." 'Yellow' }
}

# --- One launcher per copy of the code ---------------------------------------------------------------------------
# Two launcher windows on the same folder fight over the app and the build. A named mutex (a lock Windows keeps for
# us) is held for as long as this window is open and let go the moment it closes, however it closes.
$repoKey = [IO.Path]::GetFullPath($Repo).TrimEnd('\', '/').ToLowerInvariant()
$sha = [Security.Cryptography.SHA256]::Create()
$hash = -join ($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($repoKey)) | ForEach-Object { $_.ToString('x2') })
$LauncherLock = New-Object System.Threading.Mutex($false, "Local\MusicOrganiserRustLauncher-$($hash.Substring(0, 32))")
try { $gotLock = $LauncherLock.WaitOne(0) }
catch [System.Threading.AbandonedMutexException] { $gotLock = $true }   # the last holder closed without letting go
if (-not $gotLock) {
    Stop-WithMessage ("Another Music Organiser Rust launcher window is already running (it may be behind other windows). " +
        "Close that one first, or keep using it.")
}

if (-not (Test-Path (Join-Path $Repo '.git'))) {
    Say "Downloading the code from GitHub into $Repo (only the first time)..." 'Cyan'
    # Only main comes down (and only main on later plain fetches): see Get-FromGitHub.
    & $GitExe clone --single-branch --branch main "https://github.com/$GitHubRepo.git" $Repo
    if ($LASTEXITCODE -ne 0) { Stop-WithMessage 'Couldn''t download the code from GitHub. Is the internet on?' }
}

# --- Which version --------------------------------------------------------------------------------------------
if (-not $Branch) {
    $remembered = if (Test-Path $LastChoice) { (Get-Content $LastChoice -Raw).Trim() } else { '' }
    $choices = @()
    try {
        # Windows PowerShell 5.1 may not offer TLS 1.2 by itself, which GitHub requires.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        $prs = Invoke-RestMethod -UseBasicParsing -TimeoutSec 15 -Headers @{ 'User-Agent' = 'music-organiser-run-latest' } `
            -Uri "https://api.github.com/repos/$GitHubRepo/pulls?state=open&sort=updated&direction=desc&per_page=15"
        foreach ($pr in $prs) {
            # A pull request from someone's fork has its branch there, not here: this clone can't fetch it.
            if ($pr.head.repo.full_name -ne $pr.base.repo.full_name) { continue }
            $choices += [pscustomobject]@{ Branch = $pr.head.ref; Label = "#$($pr.number)  $($pr.title)"; Updated = [datetime]$pr.updated_at }
        }
    }
    catch {
        Say "Couldn't ask GitHub for the open pull requests ($($_.Exception.Message))." 'Yellow'
    }
    $choices += [pscustomobject]@{ Branch = 'main'; Label = 'main  (only work that has been merged)'; Updated = $null }

    # Enter picks the version used last time, or main when that one isn't open any more (merged, say).
    $default = $choices.Count
    for ($i = 0; $i -lt $choices.Count; $i++) { if ($choices[$i].Branch -eq $remembered) { $default = $i + 1 } }

    Say ''
    Say 'Which version of the app?' 'Cyan'
    for ($i = 0; $i -lt $choices.Count; $i++) {
        $c = $choices[$i]
        $when = ''
        if ($c.Updated) {
            $ago = (Get-Date).ToUniversalTime() - $c.Updated.ToUniversalTime()
            $when = if ($ago.TotalHours -lt 1) { "  (changed $([int]$ago.TotalMinutes) min ago)" }
                    elseif ($ago.TotalDays -lt 1) { "  (changed $([int]$ago.TotalHours) h ago)" }
                    else { "  (changed $([int]$ago.TotalDays) days ago)" }
        }
        $mark = if ($i + 1 -eq $default) { '>' } else { ' ' }
        Say ("{0} {1,2}) {2}{3}" -f $mark, ($i + 1), $c.Label, $when)
    }
    $answer = Read-Host "Type a number, or press Enter for $default"
    $n = 0
    if (-not [int]::TryParse($answer, [ref]$n) -or $n -lt 1 -or $n -gt $choices.Count) { $n = $default }
    $Branch = $choices[$n - 1].Branch
}
Set-Content -Path $LastChoice -Value $Branch

# --- Bring it down ----------------------------------------------------------------------------------------------
# While git changes a ref (or the index) it puts "<name>.lock" beside it, and removes it when done. A git stopped
# part-way (the window closed during a fetch, say) leaves its locks behind, and every later fetch then fails on them
# ("cannot lock ref ... File exists"). Only one launcher runs per copy of the code (the mutex above), so when no git
# is running at all, a lock in this copy's .git folder is stale and is removed. Only files named *.lock, only in .git
# itself and under .git\refs, never through a link.
function Get-LockFiles([string]$Dir, [switch]$Deep) {
    foreach ($item in @(Get-ChildItem -LiteralPath $Dir -Force -ErrorAction SilentlyContinue)) {
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { continue }
        if ($item.PSIsContainer) { if ($Deep) { Get-LockFiles $item.FullName -Deep } }
        elseif ($item.Name -like '*.lock') { $item }
    }
}
function Clear-StaleGitLocks {
    $gitDir = Join-Path $Repo '.git'
    $refs = Join-Path $gitDir 'refs'
    if (-not (Test-Path -LiteralPath $refs -PathType Container)) { return }
    if ((Get-Item -LiteralPath $gitDir -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { return }
    # Any git running (here or in another folder) might own a lock: then nothing is touched this time.
    if (Get-Process -Name 'git', 'git-remote-https', 'git-remote-http' -ErrorAction SilentlyContinue) { return }
    $locks = @(Get-LockFiles $gitDir) + @(Get-LockFiles $refs -Deep)
    if ($locks.Count -eq 0) { return }
    $removed = 0
    foreach ($lock in $locks) {
        try { Remove-Item -LiteralPath $lock.FullName -Force -ErrorAction Stop; $removed++ }
        catch { Say "Couldn't remove the stale git lock $($lock.FullName) ($($_.Exception.Message))." 'Yellow' }
    }
    if ($removed -gt 0) { Say "Removed $removed lock file(s) an earlier git left behind when it was stopped." 'DarkGray' }
}

# Only the version being run comes down from GitHub, into origin/<branch>. Fetching every branch (one per pull
# request) was slower and gave a stale lock many more places to stop the fetch.
# What git said goes in $script:FetchSaid, so the message can say what really went wrong.
function Get-FromGitHub {
    $spec = "+refs/heads/${Branch}:refs/remotes/origin/$Branch"
    $script:FetchSaid = @(& $GitExe -C $Repo fetch --quiet origin $spec 2>&1 | ForEach-Object { "$_" }) -join "`n"
    return ($LASTEXITCODE -eq 0)
}
function Get-FetchProblem {
    if ($FetchSaid -match '\.lock|cannot lock') {
        return 'GitHub was reached, but a lock file left by an earlier git that was stopped part-way blocked the update'
    }
    if ($FetchSaid -match "couldn't find remote ref") { return "'$Branch' isn't on GitHub any more (merged, perhaps)" }
    if ($FetchSaid -match 'Could not resolve host|unable to access|Failed to connect|timed out') {
        return "Couldn't reach GitHub (is the internet on?)"
    }
    return "Git couldn't get the latest from GitHub"
}

# The other branches an older launcher brought down are only clutter here: their origin/... copies are dropped
# (nothing on GitHub changes, and a branch picked later comes down again). A plain "git fetch" in this copy then
# brings only main too.
function Remove-OtherBranchCopies {
    & $GitExe -C $Repo config remote.origin.fetch '+refs/heads/main:refs/remotes/origin/main'
    # Full names: the short name of origin/HEAD is just "origin".
    $keep = @('refs/remotes/origin/main', "refs/remotes/origin/$Branch", 'refs/remotes/origin/HEAD')
    $drop = @(& $GitExe -C $Repo for-each-ref --format='%(refname)' refs/remotes/origin/ |
        Where-Object { $_ -and $keep -notcontains $_ } | ForEach-Object { $_.Substring('refs/remotes/'.Length) })
    # In groups, so the command line stays well inside Windows' limit.
    for ($i = 0; $i -lt $drop.Count; $i += 100) {
        $group = @($drop[$i..([Math]::Min($i + 99, $drop.Count - 1))])
        & $GitExe -C $Repo branch --quiet --delete --remotes @group 2>$null | Out-Null
    }
}

Say ''
Say "Getting the latest '$Branch' from GitHub..." 'Cyan'
Clear-StaleGitLocks
if (-not (Get-FromGitHub)) {
    if ($FetchSaid) { Say $FetchSaid 'DarkGray' }
    Say "$(Get-FetchProblem); running the version already here." 'Yellow'
}
Remove-OtherBranchCopies

function Save-LocalChanges {
    # This copy is only for running the app, so anything changed in it is put aside, not lost.
    if (-not (& $GitExe -C $Repo status --porcelain)) { return }
    & $GitExe -C $Repo stash push --quiet --include-untracked -m "Put aside by Run latest, $(Get-Date -Format 'yyyy-MM-dd HH:mm')"
    if ($LASTEXITCODE -eq 0) { Say "Changes in $Repo were put aside (git stash list shows them)." 'Yellow' }
}
Save-LocalChanges

& $GitExe -C $Repo show-ref --verify --quiet "refs/heads/$Branch"
if ($LASTEXITCODE -eq 0) { Git switch --quiet $Branch }
else {
    & $GitExe -C $Repo show-ref --verify --quiet "refs/remotes/origin/$Branch"
    if ($LASTEXITCODE -ne 0) {
        Stop-WithMessage "'$Branch' isn't in this copy yet and GitHub can't be reached. Try again when the internet is back, or pick main."
    }
    Git switch --quiet --track -c $Branch "origin/$Branch"
}

function Update-ToRemote {
    # Moves the branch to GitHub's version. Normally that's a fast-forward; if the branch was rewritten on GitHub,
    # the old copy is kept as a backup branch first, so nothing is lost.
    & $GitExe -C $Repo merge --ff-only --quiet "origin/$Branch" 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) { return $true }
    if (& $GitExe -C $Repo status --porcelain) {
        Save-LocalChanges
        & $GitExe -C $Repo merge --ff-only --quiet "origin/$Branch" 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { return $true }
    }
    & $GitExe -C $Repo merge-base --is-ancestor HEAD "origin/$Branch" 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) {
        $backup = "backup/$Branch-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
        & $GitExe -C $Repo branch $backup 2>$null | Out-Null
        & $GitExe -C $Repo reset --quiet --keep "origin/$Branch" 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) {
            Say "The branch was rewritten on GitHub; the old copy is kept as $backup." 'Yellow'
            return $true
        }
    }
    Say "Couldn't bring in the latest from GitHub (git said no, see above). The app keeps running the version it has." 'Yellow'
    return $false
}
Update-ToRemote | Out-Null
Say ("Now on: " + (& $GitExe -C $Repo log -1 --format='%h  %s  (%cr)')) 'Green'

# A newer copy of this script in that version is used from the next run (never an older one, from an older branch).
$newer = Join-Path $Repo 'tools/dev/run-latest.ps1'
if ((Test-Path $newer) -and (Test-Path $Installed) -and $PSCommandPath -eq $Installed -and
    (Get-LauncherVersion $newer) -gt $LauncherVersion) {
    Copy-Item -Force $newer $Installed
    Say 'This launcher has been updated; the new one is used next time.' 'DarkGray'
}

# --- Build and run ----------------------------------------------------------------------------------------------
# Below this much free space (in GB) on the drive it builds on, the build folder is cleared first, and if there still
# isn't enough the build waits for the next push.
$NeededGb = 4
# Cargo never deletes the old pieces of earlier builds, so the build folder only ever grows: by gigabytes over a few
# weeks of pushes. Past this size (in GB) it's cleared once the new app is open, and the next build starts from scratch.
$MaxBuildGb = 25
# The incremental cache (what cargo keeps of the app's own code between builds) makes the build after a push about
# five times quicker, so it's kept; but its old copies pile up, so past this size (in GB) just the cache is cleared.
$MaxIncrementalGb = 5

function Test-BuildFolder([string]$Path) {
    # True only for a whole path to a folder cargo builds in: when cargo makes one, it puts a CACHEDIR.TAG in it that
    # says "created by cargo". Everything that clears build files asks this first, so a wrong or empty path, or a
    # folder cargo didn't make, never has anything cleared.
    if (-not $Path -or -not [IO.Path]::IsPathRooted($Path)) { return $false }
    if ($OnWindows -and $Path -notmatch '^([A-Za-z]:[\\/]|[\\/]{2})') { return $false }
    try {
        $tag = [IO.File]::ReadAllText([IO.Path]::Combine($Path, 'CACHEDIR.TAG'))
        return $tag.StartsWith('Signature: 8a477f597d28d172789f06886806bc55') -and $tag.Contains('created by cargo')
    }
    catch { return $false }
}

function Remove-Tree([string]$Path) {
    # Deletes a folder or file for good. A link (a junction or symbolic link) is removed as a link and never gone
    # into, so what it points to is never touched. What Windows won't let go (a file in use) stays; cargo rebuilds it.
    try { $attr = [IO.File]::GetAttributes($Path) } catch { return }
    $isLink = ($attr -band [IO.FileAttributes]::ReparsePoint) -ne 0
    $isDir = ($attr -band [IO.FileAttributes]::Directory) -ne 0
    try {
        if (-not $isDir) {
            if (($attr -band [IO.FileAttributes]::ReadOnly) -ne 0) { [IO.File]::SetAttributes($Path, [IO.FileAttributes]::Normal) }
            [IO.File]::Delete($Path)
        }
        elseif ($isLink) { [IO.Directory]::Delete($Path, $false) }
        else { [IO.Directory]::Delete($Path, $true) }   # .NET's own: links inside are removed, not gone into
        return
    } catch { }
    if (-not $isDir -or $isLink) { return }
    # Something was in the way (a read-only file, say): one by one, then.
    try { $inside = [IO.Directory]::GetFileSystemEntries($Path) } catch { return }
    foreach ($entry in $inside) { Remove-Tree $entry }
    try { [IO.Directory]::Delete($Path, $false) } catch { }
}

function Clear-BuildFolder([string]$Path) {
    # Empties a build folder of what cargo makes in it: its debug and release builds and its scratch files. Only
    # those, so even a build folder that's a link to a folder holding other things loses nothing else; the folder
    # itself (or the link) stays. $false when $Path isn't a build folder, and then nothing is touched.
    if (-not (Test-BuildFolder $Path)) { return $false }
    foreach ($name in 'debug', 'release', 'tmp') { Remove-Tree ([IO.Path]::Combine($Path, $name)) }
    return $true
}

function Get-FolderGb([string]$Path, [string]$Skip = '') {
    # How big a folder is, in GB, leaving out the folder $Skip inside it; 0 when it isn't there. .NET's own listing
    # (PowerShell's takes several times as long on a big folder), and links inside are counted as links, not gone into.
    $bytes = 0L
    if ($Skip) { $Skip = [IO.Path]::GetFullPath($Skip).TrimEnd('\', '/') }
    $todo = [System.Collections.Generic.Stack[string]]::new()
    $todo.Push($Path)
    while ($todo.Count -gt 0) {
        try {
            $dir = [IO.DirectoryInfo]::new($todo.Pop())
            foreach ($file in $dir.EnumerateFiles()) { $bytes += $file.Length }
            foreach ($sub in $dir.EnumerateDirectories()) {
                if (($sub.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0 -and
                    -not [string]::Equals($sub.FullName, $Skip, [StringComparison]::OrdinalIgnoreCase)) { $todo.Push($sub.FullName) }
            }
        } catch { }
    }
    return $bytes / 1GB
}

$SaidNotBuildFolder = $false
function Limit-BuildFolder([string]$Path) {
    # Keeps the build folder in check. Called once the new app is open (it runs from its own copy in run\), so
    # measuring a big folder never holds it up. Only what clearing would remove is measured: cargo's debug, release
    # and scratch folders. Past $MaxBuildGb they're cleared, and the next build takes the long first-build time once;
    # otherwise past $MaxIncrementalGb just the incremental cache is, and the next build takes a little longer.
    if (-not (Test-BuildFolder $Path)) {
        if ([IO.Directory]::Exists($Path) -and -not $script:SaidNotBuildFolder) {
            $script:SaidNotBuildFolder = $true
            Say "$Path has no mark from cargo saying it made it (CACHEDIR.TAG), so the launcher never clears it." 'Yellow'
        }
        return
    }
    $incremental = [IO.Path]::Combine($Path, 'debug', 'incremental')
    $incrementalGb = Get-FolderGb $incremental
    $gb = $incrementalGb + (Get-FolderGb ([IO.Path]::Combine($Path, 'debug')) $incremental) +
        (Get-FolderGb ([IO.Path]::Combine($Path, 'release'))) + (Get-FolderGb ([IO.Path]::Combine($Path, 'tmp')))
    if ($gb -gt $MaxBuildGb) {
        if (Clear-BuildFolder $Path) {
            Say ("The build folder had grown to {0:N1} GB, so it was cleared. The next build starts from scratch (a few minutes longer, once)." -f $gb) 'Yellow'
        }
    }
    elseif ($incrementalGb -gt $MaxIncrementalGb) {
        Remove-Tree $incremental
        if (-not [IO.Directory]::Exists($incremental)) {
            Say ("The incremental cache had grown to {0:N1} GB, so it was cleared. The next build takes a little longer, once." -f $incrementalGb) 'Yellow'
        }
    }
}

function Get-FreeGb([string]$Path) {
    # The free space on the drive holding $Path (or its nearest folder that exists), in GB; $null when unknown.
    try {
        $probe = $Path
        while ($probe -and -not (Test-Path -LiteralPath $probe)) { $probe = [IO.Path]::GetDirectoryName($probe) }
        if (-not $probe) { return $null }
        $root = [System.IO.Path]::GetPathRoot((Resolve-Path -LiteralPath $probe).ProviderPath)
        return [System.IO.DriveInfo]::new($root).AvailableFreeSpace / 1GB
    } catch { return $null }
}

function Get-PdbName([string]$Program) {
    # The name of the debugging-info file (.pdb) a Windows program looks for, from its CodeView record; $null when it
    # has none. rustc links with /PDBALTPATH:%_PDB%, so the program holds just the file's name, not where it was
    # built, and a crash's backtrace finds it only beside the program (or in the folder it was started in).
    try {
        $fs = [IO.File]::OpenRead($Program)
        try {
            $r = New-Object IO.BinaryReader($fs)
            $fs.Position = 0x3C
            $pe = [long]$r.ReadUInt32()
            $fs.Position = $pe
            if ($r.ReadUInt32() -ne 0x4550) { return $null }   # "PE\0\0"
            $fs.Position = $pe + 6
            $sections = [int]$r.ReadUInt16()
            $fs.Position = $pe + 20
            $optional = $pe + 24
            $sectionTable = $optional + [long]$r.ReadUInt16()
            $fs.Position = $optional
            $dataDirs = if ($r.ReadUInt16() -eq 0x20B) { $optional + 112 } else { $optional + 96 }
            $fs.Position = $dataDirs + 6 * 8   # data directory 6: the debug directory
            $debugRva = [long]$r.ReadUInt32()
            $debugSize = [long]$r.ReadUInt32()
            if ($debugRva -eq 0) { return $null }
            $debugAt = $null
            for ($i = 0; $i -lt $sections; $i++) {
                $fs.Position = $sectionTable + 40 * $i + 8
                $virtualSize = [long]$r.ReadUInt32()
                $virtualAt = [long]$r.ReadUInt32()
                $rawSize = [long]$r.ReadUInt32()
                $rawAt = [long]$r.ReadUInt32()
                if ($debugRva -ge $virtualAt -and $debugRva -lt $virtualAt + [Math]::Max($virtualSize, $rawSize)) {
                    $debugAt = $debugRva - $virtualAt + $rawAt
                    break
                }
            }
            if ($null -eq $debugAt) { return $null }
            for ($i = 0; $i -lt [Math]::Floor($debugSize / 28); $i++) {
                $fs.Position = $debugAt + 28 * $i + 12
                $type = $r.ReadUInt32()
                $size = [long]$r.ReadUInt32()
                $null = $r.ReadUInt32()
                $at = [long]$r.ReadUInt32()
                if ($type -ne 2 -or $size -le 24) { continue }   # 2: CodeView
                $fs.Position = $at
                if ($r.ReadUInt32() -ne 0x53445352) { continue }   # "RSDS"
                $fs.Position = $at + 24   # after the GUID and age
                $name = [Text.Encoding]::UTF8.GetString($r.ReadBytes([int]$size - 24)).Split([char]0)[0]
                return ($name -split '[\\/]')[-1]
            }
        }
        finally { $fs.Dispose() }
    }
    catch { }
    return $null
}

function Build-App {
    # Builds while the app (if any) stays open, then copies the new app into a folder of its own in run\. Returns its
    # path, or $null when the build failed (what cargo said is shown).
    # What cargo says is shown as it goes, so a long first build doesn't look stuck, and a failure shows its cause.
    Say "$(Get-Date -Format 'HH:mm:ss')  Building..." 'DarkGray'
    # The build folder is named, so a CARGO_TARGET_DIR set on the PC can't move the app somewhere this doesn't look.
    $targetDir = $TargetDir
    # A full drive makes the compiler stop part way ("no space on device"). Before building, when space is short the
    # build folder is cleared (it only speeds builds up), and the build waits when there still isn't enough, saying
    # how much is free.
    $freeGb = Get-FreeGb $targetDir
    if ($null -ne $freeGb -and $freeGb -lt $NeededGb) {
        if (Clear-BuildFolder $targetDir) {
            Say ("Only {0:N1} GB was free on the drive the app builds on, so the build folder was cleared (the next build starts from scratch)." -f $freeGb) 'Yellow'
            $freeGb = Get-FreeGb $targetDir
        }
        if ($null -ne $freeGb -and $freeGb -lt $NeededGb) {
            Say ("Only {0:N1} GB free: a build needs about {1} GB. Free some space on that drive, and the next push is tried again." -f $freeGb, $NeededGb) 'Red'
            return $null
        }
    }
    # The compiler itself crashing (rustc exits with 101; a plain build error is 1) can come from a damaged
    # incremental cache even when the code is fine, as CI builds without one. The cache only speeds builds up, so
    # it's cleared and the build tried once more.
    $crashed = $false
    $full = $false
    & $CargoExe build --manifest-path (Join-Path $Repo 'Cargo.toml') --target-dir $targetDir --bin $AppName @KindArgs --color never 2>&1 |
        ForEach-Object {
            if ("$_" -match 'rustc.*\(exit code: 101\)') { $crashed = $true }
            if ("$_" -match 'no space (left )?on device|not enough space on the disk') { $full = $true }
            Say "  $_" 'DarkGray'
        }
    if ($LASTEXITCODE -ne 0 -and $full) {
        Say "The drive the app builds on is full. Free some space on it; the next push is tried again (with the build folder cleared first if space is still short)." 'Red'
        return $null
    }
    if ($LASTEXITCODE -ne 0 -and $crashed) {
        Say 'The compiler crashed. Clearing its incremental cache and trying once more...' 'Yellow'
        if (Test-BuildFolder $targetDir) { Remove-Tree ([IO.Path]::Combine($targetDir, $BuildKind, 'incremental')) }
        & $CargoExe build --manifest-path (Join-Path $Repo 'Cargo.toml') --target-dir $targetDir --bin $AppName @KindArgs --color never 2>&1 |
            ForEach-Object { Say "  $_" 'DarkGray' }
    }
    if ($LASTEXITCODE -ne 0) {
        Say 'The build failed (what cargo said is above). The app keeps the version it has; the next push is tried again.' 'Red'
        return $null
    }
    $built = [IO.Path]::Combine($targetDir, $BuildKind, $Exe)
    # Each build gets a folder of its own, so its debugging info can sit beside it under the name it looks for.
    $folder = Join-Path $RunDir (Get-Date -Format 'yyyyMMdd-HHmmss')
    $copy = Join-Path $folder $Exe
    # A copy that fails (a full disk, antivirus holding the file) counts as a failed build: the open app stays as it is.
    try {
        $null = [IO.Directory]::CreateDirectory($folder)
        Copy-Item -Force -LiteralPath $built -Destination $copy -ErrorAction Stop
    }
    catch {
        Say "Couldn't copy the new build ($($_.Exception.Message)). The app keeps the version it has." 'Red'
        return $null
    }
    if ($OnWindows) {
        # Its debugging info (.pdb) goes beside it, so a crash log's backtrace names the functions and lines. cargo
        # keeps it as deps\<the name the app looks for>, and also as <app>.pdb beside the app it built.
        $pdbName = Get-PdbName $copy
        $plain = ($AppName -replace '-', '_') + '.pdb'
        $from = $null
        if ($pdbName -and [IO.File]::Exists([IO.Path]::Combine($targetDir, $BuildKind, 'deps', $pdbName))) {
            $from = [IO.Path]::Combine($targetDir, $BuildKind, 'deps', $pdbName)
        }
        elseif ([IO.File]::Exists([IO.Path]::Combine($targetDir, $BuildKind, $plain))) {
            $from = [IO.Path]::Combine($targetDir, $BuildKind, $plain)
        }
        if ($from) {
            $to = Join-Path $folder $(if ($pdbName) { $pdbName } else { $plain })
            try { Copy-Item -Force -LiteralPath $from -Destination $to -ErrorAction Stop }
            catch { Say "Couldn't copy the debugging info ($($_.Exception.Message)): a crash log may not name the functions." 'Yellow' }
        }
    }
    Add-BitwigExtension $folder
    $copy
}

function Add-BitwigExtension([string]$Folder) {
    # The Bitwig extension (extensions\bitwig-java, Java) is built with its Maven Wrapper when a JDK is installed, and
    # copied beside the app as extensions\MusicOrganiser.bwextension, where Settings > DAWs installs it from. It's only
    # built again when its code changed, and without its tests (CI runs them). Without a JDK, or if the build fails,
    # the app still runs: the DAWs tab says the app has no copy to install.
    $code = [IO.Path]::Combine($Repo, 'extensions', 'bitwig-java')
    if (-not [IO.File]::Exists((Join-Path $code 'pom.xml'))) { return }
    $built = [IO.Path]::Combine($code, 'target', 'MusicOrganiser.bwextension')
    if (-not $env:JAVA_HOME -and -not (Get-Command java -ErrorAction SilentlyContinue)) {
        Say 'No JDK, so the Bitwig extension is not built (winget install Microsoft.OpenJDK.21 to have it).' 'Yellow'
        return
    }
    $sources = @(Get-ChildItem -LiteralPath ([IO.Path]::Combine($code, 'src', 'main')) -Recurse -File -ErrorAction SilentlyContinue) +
               @(Get-Item -LiteralPath (Join-Path $code 'pom.xml'))
    $newest = ($sources | Measure-Object -Property LastWriteTimeUtc -Maximum).Maximum
    if (-not [IO.File]::Exists($built) -or [IO.File]::GetLastWriteTimeUtc($built) -lt $newest) {
        Say 'Building the Bitwig extension...' 'DarkGray'
        $wrapper = if ($OnWindows) { Join-Path $code 'mvnw.cmd' } else { Join-Path $code 'mvnw' }
        Push-Location -LiteralPath $code
        try { & $wrapper -B -q package -DskipTests 2>&1 | ForEach-Object { Say "  $_" 'DarkGray' } }
        finally { Pop-Location }
        if ($LASTEXITCODE -ne 0 -or -not [IO.File]::Exists($built)) {
            Say 'The Bitwig extension did not build (what Maven said is above). The app runs without a copy to install.' 'Yellow'
            return
        }
    }
    try {
        $null = [IO.Directory]::CreateDirectory((Join-Path $Folder 'extensions'))
        Copy-Item -Force -LiteralPath $built -Destination ([IO.Path]::Combine($Folder, 'extensions', 'MusicOrganiser.bwextension')) -ErrorAction Stop
    }
    catch { Say "Couldn't copy the Bitwig extension beside the app ($($_.Exception.Message))." 'Yellow' }
}

function Remove-OldBuilds([string]$Keep) {
    # Older builds in run\ that no open app is using: each in a folder of its own (the app and its debugging info),
    # and the single files older launchers left. One still in use just stays until next time.
    $keepFolder = [IO.Path]::GetDirectoryName($Keep)
    Get-ChildItem -LiteralPath $RunDir -Force -ErrorAction SilentlyContinue | Where-Object { $_.FullName -ne $keepFolder } |
        ForEach-Object {
            $isLink = ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
            if ($_.PSIsContainer -and $_.Name -match '^\d{8}-\d{6}$' -and $isLink) {
                Remove-Tree $_.FullName   # a link named like a build: just the link goes, never what it leads to
            }
            elseif ($_.PSIsContainer -and $_.Name -match '^\d{8}-\d{6}$') {
                # The app first: Windows won't delete it while it's open, and then its folder stays as it is.
                try {
                    $old = Join-Path $_.FullName $Exe
                    if ([IO.File]::Exists($old)) { [IO.File]::Delete($old) }
                    Remove-Tree $_.FullName
                }
                catch { }
            }
            elseif (-not $_.PSIsContainer -and $_.Name -like "$AppName-*") {
                Remove-Item -Force -LiteralPath $_.FullName -ErrorAction SilentlyContinue
            }
        }
}

function Start-App([string]$Path) {
    # The app's log then says the launcher is keeping it up to date.
    if (-not $NoWatch -and $PullEvery -gt 0) { $env:MO_WATCH = '1' }
    # In this window, so what the app prints (a test build's panics, say) shows here, not in a window of its own.
    # Ctrl+C here closes the app at once, without its "Closed" log line; the note draft is already kept as it's typed,
    # and a writing.lock left behind is seen through by its process id.
    $p = Start-Process -FilePath $Path -WorkingDirectory $Repo -NoNewWindow -PassThru
    $null = $p.Handle     # keeps its exit code readable after it ends
    $p
}

function Stop-App($Process) {
    # Asks the app to close, so its session log ends with "Closed" and an open note keeps its draft; forces it only
    # if it doesn't close within ten seconds.
    if (-not $Process -or $Process.HasExited) { return }
    $null = $Process.CloseMainWindow()
    if (-not $Process.WaitForExit(10000)) {
        Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
        $Process.WaitForExit(5000) | Out-Null
    }
}

function Wait-ForNote {
    # The app puts notes\writing.lock up while an F12 note is open (holding its process id). A restart then would
    # close the note half-written, so new code waits until the note is saved or cancelled.
    $lock = Join-Path (Join-Path (Get-AppData) 'notes') 'writing.lock'
    $said = $false
    while (Test-Path $lock) {
        $id = 0
        if (-not [int]::TryParse(((Get-Content $lock -Raw -ErrorAction SilentlyContinue) + '').Trim(), [ref]$id) -or
            -not (Get-Process -Id $id -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -like '*usic*rganiser*' })) {
            break   # left by an app that has gone (its number may since belong to another program)
        }
        if (-not $said) { Say 'An F12 note is open: new code waits until it is saved or cancelled...' 'Yellow'; $said = $true }
        Start-Sleep -Seconds 2
    }
}

Say ''
Say 'Building and starting the app. The first build takes a few minutes; after that it''s quicker.' 'Cyan'
if ($Release) { Say 'A release build: quicker to run, but each build takes several minutes (cargo keeps no incremental cache for it).' 'Cyan' }
if (-not $NoWatch) {
    Say 'Leave this window open: changes pushed to GitHub show up in the app by themselves.' 'Cyan'
}
Say 'In the app, F12 saves a note (and a picture) for Claude.' 'Cyan'
Say 'Press Ctrl+C here, or close this window, to stop. (Close just the app and the next change opens it again.)' 'Cyan'
Say ''

# Once the builds go to another drive (-BuildDir), the launcher's own old builds beside the code (often many GB on C:)
# are cleared, and again after any run that had to build there because that drive wasn't plugged in. Only its debug
# builds: the twin tests build their release builds there too, and keep them.
if ($ChosenBuildDir) {
    $oldTarget = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath((Join-Path $Repo 'target'))
    $oldBuilds = [IO.Path]::Combine($oldTarget, 'debug')
    if ((Test-BuildFolder $oldTarget) -and [IO.Directory]::Exists($oldBuilds)) {
        Say "Clearing the old builds beside the code ($oldBuilds)..." 'DarkGray'
        $gb = Get-FolderGb $oldBuilds
        Remove-Tree $oldBuilds
        Say ("Done: {0:N1} GB freed." -f ($gb - (Get-FolderGb $oldBuilds))) 'Green'
    }
}

$current = Build-App
if (-not $current) {
    if ($NoWatch -or $PullEvery -le 0) { exit 1 }
    Say 'Waiting for a push that fixes it...' 'Yellow'
}
$app = $null
if ($current) {
    $app = Start-App $current
    Remove-OldBuilds $current
    Limit-BuildFolder $TargetDir
}
if ($NoWatch -or $PullEvery -le 0) {
    if ($app) { $app.WaitForExit() }
    exit 0
}

try {
    $saidClosed = $false
    $saidGone = $false
    $couldntTake = ''   # a GitHub version that couldn't be brought in: not tried again until GitHub moves on
    $next = (Get-Date).AddSeconds($PullEvery)
    while ($true) {
        Start-Sleep -Milliseconds 500
        if ($app -and $app.HasExited -and -not $saidClosed) {
            $saidClosed = $true
            if ($app.ExitCode -eq 0) { Say 'The app was closed. The next change from GitHub opens it again.' 'DarkGray' }
            else { Say "The app stopped with exit code $($app.ExitCode). Its log is in $(Get-AppData)\logs. The next change opens it again." 'Yellow' }
        }
        if ((Get-Date) -lt $next) { continue }
        $next = (Get-Date).AddSeconds($PullEvery)

        if (-not (Get-FromGitHub)) {
            # Offline for a moment (try again next time), or the branch is gone from GitHub (merged and deleted), or a
            # git stopped part-way left a lock (cleared here, so the next try gets through).
            Clear-StaleGitLocks
            & $GitExe -C $Repo ls-remote --exit-code --heads origin $Branch 2>$null | Out-Null
            if ($LASTEXITCODE -eq 2 -and -not $saidGone) {
                $saidGone = $true
                Say "'$Branch' isn't on GitHub any more (merged, perhaps). The app stays as it is; start the shortcut again and pick main for the merged work." 'Yellow'
            }
            continue
        }
        $here = (& $GitExe -C $Repo rev-parse HEAD).Trim()
        $there = (& $GitExe -C $Repo rev-parse "origin/$Branch").Trim()
        if ($here -eq $there -or $there -eq $couldntTake) { continue }

        $news = @(& $GitExe -C $Repo log --format='  %h %s' "$here..$there")
        if (-not (Update-ToRemote)) { $couldntTake = $there; continue }
        Say ''
        Say "$(Get-Date -Format 'HH:mm:ss')  New from GitHub ($($news.Count) change(s)):" 'Green'
        $news | ForEach-Object { Say $_ 'Green' }

        $built = Build-App
        if (-not $built) { continue }
        Wait-ForNote
        Stop-App $app
        $app = Start-App $built
        $current = $built
        $saidClosed = $false
        Remove-OldBuilds $current
        Say "$(Get-Date -Format 'HH:mm:ss')  The app is open on the new build." 'Green'
        Limit-BuildFolder $TargetDir
    }
}
finally {
    Stop-App $app
    try { $LauncherLock.ReleaseMutex() } catch { }
}
