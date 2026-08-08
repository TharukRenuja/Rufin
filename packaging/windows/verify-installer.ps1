[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $InstallerPath,
    [Parameter(Mandatory = $true)]
    [string] $MediaDirectory,
    [string] $TemporaryDirectory = [System.IO.Path]::GetTempPath()
)

$ErrorActionPreference = "Stop"

$installer = Get-Item -LiteralPath $InstallerPath
if (-not (Test-Path -LiteralPath $MediaDirectory -PathType Container)) {
    throw "Media verification directory was not found: $MediaDirectory"
}
New-Item -ItemType Directory -Force -Path $TemporaryDirectory | Out-Null

$kitsRoot = (Get-ItemProperty `
  "HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots").KitsRoot10
$manifestTool = Get-ChildItem `
  -Path (Join-Path $kitsRoot "bin\*\x64\mt.exe") `
  | Sort-Object FullName -Descending `
  | Select-Object -First 1
if (-not $manifestTool) {
  throw "The Windows manifest tool was not found"
}
$manifest = Join-Path $TemporaryDirectory "rufin-installer.manifest"
& $manifestTool.FullName "-inputresource:$($installer.FullName);#1" "-out:$manifest"
if ($LASTEXITCODE -ne 0) {
  throw "Could not read the Windows installer manifest"
}
$manifestText = Get-Content -Raw $manifest
if ($manifestText -notmatch 'requestedExecutionLevel level="asInvoker"') {
  throw "Windows installer is not declared as a per-user process"
}
$installDir = Join-Path $env:LOCALAPPDATA "Programs\Rufin"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $installDir
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Set-Content -NoNewline -Path (Join-Path $installDir "rufin.exe") -Value "legacy"
Set-Content -NoNewline -Path (Join-Path $installDir "legacy-runtime.dll") -Value "legacy"
New-Item -ItemType Directory -Force -Path (Join-Path $installDir "bin") | Out-Null
Set-Content -NoNewline -Path (Join-Path $installDir "bin\obsolete-runtime.dll") -Value "obsolete"
$legacyInstallDir = Join-Path $TemporaryDirectory "Rufin legacy custom"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $legacyInstallDir
New-Item -ItemType Directory -Force -Path $legacyInstallDir | Out-Null
Set-Content -NoNewline -Path (Join-Path $legacyInstallDir "rufin.exe") -Value "legacy"
Set-Content -NoNewline -Path (Join-Path $legacyInstallDir "legacy-runtime.dll") -Value "legacy"
Set-Content -NoNewline -Path (Join-Path $legacyInstallDir "Uninstall.exe") -Value "legacy"
Set-Content -NoNewline -Path (Join-Path $legacyInstallDir "rufin.ico") -Value "legacy"
Set-Content -NoNewline -Path (Join-Path $legacyInstallDir "keep.txt") -Value "keep"
New-Item -ItemType Directory -Force -Path "HKCU:\Software\Rufin" | Out-Null
Set-ItemProperty -Path "HKCU:\Software\Rufin" -Name InstallDir -Value $legacyInstallDir
$desktopShortcut = Join-Path ([Environment]::GetFolderPath("Desktop")) "Rufin.lnk"
$shortcutShell = New-Object -ComObject WScript.Shell
$legacyShortcut = $shortcutShell.CreateShortcut($desktopShortcut)
$legacyShortcut.TargetPath = Join-Path $installDir "rufin.exe"
$legacyShortcut.Save()

$invalidChannel = Start-Process `
  -FilePath $installer.FullName `
  -ArgumentList @("/S", "/RUFINCHANNEL=unknown") `
  -Wait `
  -PassThru
if ($invalidChannel.ExitCode -eq 0) {
  throw "Windows installer accepted an unknown update channel"
}
if (Test-Path (Join-Path $installDir "update-channel")) {
  throw "Rejected Windows install wrote an update channel marker"
}

$legacyHandle = [System.IO.File]::Open(
  (Join-Path $legacyInstallDir "rufin.exe"),
  [System.IO.FileMode]::Open,
  [System.IO.FileAccess]::Read,
  [System.IO.FileShare]::None
)
try {
  $blockedInstall = Start-Process -FilePath $installer.FullName -ArgumentList "/S" -Wait -PassThru
  if ($blockedInstall.ExitCode -eq 0) {
    throw "Windows installer replaced a running legacy Rufin executable"
  }
  if (-not (Test-Path (Join-Path $legacyInstallDir "rufin.exe"))) {
    throw "Blocked Windows upgrade changed the legacy installation"
  }
} finally {
  $legacyHandle.Dispose()
}

$process = Start-Process -FilePath $installer.FullName -ArgumentList "/S" -Wait -PassThru
if ($process.ExitCode -ne 0) {
  throw "Windows installer exited with code $($process.ExitCode)"
}

$installedBin = Join-Path $installDir "bin"
$installedExe = Join-Path $installedBin "rufin.exe"
$deadline = (Get-Date).AddSeconds(60)
while (-not (Test-Path $installedExe) -and (Get-Date) -lt $deadline) {
  Start-Sleep -Milliseconds 250
}
if (-not (Test-Path $installedExe)) {
  throw "Installed Rufin executable was not found: $installedExe"
}
foreach ($legacyRuntime in @("rufin.exe", "legacy-runtime.dll", "bin\obsolete-runtime.dll")) {
  $legacyPath = Join-Path $installDir $legacyRuntime
  if (Test-Path $legacyPath) {
    throw "Legacy Windows runtime was not removed: $legacyPath"
  }
}
foreach ($legacyRuntime in @("rufin.exe", "legacy-runtime.dll", "Uninstall.exe", "rufin.ico")) {
  $legacyPath = Join-Path $legacyInstallDir $legacyRuntime
  if (Test-Path $legacyPath) {
    throw "Custom legacy Windows runtime was not removed: $legacyPath"
  }
}
if (-not (Test-Path (Join-Path $legacyInstallDir "keep.txt"))) {
  throw "Windows migration removed a file it did not own"
}
if (Test-Path "HKCU:\Software\Rufin") {
  throw "Windows migration left the legacy install registry key behind"
}
$desktopTarget = $shortcutShell.CreateShortcut($desktopShortcut).TargetPath
if ($desktopTarget -ne $installedExe) {
  throw "Desktop shortcut still targets the legacy runtime: $desktopTarget"
}
$startMenuShortcut = Join-Path ([Environment]::GetFolderPath("Programs")) "Rufin\Rufin.lnk"
$startMenuTarget = $shortcutShell.CreateShortcut($startMenuShortcut).TargetPath
if ($startMenuTarget -ne $installedExe) {
  throw "Start Menu shortcut does not target the installed executable: $startMenuTarget"
}
$shell = New-Object -ComObject Shell.Application
$shortcutFolder = $shell.Namespace((Split-Path $startMenuShortcut))
$shortcutItem = $shortcutFolder.ParseName((Split-Path $startMenuShortcut -Leaf))
$shortcutAppId = $shortcutItem.ExtendedProperty("System.AppUserModel.ID")
if ($shortcutAppId -ne "io.github.screwys.Rufin") {
  throw "Start Menu shortcut has the wrong application identity: $shortcutAppId"
}
$installedDlls = @(Get-ChildItem -LiteralPath $installedBin -Filter "*.dll" -File)
Write-Host "Installed Rufin runtime has $($installedDlls.Count) DLLs"
if ($installedDlls.Count -eq 0) {
  throw "Installed Rufin runtime has no DLLs"
}
$channelMarker = Join-Path $installDir "update-channel"
if ((Get-Content -Raw $channelMarker).Trim() -ne "direct") {
  throw "Direct Windows install has the wrong update channel"
}
$updaterRoots = @(Get-ChildItem -LiteralPath (Join-Path $installDir "updater") -Directory)
if ($updaterRoots.Count -ne 1) {
  throw "Windows install does not contain one versioned update helper"
}
$updaterRoot = $updaterRoots[0].FullName
$updateHelper = Join-Path $updaterRoot "rufin-update-helper.exe"
$updateSentinel = Join-Path $updaterRoot "rufin-update-helper.complete"
if (-not (Test-Path $updateHelper) -or -not (Test-Path $updateSentinel)) {
  throw "Windows update helper closure is incomplete"
}
$sentinelVersion = $updaterRoots[0].Name
if ((Get-Content -Raw $updateSentinel) -ne "rufin-update-helper:$sentinelVersion`n") {
  throw "Windows update helper has the wrong ownership sentinel"
}
$previousPath = $env:PATH
try {
  $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
  $selfCheck = Start-Process `
    -FilePath $updateHelper `
    -ArgumentList "--self-check" `
    -Wait `
    -PassThru
  if ($selfCheck.ExitCode -ne 0) {
    throw "Packaged Windows update helper could not start from its private closure"
  }
  $rejectedSelfCheck = Start-Process `
    -FilePath $updateHelper `
    -ArgumentList @("--self-check", "unexpected") `
    -Wait `
    -PassThru
  if ($rejectedSelfCheck.ExitCode -eq 0) {
    throw "Windows update helper accepted extra self-check arguments"
  }
} finally {
  $env:PATH = $previousPath
}

$runtimeStdout = Join-Path $TemporaryDirectory "rufin-runtime.out"
$runtimeStderr = Join-Path $TemporaryDirectory "rufin-runtime.err"
$previousPath = $env:PATH
$gstreamerRegistry = Join-Path $TemporaryDirectory "rufin-gstreamer-registry.bin"
Remove-Item -Force -ErrorAction SilentlyContinue $gstreamerRegistry
foreach ($variable in @(
  "GST_PLUGIN_PATH",
  "GST_PLUGIN_PATH_1_0",
  "GST_PLUGIN_SYSTEM_PATH",
  "GST_PLUGIN_SYSTEM_PATH_1_0"
)) {
  Remove-Item -Path "Env:$variable" -ErrorAction SilentlyContinue
}
try {
  $env:PATH = "$installedBin;$env:SystemRoot\System32;$env:SystemRoot"
  $env:GST_REGISTRY_1_0 = $gstreamerRegistry
  foreach ($media in Get-ChildItem -LiteralPath $mediaDirectory -File) {
    $runtimeProcess = Start-Process `
      -FilePath $installedExe `
      -ArgumentList @("--verify-media", $media.FullName) `
      -WorkingDirectory $installDir `
      -RedirectStandardOutput $runtimeStdout `
      -RedirectStandardError $runtimeStderr `
      -PassThru
    if (-not $runtimeProcess.WaitForExit(30000)) {
      Stop-Process -Id $runtimeProcess.Id -Force
      throw "Installed Rufin media check timed out: $($media.Name)"
    }
    if ($runtimeProcess.ExitCode -ne 0) {
      throw "Installed Rufin could not read $($media.Name)"
    }
  }
} catch {
  Get-ChildItem $installDir | Select-Object Name, Length
  throw "Installed Rufin executable failed to start: $_"
} finally {
  $env:PATH = $previousPath
  Remove-Item Env:GST_REGISTRY_1_0 -ErrorAction SilentlyContinue
  foreach ($runtimeLog in @($runtimeStdout, $runtimeStderr)) {
    if ((Test-Path $runtimeLog) -and (Get-Item $runtimeLog).Length -gt 0) {
      Get-Content $runtimeLog
    }
  }
}

$lockedDllHandle = [System.IO.File]::Open(
  $installedDlls[0].FullName,
  [System.IO.FileMode]::Open,
  [System.IO.FileAccess]::Read,
  [System.IO.FileShare]::None
)
try {
  $upgrade = Start-Process -FilePath $installer.FullName -ArgumentList "/S" -Wait -PassThru
  if ($upgrade.ExitCode -ne 5) {
    throw "Windows installer reported the wrong cleanup exit code: $($upgrade.ExitCode)"
  }
} finally {
  $lockedDllHandle.Dispose()
}

$oldUpdaterRoot = Join-Path (Split-Path $updaterRoot) "0.0.0"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $oldUpdaterRoot
Copy-Item -Recurse -Path $updaterRoot -Destination $oldUpdaterRoot
Set-Content `
  -NoNewline `
  -Path (Join-Path $oldUpdaterRoot "rufin-update-helper.complete") `
  -Value "rufin-update-helper:0.0.0`n"
$helperReady = Join-Path $TemporaryDirectory "rufin-update-helper.ready"
$helperError = Join-Path $TemporaryDirectory "rufin-update-helper.err"
$helperUpdateRoot = Join-Path $TemporaryDirectory "rufin-update-helper-cache"
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $helperUpdateRoot
$helperInstallerDir = Join-Path $helperUpdateRoot "downloads\$sentinelVersion"
New-Item -ItemType Directory -Force -Path $helperInstallerDir | Out-Null
$helperInstaller = Join-Path $helperInstallerDir $installer.Name
Copy-Item -LiteralPath $installer.FullName -Destination $helperInstaller
$helperResult = Join-Path $helperUpdateRoot "result.json"
$relaunchPeer = Join-Path $TemporaryDirectory "rufin-relaunch-peer.bat"
$relaunchPeerSource = @"
@echo off
if not "%~1"=="--updated-restart" exit /b 2
if not "%~2"=="$sentinelVersion" exit /b 2
if not "%~3"=="" exit /b 2
echo READY
set /p relaunch_permission=
if not "%relaunch_permission%"=="PRESENT" exit /b 3
echo VISIBLE
"@
Set-Content -NoNewline -Path $relaunchPeer -Value $relaunchPeerSource
Remove-Item -Force -ErrorAction SilentlyContinue $helperReady, $helperError
$waitParent = Start-Process `
  -FilePath "powershell.exe" `
  -ArgumentList @("-NoProfile", "-Command", "Start-Sleep -Seconds 120") `
  -PassThru
$oldUpdateHelper = Join-Path $oldUpdaterRoot "rufin-update-helper.exe"
$helperProcess = Start-Process `
  -FilePath $oldUpdateHelper `
  -ArgumentList @(
    "--parent-pid", $waitParent.Id,
    "--channel", "direct",
    "--target-version", $sentinelVersion,
    "--result-file", $helperResult,
    "--relaunch", $relaunchPeer,
    "--installer", $helperInstaller
  ) `
  -WorkingDirectory $oldUpdaterRoot `
  -RedirectStandardOutput $helperReady `
  -RedirectStandardError $helperError `
  -PassThru
$readyDeadline = (Get-Date).AddSeconds(15)
while ((((Get-Content -Raw $helperReady -ErrorAction SilentlyContinue) ?? "").Trim() -ne "READY") -and
    (Get-Date) -lt $readyDeadline -and -not $helperProcess.HasExited) {
  Start-Sleep -Milliseconds 100
}
if (((Get-Content -Raw $helperReady -ErrorAction SilentlyContinue) ?? "").Trim() -ne "READY") {
  Stop-Process -Id $waitParent.Id -Force -ErrorAction SilentlyContinue
  Stop-Process -Id $helperProcess.Id -Force -ErrorAction SilentlyContinue
  if (Test-Path $helperError) { Get-Content $helperError }
  throw "Windows update helper did not report READY"
}
if ($helperProcess.HasExited) {
  Stop-Process -Id $waitParent.Id -Force -ErrorAction SilentlyContinue
  throw "Windows update helper exited instead of waiting for Rufin"
}
if (-not (Test-Path $helperResult)) {
  Stop-Process -Id $waitParent.Id -Force -ErrorAction SilentlyContinue
  Stop-Process -Id $helperProcess.Id -Force -ErrorAction SilentlyContinue
  throw "Waiting Windows update helper did not leave a pending result"
}
$pendingResult = Get-Content -Raw $helperResult | ConvertFrom-Json
if ($pendingResult.version -ne $sentinelVersion -or
    $pendingResult.message -ne "The update did not finish after Rufin closed.") {
  Stop-Process -Id $waitParent.Id -Force -ErrorAction SilentlyContinue
  Stop-Process -Id $helperProcess.Id -Force -ErrorAction SilentlyContinue
  throw "Windows update helper wrote the wrong pending result"
}
Stop-Process -Id $waitParent.Id -Force
if (-not $helperProcess.WaitForExit(120000)) {
  Stop-Process -Id $helperProcess.Id -Force
  throw "Windows update helper did not finish the direct installer"
}
if ($helperProcess.ExitCode -ne 0) {
  if (Test-Path $helperError) { Get-Content $helperError }
  if (Test-Path $helperResult) { Get-Content $helperResult }
  throw "Windows update helper exited with code $($helperProcess.ExitCode)"
}
if (-not (Test-Path $helperResult)) {
  throw "Successful Windows helper update did not leave an installed result"
}
$installedResult = Get-Content -Raw $helperResult | ConvertFrom-Json
if ($installedResult.status -ne "installed" -or
    $installedResult.version -ne $sentinelVersion) {
  Get-Content $helperResult
  throw "Successful Windows helper update left the wrong result"
}
Remove-Item -Force $helperResult
if ((Test-Path $helperInstaller) -or (Test-Path $helperInstallerDir)) {
  throw "Successful Windows helper update left its cached installer"
}
if ((Get-Content -Raw $channelMarker).Trim() -ne "direct") {
  throw "Direct helper update did not preserve its update channel"
}
if (-not (Test-Path $oldUpdateHelper) -or -not (Test-Path $updateHelper)) {
  throw "Windows helper update did not preserve the old versioned helper"
}

$testUninstaller = Join-Path $TemporaryDirectory "Rufin-Uninstall.exe"
Copy-Item `
  -LiteralPath (Join-Path $installDir "Uninstall.exe") `
  -Destination $testUninstaller
function Invoke-RufinUninstaller([string[]] $Arguments) {
  Start-Process -FilePath $testUninstaller `
    -ArgumentList ($Arguments + "_?=$installDir") -Wait -PassThru
}
$cacheDir = Join-Path $env:LOCALAPPDATA "screwys\Rufin\cache"
New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null
Set-Content -NoNewline -Path (Join-Path $cacheDir "keep.txt") -Value "keep"
$runningHandle = [System.IO.File]::Open(
  $installedExe,
  [System.IO.FileMode]::Open,
  [System.IO.FileAccess]::Read,
  [System.IO.FileShare]::None
)
try {
  $blockedUninstall = Invoke-RufinUninstaller -Arguments @("/S")
  if ($blockedUninstall.ExitCode -ne 2) {
    throw "Windows uninstaller reported the wrong running-app exit code: $($blockedUninstall.ExitCode)"
  }
  if (-not (Test-Path $installedExe)) {
    throw "Blocked Windows uninstall changed the installation"
  }
} finally {
  $runningHandle.Dispose()
}

$lockedDllHandle = [System.IO.File]::Open(
  $installedDlls[0].FullName,
  [System.IO.FileMode]::Open,
  [System.IO.FileAccess]::Read,
  [System.IO.FileShare]::None
)
try {
  $incompleteUninstall = Invoke-RufinUninstaller -Arguments @("/S")
  if ($incompleteUninstall.ExitCode -ne 5) {
    throw "Windows uninstaller reported the wrong cleanup exit code: $($incompleteUninstall.ExitCode)"
  }
  if (-not (Test-Path (Join-Path $installDir "Uninstall.exe")) -or
      -not (Test-Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Rufin")) {
    throw "Incomplete Windows uninstall removed its retry path"
  }
  if (-not (Test-Path (Join-Path $cacheDir "keep.txt"))) {
    throw "Incomplete Windows uninstall removed Rufin's cache"
  }
} finally {
  $lockedDllHandle.Dispose()
}

$uninstall = Invoke-RufinUninstaller -Arguments @("/S", "/PURGEX")
if ($uninstall.ExitCode -ne 0) {
  throw "Windows uninstaller exited with code $($uninstall.ExitCode)"
}
if (Test-Path $installDir) {
  throw "Windows uninstaller left the dedicated install directory behind"
}
if (-not (Test-Path (Join-Path $cacheDir "keep.txt"))) {
  throw "Normal Windows uninstall removed Rufin's cache"
}

$scoopInstall = Start-Process `
  -FilePath $installer.FullName `
  -ArgumentList @("/S", "/RUFINCHANNEL=scoop") `
  -Wait `
  -PassThru
if ($scoopInstall.ExitCode -ne 0) {
  throw "Windows installer could not record the Scoop update channel"
}
if ((Get-Content -Raw $channelMarker).Trim() -ne "scoop") {
  throw "Scoop Windows install has the wrong update channel"
}
$scoopUninstall = Invoke-RufinUninstaller -Arguments @("/S")
if ($scoopUninstall.ExitCode -ne 0) {
  throw "Scoop-channel Windows uninstall exited with code $($scoopUninstall.ExitCode)"
}

$wingetInstall = Start-Process `
  -FilePath $installer.FullName `
  -ArgumentList @("/S", "/RUFINCHANNEL=winget") `
  -Wait `
  -PassThru
if ($wingetInstall.ExitCode -ne 0) {
  throw "Windows installer could not record the WinGet update channel"
}
if ((Get-Content -Raw $channelMarker).Trim() -ne "winget") {
  throw "WinGet Windows install has the wrong update channel"
}

$localRufinDir = Split-Path $cacheDir
$localSentinel = Join-Path $localRufinDir "keep-local.txt"
Set-Content -NoNewline -Path $localSentinel -Value "keep"
$roamingRufinDir = Join-Path $env:APPDATA "screwys\Rufin"
New-Item -ItemType Directory -Force -Path $roamingRufinDir | Out-Null
$roamingSentinel = Join-Path $roamingRufinDir "keep-roaming.txt"
Set-Content -NoNewline -Path $roamingSentinel -Value "keep"
$purge = Invoke-RufinUninstaller -Arguments @("/S", "/PURGE")
if ($purge.ExitCode -ne 0) {
  throw "Windows uninstaller cache purge exited with code $($purge.ExitCode)"
}
if (Test-Path $cacheDir) {
  throw "Windows uninstaller did not purge Rufin's cache"
}
if (-not (Test-Path $localSentinel)) {
  throw "Windows cache purge removed a sibling Local Rufin file"
}
if (-not (Test-Path $roamingSentinel)) {
  throw "Windows cache purge removed Rufin's Roaming data"
}
