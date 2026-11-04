# Game SDK の配布候補（CI の artifact `kukuri-game-sdk-<commit>`）を手元の Unity で確かめる（#1984 AC-2・AC-3。2026-11-04 のユーザー
# 判断で Unity は CI につながない）。Rust は使わない（PATH から外す）。結果は <WorkDir>\smoke-result.txt に、段ごとの時間とともに書く。
# 1. scripts/release/game_sdk_dist.py verify で、配布物と commit を照合する。
# 2. sample を配布物と同じ dir に展開し、sample の manifest のまま（隣の .tgz を読む）開いて compile する。
# 3. 空の project に .tgz を入れ、Mono Editor の EditMode の試験と Mono・IL2CPP の Player を動かす（sdk/unity/test.ps1）。
#    開始 → 再開（U1）、Steam Cloud の file からの再開（U2・U5）。-DesktopApp を渡すと、desktop との移行とまとめ（U3・U4）も確かめる。
# 4. 展開した package で、sample の Netcode Sync（test-netcode.ps1）と Skin Loader・Live（test-graphics.ps1）を確かめる。
# 5. sample と展開した package で、Arena の試験を Mono・IL2CPP の Player で動かす（examples/cyber-disc-arena-unity/test.ps1）。
# 6. NGO の Unity Transport のゲームへ掲示板だけを足す部分導入を確かめる（test-partial.ps1）。
#    試験の script は配布しないので、repository の同じ commit のものを使う。package を読む script は、repository と同じ配置へ写す。
#   powershell -ExecutionPolicy Bypass -File sdk/unity/smoke-dist.ps1 -Dist <artifact の dir> -Source <commit>
param(
    [Parameter(Mandatory = $true)][string]$Dist,
    [Parameter(Mandatory = $true)][string]$Source,
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f2\Editor\Unity.exe",
    [string]$WorkDir = (Join-Path $env:TEMP "kgsd"),
    [string]$DesktopApp
)
$ErrorActionPreference = "Stop"
$env:PATH = ($env:PATH -split ';' | Where-Object { $_ -notmatch '\\\.cargo\\|\\\.rustup\\' }) -join ';'
if (Get-Command cargo -ErrorAction SilentlyContinue) { throw "Rust must not be on the PATH: the distribution is used without it" }
$repository = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if ((git -C $repository rev-parse HEAD) -ne $Source) { throw "check out $Source first (the test scripts must be from the same commit)" }
python (Join-Path $repository "scripts\release\game_sdk_dist.py") verify --source $Source --dist $Dist
if ($LASTEXITCODE -ne 0) { throw "the distribution did not verify" }
$info = Get-Content (Join-Path $Dist "build-info.json") -Raw | ConvertFrom-Json
$tarball = Join-Path $Dist "com.kukuri.gamesdk-$($info.version).tgz"

if (Test-Path $WorkDir) { Remove-Item -Recurse -Force $WorkDir }
New-Item -ItemType Directory -Force $WorkDir | Out-Null
$result = Join-Path $WorkDir "smoke-result.txt"
function Record([string]$line) { Write-Output $line; Add-Content -Path $result -Value $line -Encoding utf8 }
Record "source=$Source version=$($info.version) rust=$($info.rust) unity=$(Split-Path (Split-Path (Split-Path $Unity)) -Leaf) desktop=$([bool]$DesktopApp)"

# 2. 利用者と同じ配置: .tgz の隣に sample を展開して、そのまま開く。
Copy-Item $tarball $WorkDir
Expand-Archive (Join-Path $Dist "cyber-disc-arena-unity-$($info.version).zip") $WorkDir
$sample = Join-Path $WorkDir "cyber-disc-arena-unity"
& $Unity -batchmode -nographics -quit -projectPath $sample -logFile (Join-Path $WorkDir "sample-open.log") | Out-Null
$lock = Get-Content (Join-Path $sample "Packages\packages-lock.json") -Raw
if ($LASTEXITCODE -ne 0 -or $lock -notmatch [regex]::Escape("file:../../com.kukuri.gamesdk-$($info.version).tgz")) {
    throw "the sample did not open with the package next to it; see $WorkDir\sample-open.log"
}
Record "sample opened and compiled with the package next to it"

# 3・4 は試験の script が読む配置（repository と同じ）に写す。
$packageDir = Join-Path $WorkDir "repo\sdk\unity\com.kukuri.gamesdk"
$unpacked = Join-Path $WorkDir "unpacked"
New-Item -ItemType Directory -Force $unpacked, (Split-Path $packageDir) | Out-Null
tar -xzf $tarball -C $unpacked
Move-Item (Join-Path $unpacked "package") $packageDir
foreach ($script in "test-netcode.ps1", "test-graphics.ps1") { Copy-Item (Join-Path $PSScriptRoot $script) (Split-Path $packageDir) }
$arena = Join-Path $WorkDir "repo\examples\cyber-disc-arena-unity"
New-Item -ItemType Directory -Force (Split-Path $arena) | Out-Null
Move-Item $sample $arena
Copy-Item (Join-Path $repository "examples\cyber-disc-arena-unity\test.ps1") $arena

$failed = $false
$copied = Join-Path $WorkDir (Split-Path $tarball -Leaf)
$desktop = if ($DesktopApp) { @("-DesktopApp", $DesktopApp) } else { @() }
$runs = @(
    @{ Name = "package-mono"; Script = (Join-Path $PSScriptRoot "test.ps1"); Arguments = @("-Backend", "Mono2x", "-Tarball", $copied) + $desktop },
    @{ Name = "package-il2cpp"; Script = (Join-Path $PSScriptRoot "test.ps1"); Arguments = @("-Backend", "IL2CPP", "-SkipEditMode", "-Tarball", $copied) },
    @{ Name = "netcode"; Script = (Join-Path (Split-Path $packageDir) "test-netcode.ps1"); Arguments = @() },
    @{ Name = "graphics"; Script = (Join-Path (Split-Path $packageDir) "test-graphics.ps1"); Arguments = @() },
    @{ Name = "arena-mono"; Script = (Join-Path $arena "test.ps1"); Arguments = @("-Backend", "Mono2x") },
    @{ Name = "arena-il2cpp"; Script = (Join-Path $arena "test.ps1"); Arguments = @("-Backend", "IL2CPP", "-SkipEditMode") },
    @{ Name = "partial"; Script = (Join-Path $PSScriptRoot "test-partial.ps1"); Arguments = @("-Tarball", $copied) }
)
foreach ($run in $runs) {
    $log = Join-Path $WorkDir "$($run.Name).log"
    $arguments = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $run.Script, "-Unity", $Unity, "-ProjectDir", (Join-Path $WorkDir "p-$($run.Name)")) + $run.Arguments
    # 子の stderr で止まらず、全部の run を回して結果を残す。
    $ErrorActionPreference = "Continue"
    $clock = [Diagnostics.Stopwatch]::StartNew()
    & powershell @arguments 2>&1 | Out-File -Encoding utf8 $log
    $code = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    Record ("{0}: exit={1} seconds={2:n0}" -f $run.Name, $code, $clock.Elapsed.TotalSeconds)
    if ($code -ne 0) { $failed = $true }
}
if ($failed) { throw "the smoke failed; see the logs in $WorkDir" }
Record "all smoke runs passed"
