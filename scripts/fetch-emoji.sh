#!/usr/bin/env bash
# Noto Color Emoji as bitmaps (CBDT). iced can't draw the COLRv1 build most
# distributions ship, so vimgram loads this one itself.
set -euo pipefail
tag=v2026-09-24-unicode18_0
dir="${XDG_DATA_HOME:-$HOME/.local/share}/vimgram/fonts"
mkdir -p "$dir"
curl -fSL -o "$dir/NotoColorEmoji.ttf.part" \
    "https://raw.githubusercontent.com/googlefonts/noto-emoji/$tag/2D/fonts/NotoColorEmoji.ttf"
mv "$dir/NotoColorEmoji.ttf.part" "$dir/NotoColorEmoji.ttf"
echo "installed $dir/NotoColorEmoji.ttf"
