# Downloads the tools embedded into the exe (adb from SDK Platform Tools, scrcpy).
$ErrorActionPreference = "Stop"
$embed = Join-Path $PSScriptRoot "..\src-tauri\embed"
New-Item -ItemType Directory -Force $embed | Out-Null
Invoke-WebRequest "https://dl.google.com/android/repository/platform-tools-latest-windows.zip" -OutFile "$embed\platform-tools.zip"
$release = Invoke-RestMethod "https://api.github.com/repos/Genymobile/scrcpy/releases/latest"
$asset = $release.assets | Where-Object { $_.name -match "win64" -and $_.name -like "*.zip" } | Select-Object -First 1
Invoke-WebRequest $asset.browser_download_url -OutFile "$embed\scrcpy.zip"
Write-Host "Done: platform-tools + scrcpy $($release.tag_name)"
