[CmdletBinding()]
param(
    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path,
    [string]$MsysPrefix = "C:\msys64\ucrt64",
    [string]$Configuration = "release",
    [string]$StageDir = (Join-Path $RepoRoot "dist\windows\Rufin")
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Copy-DirectoryContents {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if (-not (Test-Path $Source)) {
        return
    }

    New-Item -ItemType Directory -Force $Destination | Out-Null
    Copy-Item -Path (Join-Path $Source "*") -Destination $Destination -Recurse -Force
}

function Copy-FileIfExists {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if (Test-Path $Source) {
        New-Item -ItemType Directory -Force (Split-Path -Parent $Destination) | Out-Null
        Copy-Item -Path $Source -Destination $Destination -Force
    }
}

function Copy-FileSet {
    param(
        [Parameter(Mandatory = $true)][string]$SourceDir,
        [Parameter(Mandatory = $true)][string]$DestinationDir,
        [Parameter(Mandatory = $true)][string]$Filter
    )

    if (-not (Test-Path $SourceDir)) {
        throw "Source directory was not found: $SourceDir"
    }

    $files = @(Get-ChildItem -LiteralPath $SourceDir -Filter $Filter -File)
    if ($files.Count -eq 0) {
        throw "No files matching $Filter were found in $SourceDir"
    }

    New-Item -ItemType Directory -Force $DestinationDir | Out-Null
    foreach ($file in $files) {
        Copy-Item `
            -LiteralPath $file.FullName `
            -Destination (Join-Path $DestinationDir $file.Name) `
            -Force
    }

    return $files.Count
}

if (-not (Test-Path $MsysPrefix)) {
    throw "MSYS2 UCRT64 prefix was not found: $MsysPrefix"
}

$binary = Join-Path $RepoRoot "target\$Configuration\rufin.exe"
if (-not (Test-Path $binary)) {
    throw "Rufin executable was not found: $binary"
}

if (Test-Path $StageDir) {
    Remove-Item $StageDir -Recurse -Force
}
New-Item -ItemType Directory -Force $StageDir | Out-Null

Copy-Item -Path $binary -Destination (Join-Path $StageDir "rufin.exe") -Force
Copy-FileIfExists (Join-Path $RepoRoot "LICENSE") (Join-Path $StageDir "LICENSE")
Copy-FileIfExists `
    (Join-Path $RepoRoot "packaging\windows\assets\rufin.ico") `
    (Join-Path $StageDir "rufin.ico")

$appShare = Join-Path $StageDir "share"
Copy-FileIfExists `
    (Join-Path $RepoRoot "data\io.github.screwys.Rufin.desktop") `
    (Join-Path $appShare "applications\io.github.screwys.Rufin.desktop")
Copy-FileIfExists `
    (Join-Path $RepoRoot "data\io.github.screwys.Rufin.metainfo.xml") `
    (Join-Path $appShare "metainfo\io.github.screwys.Rufin.metainfo.xml")

$runtimeDllCount = Copy-FileSet `
    -SourceDir (Join-Path $MsysPrefix "bin") `
    -DestinationDir $StageDir `
    -Filter "*.dll"
Write-Host "Copied $runtimeDllCount UCRT64 runtime DLLs"

foreach ($helper in @("gspawn-win64-helper.exe", "gspawn-win64-helper-console.exe")) {
    Copy-FileIfExists `
        (Join-Path $MsysPrefix "bin\$helper") `
        (Join-Path $StageDir $helper)
}

Copy-DirectoryContents (Join-Path $MsysPrefix "lib\gstreamer-1.0") (Join-Path $StageDir "lib\gstreamer-1.0")
Copy-DirectoryContents (Join-Path $MsysPrefix "lib\gdk-pixbuf-2.0") (Join-Path $StageDir "lib\gdk-pixbuf-2.0")
Copy-DirectoryContents (Join-Path $MsysPrefix "lib\gio\modules") (Join-Path $StageDir "lib\gio\modules")
Copy-DirectoryContents (Join-Path $MsysPrefix "libexec") (Join-Path $StageDir "libexec")

Copy-DirectoryContents (Join-Path $MsysPrefix "share\glib-2.0\schemas") (Join-Path $appShare "glib-2.0\schemas")
Copy-DirectoryContents (Join-Path $MsysPrefix "share\gstreamer-1.0") (Join-Path $appShare "gstreamer-1.0")
Copy-DirectoryContents (Join-Path $MsysPrefix "share\icons\Adwaita") (Join-Path $appShare "icons\Adwaita")
Copy-DirectoryContents (Join-Path $MsysPrefix "share\icons\hicolor") (Join-Path $appShare "icons\hicolor")
Copy-DirectoryContents (Join-Path $RepoRoot "data\icons\hicolor") (Join-Path $appShare "icons\hicolor")
Copy-DirectoryContents (Join-Path $MsysPrefix "share\locale") (Join-Path $appShare "locale")
Copy-DirectoryContents (Join-Path $MsysPrefix "share\mime") (Join-Path $appShare "mime")
Copy-DirectoryContents (Join-Path $MsysPrefix "share\themes") (Join-Path $appShare "themes")
Copy-DirectoryContents (Join-Path $MsysPrefix "share\licenses") (Join-Path $appShare "licenses")

$settingsDir = Join-Path $StageDir "etc\gtk-4.0"
New-Item -ItemType Directory -Force $settingsDir | Out-Null
Set-Content `
    -Path (Join-Path $settingsDir "settings.ini") `
    -Encoding ASCII `
    -Value @("[Settings]", "gtk-font-name=Segoe UI 9")

$schemaCompiler = Join-Path $MsysPrefix "bin\glib-compile-schemas.exe"
$schemaDir = Join-Path $appShare "glib-2.0\schemas"
if ((Test-Path $schemaCompiler) -and (Test-Path $schemaDir)) {
    & $schemaCompiler $schemaDir
}

$pixbufQuery = Join-Path $MsysPrefix "bin\gdk-pixbuf-query-loaders.exe"
$pixbufLoaderDir = Join-Path $StageDir "lib\gdk-pixbuf-2.0\2.10.0\loaders"
$pixbufCache = Join-Path $StageDir "lib\gdk-pixbuf-2.0\2.10.0\loaders.cache"
if ((Test-Path $pixbufQuery) -and (Test-Path $pixbufLoaderDir)) {
    $env:GDK_PIXBUF_MODULEDIR = $pixbufLoaderDir
    & $pixbufQuery | Out-File -FilePath $pixbufCache -Encoding ASCII
}

Get-ChildItem -Path $StageDir -Recurse -File |
    Measure-Object -Property Length -Sum |
    ForEach-Object {
        $sizeMiB = [math]::Round($_.Sum / 1MB, 2)
        Write-Host "Staged Rufin Windows runtime at $StageDir ($sizeMiB MiB)"
    }
