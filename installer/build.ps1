<#
  Builds the Windows setup .exe from a built app: installer/setup.iss, with the name, logo and id from product.toml and
  the version from Cargo.toml, so none of them is typed twice.

    installer\build.ps1 -Exe target\dist\music-organiser.exe [-BitwigExtension <file>] [-ClapPlugin <file>] [-Out dist]

  The .pdb beside the .exe (music_organiser.pdb, what a crash log needs to say where it happened) goes in too.

  Needs Inno Setup 6 (GitHub's Windows machines have it; elsewhere: https://jrsoftware.org/isdl.php). Writes
  <Out>\<Name>-Setup-<version>.exe and prints its path.

  Plain ASCII on purpose: Windows PowerShell 5.1 misreads anything else in a file without a BOM.
#>
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$BitwigExtension,
    # The built CLAP plugin (target\dist\mo_plugin.dll): it goes beside the app as plugins\DAWmate.clap.
    [string]$ClapPlugin,
    [string]$Out = 'dist'
)
$ErrorActionPreference = 'Stop'
$top = Split-Path -Parent $PSScriptRoot

# product.toml: only key = "words" lines (crates/mo-app/product_toml.rs checks the file in every build).
$product = @{}
foreach ($line in Get-Content -LiteralPath (Join-Path $top 'product.toml')) {
    if ($line -match '^\s*([a-z_]+)\s*=\s*"([^"\\]*)"\s*$') { $product[$Matches[1]] = $Matches[2] }
}
foreach ($key in 'name', 'description', 'publisher', 'website', 'logo_ico', 'installer_id') {
    if (-not $product.ContainsKey($key)) { throw "product.toml has no $key" }
}

# The workspace's version, from Cargo.toml's [workspace.package].
$cargo = Get-Content -LiteralPath (Join-Path $top 'Cargo.toml') -Raw
if ($cargo -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { throw 'Cargo.toml has no workspace version' }
$version = $Matches[1]
# Windows' file version is four numbers: 1.2.3-beta.1 is 1.2.3.0.
if ($version -notmatch '^(\d+)\.(\d+)\.(\d+)') { throw "the version $version doesn't start with three numbers" }
$numbers = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"

$exeFile = (Resolve-Path -LiteralPath $Exe).Path
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$outDir = (Resolve-Path -LiteralPath $Out).Path
# A name that's safe in a file name and a web address: DAWmate-Setup-1.0.0.exe.
$safeName = ($product['name'] -replace '[^A-Za-z0-9]+', '-').Trim('-')
$outName = "$safeName-Setup-$version"

$iscc = @(
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
if (-not $iscc) { $iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source }
if (-not $iscc) { throw 'Inno Setup 6 (ISCC.exe) was not found' }

$defines = @(
    "/DAppName=$($product['name'])",
    "/DAppDescription=$($product['description'])",
    "/DAppPublisher=$($product['publisher'])",
    "/DAppWebsite=$($product['website'])",
    "/DAppId=$($product['installer_id'])",
    "/DAppVersion=$version",
    "/DAppVersionNumbers=$numbers",
    "/DLogoIco=$(Join-Path $top $product['logo_ico'])",
    "/DExeFile=$exeFile",
    "/DExeName=music-organiser.exe",
    "/DOutputDir=$outDir",
    "/DOutputName=$outName"
)
$pdb = Join-Path (Split-Path -Parent $exeFile) 'music_organiser.pdb'
if (Test-Path -LiteralPath $pdb) { $defines += "/DPdbFile=$pdb" }
if ($BitwigExtension) {
    $defines += "/DBitwigExtension=$((Resolve-Path -LiteralPath $BitwigExtension).Path)"
}
if ($ClapPlugin) {
    $defines += "/DClapPlugin=$((Resolve-Path -LiteralPath $ClapPlugin).Path)"
}
# Inno Setup's own words go to the screen, so this script's only output is the setup's path.
& $iscc /Q @defines (Join-Path $PSScriptRoot 'setup.iss') | Out-Host
if ($LASTEXITCODE -ne 0) { throw "Inno Setup stopped with $LASTEXITCODE" }
$setup = Join-Path $outDir "$outName.exe"
if (-not (Test-Path -LiteralPath $setup)) { throw "no $setup was made" }
Write-Output $setup
