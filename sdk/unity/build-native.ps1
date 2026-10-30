# Game SDK の native（kukuri_game_sdk.dll と kukuri-sdk-sidecar.exe）を release で build し、UPM package へ置く。Live の VP8 の
# ために、build-libvpx.ps1 の出力を KUKURI_LIBVPX_DIR に指定しておく（ADR 0070 §8.3）。
# 置いた binary は git に入れない（配布物は #1984 が作る）。DLL を差し替えたら、読み込み済みの Unity Editor は再起動する
# （Windows は読み込んだ DLL を解放しない）。
param(
    [string]$TargetDir = ""
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$package = Join-Path $PSScriptRoot "com.kukuri.gamesdk"
if ($TargetDir -eq "") { $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "target" } }
Push-Location $root
try {
    $env:CARGO_TARGET_DIR = $TargetDir
    if (-not $env:KUKURI_LIBVPX_DIR) { throw "set KUKURI_LIBVPX_DIR to the output of build-libvpx.ps1" }
    cargo build --release -p kukuri-game-sdk --features vp8
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}
$release = Join-Path $TargetDir "release"
$plugins = Join-Path $package "Runtime\Plugins\x86_64"
$sidecar = Join-Path $package "Editor\Sidecar~"
New-Item -ItemType Directory -Force $plugins, $sidecar | Out-Null
Copy-Item (Join-Path $release "kukuri_game_sdk.dll") $plugins -Force
Copy-Item (Join-Path $release "kukuri-sdk-sidecar.exe") $sidecar -Force
Write-Output "placed kukuri_game_sdk.dll and kukuri-sdk-sidecar.exe in $package"
