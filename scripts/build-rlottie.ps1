# Build rlottie into vendor\rlottie (Windows). rlottie-sys's own build
# assumes GCC (it links libstdc++). A DLL, because rlottie's static Windows
# build forces the static CRT, which clashes with Rust's. rlottie-sys finds
# it through pkg-config: add vendor\rlottie to PKG_CONFIG_PATH, and copy
# vendor\rlottie\build\Release\rlottie.dll next to vimgram.exe.
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Rev = "ea06d2f29ba01b8d06c00a838d107f5e484ae59b"
Set-Location (Join-Path $PSScriptRoot "..")
$root = Join-Path (Get-Location) "vendor\rlottie"
if (Test-Path $root) { Remove-Item -Recurse -Force $root }
git clone -q https://github.com/Samsung/rlottie.git "$root\src"
git -C "$root\src" checkout -q $Rev
cmake -S "$root\src" -B "$root\build" -Wno-dev `
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DBUILD_SHARED_LIBS=ON -DLOTTIE_MODULE=OFF
cmake --build "$root\build" --config Release --target rlottie --parallel
$dir = $root -replace '\\', '/'
@"
Name: rlottie
Description: Lottie animation renderer
Version: 0.2
Libs: -L$dir/build/Release -lrlottie
Cflags: -I$dir/src/inc
"@ | Set-Content "$root\rlottie.pc"
Write-Host "rlottie built into vendor\rlottie"
