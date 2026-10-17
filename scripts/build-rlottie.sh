#!/usr/bin/env sh
# Build rlottie into vendor/rlottie for macOS. rlottie-sys's own build
# breaks there: it links libstdc++, and on arm64 rlottie calls 32-bit NEON
# assembly it never compiles (Samsung/rlottie#496). rlottie-sys finds this
# build through pkg-config:
#   export PKG_CONFIG_PATH="$PWD/vendor/rlottie:$PKG_CONFIG_PATH"
set -eu
REV=ea06d2f29ba01b8d06c00a838d107f5e484ae59b
cd "$(dirname "$0")/.."
root=$PWD/vendor/rlottie
rm -rf "$root" && mkdir -p "$root"
git clone -q https://github.com/Samsung/rlottie.git "$root/src"
git -C "$root/src" checkout -q "$REV"
cmake -S "$root/src" -B "$root/build" -Wno-dev \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
  -DBUILD_SHARED_LIBS=OFF -DLOTTIE_MODULE=OFF \
  -DCMAKE_CXX_FLAGS=-U__ARM_NEON__
cmake --build "$root/build" --target rlottie --parallel
cat > "$root/rlottie.pc" <<EOF
Name: rlottie
Description: Lottie animation renderer
Version: 0.2
Libs: -L$root/build -lrlottie -lc++
Cflags: -I$root/src/inc
EOF
echo "rlottie built into vendor/rlottie"
