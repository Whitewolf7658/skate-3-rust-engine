# Build the WebGPU site: wasm build -> wasm-bindgen -> page -> asset pack.
#   powershell -File tools/build_web.ps1 -Pack <installed pack dir> -Out <site dir> [-DefaultMap StartPark] [-Maps A,B] [-SkipBuild] [-SkipPack]
# Needs: rustup target wasm32-unknown-unknown, wasm-bindgen CLI 0.2.127, Python 3.
# The pack directory holds private converted data: never commit or publish it.
param(
    [Parameter(Mandatory)] [string] $Pack,
    [Parameter(Mandatory)] [string] $Out,
    [string] $DefaultMap,
    [string] $Maps,
    [int] $Jobs = 2,
    [switch] $SkipBuild,
    [switch] $SkipPack
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = Join-Path (Split-Path -Parent $repo) 'target-web' }
$wasm = Join-Path $env:CARGO_TARGET_DIR 'wasm32-unknown-unknown\web\skate3rust.wasm'
New-Item -ItemType Directory -Force $Out | Out-Null

if (-not $SkipBuild) {
    Push-Location $repo
    try {
        cargo build -p skate-game --bin skate3rust --profile web --target wasm32-unknown-unknown --no-default-features -j $Jobs
        if ($LASTEXITCODE) { throw "cargo build failed ($LASTEXITCODE)" }
    } finally { Pop-Location }
}
wasm-bindgen --target web --no-typescript --out-dir $Out --out-name skate3rust $wasm
if ($LASTEXITCODE) { throw "wasm-bindgen failed ($LASTEXITCODE)" }
Copy-Item (Join-Path $repo 'web\*') $Out -Force

if (-not $SkipPack) {
    $packArgs = @((Join-Path $repo 'tools\web_pack.py'), $Pack, $Out)
    if ($DefaultMap) { $packArgs += @('--default', $DefaultMap) }
    if ($Maps) { $packArgs += @('--maps', $Maps) }
    python @packArgs
    if ($LASTEXITCODE) { throw "web_pack.py failed ($LASTEXITCODE)" }
}
$size = (Get-Item (Join-Path $Out 'skate3rust_bg.wasm')).Length / 1MB
"Site ready in $Out (wasm {0:N1} MB). Serve it over HTTP, e.g. python -m http.server -d `"$Out`"" -f $size
