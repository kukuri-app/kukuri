# libvpx 1.17.0 の VP8 を Windows x64 の静的 library に build し、Game SDK の feature `vp8` が link する形で `-Out` へ置く
# （`include\vpx\*.h` と `lib\vpx.lib`。ADR 0070 §8.3、#1982）。`-Source` は
# https://chromium.googlesource.com/webm/libvpx の tag v1.17.0 の clone。Git Bash（configure）、nasm、
# make（Git Bash に無ければ Strawberry Perl の gmake）、Visual Studio の C++ build tools が要る。
# build の作業 dir は source の隣（`<source>-kukuri-build`）に作る。
#   powershell -ExecutionPolicy Bypass -File sdk/unity/build-libvpx.ps1 -Source C:\src\libvpx -Out C:\src\libvpx-out
#   $env:KUKURI_LIBVPX_DIR = "C:\src\libvpx-out"; cargo build --release -p kukuri-game-sdk --features vp8
param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Out
)
$ErrorActionPreference = "Stop"
$Source = (Resolve-Path $Source).Path
$tag = git -C $Source describe --tags --exact-match
if ($tag -ne "v1.17.0") { throw "the libvpx source must be the v1.17.0 tag (found '$tag')" }
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw "Visual Studio with the C++ build tools is required" }
$msbuild = Join-Path $vs "MSBuild\Current\Bin\MSBuild.exe"
$lib = Get-ChildItem (Join-Path $vs "VC\Tools\MSVC") -Recurse -Filter lib.exe | Where-Object { $_.FullName -like "*\Hostx64\x64\lib.exe" } | Select-Object -First 1
# build の dir は source の隣に置く（configure に `../<source の名前>` で渡す。他の相対 path では source の一部が
# project から落ちる）。
$build = Join-Path (Split-Path $Source) ((Split-Path $Source -Leaf) + "-kukuri-build")
New-Item -ItemType Directory -Force $build, (Join-Path $Out "include\vpx"), (Join-Path $Out "lib"), (Join-Path $Out "asm") | Out-Null
# configure は VS の project を作る。make が project と生成 header（vpx_config.asm 等）を置く。Windows の make が読めるよう、
# source は build の dir からの相対 path で渡す。
Push-Location $build
try {
    $relative = "../" + (Split-Path $Source -Leaf)
    # Windows の make は shell script の path を bash に読めない形で渡すので、vpx.def は先に bash で作っておく。
    bash -c "'$relative/configure' --target=x86_64-win64-vs17 --disable-vp9 --disable-examples --disable-tools --disable-docs --disable-unit-tests --enable-realtime-only --as=nasm && '$relative/build/make/gen_msvs_def.sh' --name=vpx --out=vpx.def '$relative/vp8/exports_enc' '$relative/vp8/exports_dec' '$relative/vpx/exports_com' '$relative/vpx/exports_enc' '$relative/vpx/exports_dec' && `$(command -v make || command -v gmake)"
} finally {
    Pop-Location
}
# vpx.sln は使わない（同じ理由で作れないことがある）。vpx の project が出来ていれば進む。
if (-not (Test-Path (Join-Path $build "vpx.vcxproj"))) { throw "configure failed" }
# 全体最適化の object は Rust の link と合わない（ADR 0070 §8.3）。
& $msbuild (Join-Path $build "vpx.vcxproj") /m /p:Configuration=Release /p:Platform=x64 /p:WholeProgramOptimization=false
if ($LASTEXITCODE -ne 0) { throw "MSBuild failed" }

# 1.17.0 の VS の project が含めない asm を nasm で組み立てる。
$asm = @(Get-ChildItem (Join-Path $Source "vp8\common\x86\*.asm"), (Join-Path $Source "vp8\encoder\x86\*.asm")) +
    @(Get-Item (Join-Path $Source "vpx_ports\emms_mmx.asm"), (Join-Path $Source "vpx_ports\float_control_word.asm"))
$objects = foreach ($file in $asm) {
    $object = Join-Path $Out ("asm\" + $file.Directory.Name + "_" + $file.BaseName + ".obj")
    nasm -f win64 "-I$build\" "-I$Source\" -o $object $file.FullName
    if ($LASTEXITCODE -ne 0) { throw "nasm failed: $($file.Name)" }
    $object
}
& $lib.FullName /NOLOGO "/OUT:$(Join-Path $Out 'lib\vpx.lib')" (Join-Path $build "x64\Release\vpxmd.lib") @objects
if ($LASTEXITCODE -ne 0) { throw "lib failed" }
Copy-Item (Join-Path $Source "vpx\*.h") (Join-Path $Out "include\vpx") -Force
Write-Output "libvpx is ready in $Out (set KUKURI_LIBVPX_DIR to it)"
