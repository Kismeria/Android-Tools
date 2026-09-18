# Android Tools — Windows installer.
# Usage: irm https://raw.githubusercontent.com/Kismeria/Android-Tools/main/install.ps1 | iex
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$url = "https://github.com/Kismeria/Android-Tools/releases/latest/download/Android-Tools.exe"
$dir = Join-Path $env:LOCALAPPDATA "Programs\Android Tools"
$exe = Join-Path $dir "Android Tools.exe"

Write-Host "Android Tools: downloading..." -ForegroundColor Green
Get-Process "Android Tools" -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Force $dir | Out-Null
Invoke-WebRequest $url -OutFile $exe -UseBasicParsing
Unblock-File $exe

$shell = New-Object -ComObject WScript.Shell
foreach ($folder in @([Environment]::GetFolderPath("Programs"), [Environment]::GetFolderPath("Desktop"))) {
    $link = $shell.CreateShortcut((Join-Path $folder "Android Tools.lnk"))
    $link.TargetPath = $exe
    $link.WorkingDirectory = $dir
    $link.Save()
}

Write-Host "Installed: $exe" -ForegroundColor Green
Start-Process $exe
