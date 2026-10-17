# Noto Color Emoji as bitmaps (CBDT), Windows. iced can't draw Segoe UI
# Emoji's color layers either, so vimgram loads this one itself.
$ErrorActionPreference = "Stop"
$Tag = "v2026-09-24-unicode18_0"
$Dir = Join-Path $env:APPDATA "vimgram\data\fonts"
New-Item -ItemType Directory -Force $Dir | Out-Null
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/googlefonts/noto-emoji/$Tag/2D/fonts/NotoColorEmoji.ttf" -OutFile "$Dir\NotoColorEmoji.ttf.part"
Move-Item -Force "$Dir\NotoColorEmoji.ttf.part" "$Dir\NotoColorEmoji.ttf"
Write-Host "installed $Dir\NotoColorEmoji.ttf"
