# Fetch the prebuilt TDLib that matches tdlib-rs into vendor\tdlib (Windows).
# tdlib-rs's own `download-tdlib` feature is unusable: it needs `zip ^8`,
# and every zip 8.x release has been yanked from crates.io.
$ErrorActionPreference = "Stop"
$TdlibRs = "1.4.0"
$Tdlib = "1.8.61"
Set-Location (Join-Path $PSScriptRoot "..")
if (Test-Path vendor\tdlib) { Remove-Item -Recurse -Force vendor\tdlib }
New-Item -ItemType Directory -Force vendor | Out-Null
$zip = Join-Path $env:TEMP "tdlib-$Tdlib.zip"
Invoke-WebRequest -Uri "https://github.com/FedericoBruzzone/tdlib-rs/releases/download/v$TdlibRs/tdlib-$Tdlib-windows-x86_64.zip" -OutFile $zip
Expand-Archive -Path $zip -DestinationPath vendor
Remove-Item $zip
Write-Host "TDLib $Tdlib installed into vendor\tdlib; copy vendor\tdlib\bin\*.dll next to vimgram.exe"
