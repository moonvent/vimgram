#!/usr/bin/env sh
# Fetch the prebuilt TDLib that matches tdlib-rs into vendor/tdlib.
# tdlib-rs's own `download-tdlib` feature is unusable: it needs `zip ^8`,
# and every zip 8.x release has been yanked from crates.io.
set -eu
TDLIB_RS=1.4.0
TDLIB=1.8.61
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-x86_64 ;;
  Darwin-arm64) platform=macos-aarch64 ;;
  Darwin-x86_64) platform=macos-x86_64 ;;
  *) echo "no prebuilt TDLib for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
cd "$(dirname "$0")/.."
rm -rf vendor/tdlib && mkdir -p vendor
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fL -o "$tmp/tdlib.zip" \
  "https://github.com/FedericoBruzzone/tdlib-rs/releases/download/v$TDLIB_RS/tdlib-$TDLIB-$platform.zip"
unzip -q "$tmp/tdlib.zip" -d vendor
echo "TDLib $TDLIB installed into vendor/tdlib"
