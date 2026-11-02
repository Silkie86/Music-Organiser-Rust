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

  Plain ASCII on purpose: Windows PowerShell 5.1 misreads anything else in a file without a BOM.
#>
param(
    [string]$Branch,
    [int]$PullEvery = 20,
    [switch]$NoWatch,
    [string]$Repo
)

# Raised whenever this script changes, so an older branch's copy never replaces a newer installed one.
$LauncherVersion = 4

$ErrorActionPreference = 'Continue'   # git and cargo write progress to stderr, which Windows PowerShell treats as failure
$GitHubRepo = 'Silkie86/Music-Organiser-Rust'
$AppName = 'music-organiser'
$OnWindows = [Environment]::OSVersion.Platform -eq 'Win32NT'
$Exe = if ($OnWindows) { "$AppName.exe" } else { $AppName }
$DataRoot = [Environment]::GetFolderPath('LocalApplicationData')
$Home_ = Join-Path (Join-Path $DataRoot 'MusicOrganiser') 'run-latest-rust'
$Installed = Join-Path $Home_ 'run-latest.ps1'
$LastChoice = Join-Path $Home_ 'last-branch.txt'
# Each build is copied here before it runs, so the next build never has to replace a file the open app is using.
$RunDir = Join-Path $Home_ 'run'
if (-not $Repo) { $Repo = Join-Path $Home_ 'app' }

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

function Git {
    & $GitExe -C $Repo @args
    if ($LASTEXITCODE -ne 0) { throw "git $($args -join ' ') didn't work (see above)." }
}

function Get-LauncherVersion([string]$Path) {
    $line = Select-String -Path $Path -Pattern '^\$LauncherVersion\s*=\s*(\d+)' -List -ErrorAction SilentlyContinue
    if ($line) { [int]$line.Matches[0].Groups[1].Value } else { 0 }
}

# --- First run: set up home -------------------------------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $Home_ | Out-Null
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
# An older copy of this script (say, from an old branch) never replaces a newer installed launcher.
if ($PSCommandPath -ne $Installed -and
    (-not (Test-Path $Installed) -or (Get-LauncherVersion $Installed) -le $LauncherVersion)) {
    Copy-Item -Force $PSCommandPath $Installed
    $cmd = Join-Path $Home_ 'Run latest.cmd'
    Set-Content -Path $cmd -Encoding Ascii -Value "@echo off`r`npowershell -NoProfile -ExecutionPolicy Bypass -File `"%~dp0run-latest.ps1`" %*`r`nif errorlevel 1 pause"
    if ($OnWindows) {
        try {
            $shell = New-Object -ComObject WScript.Shell
            $link = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Desktop')) 'Music Organiser Rust (latest).lnk'))
            $link.TargetPath = $cmd
            $link.WorkingDirectory = $Home_
            $link.Description = 'Runs the newest Rust Music Organiser from GitHub and keeps it up to date'
            $link.Save()
            Say 'Set up. From now on, double-click "Music Organiser Rust (latest)" on your Desktop.' 'Green'
        }
        catch { Say "Set up in $Home_ (couldn't put a shortcut on the Desktop: $($_.Exception.Message))." 'Yellow' }
    }
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
    & $GitExe clone "https://github.com/$GitHubRepo.git" $Repo
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
Say ''
Say "Getting the latest '$Branch' from GitHub..." 'Cyan'
& $GitExe -C $Repo fetch --quiet --prune origin
if ($LASTEXITCODE -ne 0) { Say "Couldn't reach GitHub; running the version already here." 'Yellow' }

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
# Below this much free space (in GB) the build's leftovers are cleared first; below the second it doesn't start.
$LowSpaceGb = 10
$NeededGb = 4

function Get-FreeGb([string]$Path) {
    # The free space on the drive holding $Path (or its nearest folder that exists), in GB; $null when unknown.
    try {
        $probe = $Path
        while ($probe -and -not (Test-Path $probe)) { $probe = Split-Path $probe -Parent }
        if (-not $probe) { return $null }
        $root = [System.IO.Path]::GetPathRoot((Resolve-Path $probe).Path)
        return [System.IO.DriveInfo]::new($root).AvailableFreeSpace / 1GB
    } catch { return $null }
}

function Build-App {
    # Builds while the app (if any) stays open, then copies the new app into run\ under a new name. Returns its path,
    # or $null when the build failed (what cargo said is shown).
    # What cargo says is shown as it goes, so a long first build doesn't look stuck, and a failure shows its cause.
    Say "$(Get-Date -Format 'HH:mm:ss')  Building..." 'DarkGray'
    # The build folder is named, so a CARGO_TARGET_DIR set on the PC can't move the app somewhere this doesn't look.
    $targetDir = Join-Path $Repo 'target'
    # A full drive makes the compiler stop part way ("no space on device"). Before building, the build's own
    # leftovers (the incremental cache and any release build; they only speed builds up) are cleared when space is
    # low, and the build waits when there still isn't enough, saying how much is free.
    $freeGb = Get-FreeGb $targetDir
    if ($null -ne $freeGb -and $freeGb -lt $LowSpaceGb) {
        Say ("Only {0:N1} GB free on the drive the app builds on. Clearing the build's leftovers..." -f $freeGb) 'Yellow'
        foreach ($leftover in @((Join-Path (Join-Path $targetDir 'debug') 'incremental'), (Join-Path $targetDir 'release'))) {
            Remove-Item -Recurse -Force $leftover -ErrorAction SilentlyContinue
        }
        $freeGb = Get-FreeGb $targetDir
        if ($null -ne $freeGb -and $freeGb -lt $NeededGb) {
            Say ("Still only {0:N1} GB free: a build needs about {1} GB. Free some space on that drive (or delete $targetDir, which is rebuilt), and the next push is tried again." -f $freeGb, $NeededGb) 'Red'
            return $null
        }
    }
    # The compiler itself crashing (rustc exits with 101; a plain build error is 1) can come from a damaged
    # incremental cache even when the code is fine, as CI builds without one. The cache only speeds builds up, so
    # it's cleared and the build tried once more.
    $crashed = $false
    $full = $false
    & $CargoExe build --manifest-path (Join-Path $Repo 'Cargo.toml') --target-dir $targetDir --bin $AppName --color never 2>&1 |
        ForEach-Object {
            if ("$_" -match 'rustc.*\(exit code: 101\)') { $crashed = $true }
            if ("$_" -match 'no space (left )?on device|not enough space on the disk') { $full = $true }
            Say "  $_" 'DarkGray'
        }
    if ($LASTEXITCODE -ne 0 -and $full) {
        Say "The drive the app builds on is full. Free some space on it (or delete $targetDir, which is rebuilt), and the next push is tried again." 'Red'
        return $null
    }
    if ($LASTEXITCODE -ne 0 -and $crashed) {
        Say 'The compiler crashed. Clearing its incremental cache and trying once more...' 'Yellow'
        Remove-Item -Recurse -Force (Join-Path (Join-Path $targetDir 'debug') 'incremental') -ErrorAction SilentlyContinue
        & $CargoExe build --manifest-path (Join-Path $Repo 'Cargo.toml') --target-dir $targetDir --bin $AppName --color never 2>&1 |
            ForEach-Object { Say "  $_" 'DarkGray' }
    }
    if ($LASTEXITCODE -ne 0) {
        Say 'The build failed (what cargo said is above). The app keeps the version it has; the next push is tried again.' 'Red'
        return $null
    }
    $built = Join-Path (Join-Path $targetDir 'debug') $Exe
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $copy = Join-Path $RunDir $(if ($OnWindows) { "$AppName-$stamp.exe" } else { "$AppName-$stamp" })
    # A copy that fails (a full disk, antivirus holding the file) counts as a failed build: the open app stays as it is.
    try { Copy-Item -Force $built $copy -ErrorAction Stop }
    catch {
        Say "Couldn't copy the new build ($($_.Exception.Message)). The app keeps the version it has." 'Red'
        return $null
    }
    $copy
}

function Remove-OldBuilds([string]$Keep) {
    # Older copies in run\ that no open app is using. One still in use just stays until next time.
    Get-ChildItem -Path $RunDir -File -ErrorAction SilentlyContinue | Where-Object { $_.FullName -ne $Keep } |
        ForEach-Object { Remove-Item -Force $_.FullName -ErrorAction SilentlyContinue }
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
    $lock = Join-Path (Join-Path (Join-Path $DataRoot 'MusicOrganiser') 'notes') 'writing.lock'
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
if (-not $NoWatch) {
    Say 'Leave this window open: changes pushed to GitHub show up in the app by themselves.' 'Cyan'
}
Say 'In the app, F12 saves a note (and a picture) for Claude.' 'Cyan'
Say 'Press Ctrl+C here, or close this window, to stop. (Close just the app and the next change opens it again.)' 'Cyan'
Say ''

$current = Build-App
if (-not $current) {
    if ($NoWatch -or $PullEvery -le 0) { exit 1 }
    Say 'Waiting for a push that fixes it...' 'Yellow'
}
$app = $null
if ($current) { $app = Start-App $current; Remove-OldBuilds $current }
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
            else { Say "The app stopped with exit code $($app.ExitCode). Its log is in $DataRoot\MusicOrganiser\logs. The next change opens it again." 'Yellow' }
        }
        if ((Get-Date) -lt $next) { continue }
        $next = (Get-Date).AddSeconds($PullEvery)

        & $GitExe -C $Repo fetch --quiet origin $Branch 2>$null
        if ($LASTEXITCODE -ne 0) {
            # Offline for a moment (try again next time), or the branch is gone from GitHub (merged and deleted).
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
    }
}
finally {
    Stop-App $app
    try { $LauncherLock.ReleaseMutex() } catch { }
}
