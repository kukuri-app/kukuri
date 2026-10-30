# glTFast 6.20.0 を入れた新しい Unity project に Game SDK の package と Skin Loader sample を入れ、スキンの動的な差し替えの
# EditMode の試験を流す（#1980 AC-2）。発行者は開発用の Sidecar、Player は FFI の 2 つの profile で、keyring を使わず
# loopback だけで動かす。先に build-native.ps1 で native を置く。glTFast を入れない build は test.ps1 が確かめる。
param(
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Unity.exe",
    [string]$ProjectDir = (Join-Path $env:TEMP "kgs-assets")
)
$ErrorActionPreference = "Stop"
$package = (Resolve-Path (Join-Path $PSScriptRoot "com.kukuri.gamesdk")).Path
if (-not (Test-Path (Join-Path $package "Runtime\Plugins\x86_64\kukuri_game_sdk.dll"))) { throw "run build-native.ps1 first" }

if (Test-Path $ProjectDir) { Remove-Item -Recurse -Force $ProjectDir }
& $Unity -batchmode -nographics -quit -createProject $ProjectDir -logFile (Join-Path $env:TEMP "kgs-assets-create.log") | Out-Null
$manifest = Get-Content (Join-Path $ProjectDir "Packages\manifest.json") -Raw | ConvertFrom-Json
$manifest.dependencies | Add-Member -NotePropertyName "com.kukuri.gamesdk" -NotePropertyValue ("file:" + ($package -replace '\\', '/')) -Force
$manifest.dependencies | Add-Member -NotePropertyName "com.unity.cloud.gltfast" -NotePropertyValue "6.20.0" -Force
$manifest.dependencies | Add-Member -NotePropertyName "com.unity.test-framework" -NotePropertyValue "1.6.0" -Force
$manifest | Add-Member -NotePropertyName "testables" -NotePropertyValue @("com.kukuri.gamesdk") -Force
[IO.File]::WriteAllText((Join-Path $ProjectDir "Packages\manifest.json"), ($manifest | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding $false))
# sample は Package Manager の Import と同じく Samples~ から写す（新しい project へそのまま入ることを確かめる）。
$sample = Join-Path $ProjectDir "Assets\Samples\SkinLoader"
New-Item -ItemType Directory -Force $sample | Out-Null
Copy-Item (Join-Path $package "Samples~\SkinLoader\*.cs") $sample

$env:KUKURI_DISABLE_KEYRING = "1"
$env:KUKURI_DISCOVERY_MODE = "static_peer"
$env:KUKURI_BIND_ADDR = "127.0.0.1:0"
$env:KUKURI_ADVERTISE_HOST = "127.0.0.1"
foreach ($name in "KUKURI_APP_DATA_DIR", "KUKURI_INSTANCE", "KUKURI_DISCOVERY_SEEDS", "KUKURI_ADVERTISE_PORT") { Remove-Item "env:$name" -ErrorAction SilentlyContinue }

$results = Join-Path $ProjectDir "editmode-results.xml"
$log = Join-Path $ProjectDir "editmode.log"
# graphics を使い、差し替えの前後の描画を %TEMP%\kgs-skin-*.png に書く。
& $Unity -batchmode -projectPath $ProjectDir -runTests -testPlatform EditMode -assemblyNames "Kukuri.GameSdk.Assets.Tests" -testResults $results -logFile $log | Out-Null
if (-not (Test-Path $results)) { throw "EditMode tests did not run; see $log" }
[xml]$xml = Get-Content $results
$run = $xml.'test-run'
Write-Output "EditMode (glTFast): total=$($run.total) passed=$($run.passed) failed=$($run.failed)"
$line = Select-String -Path $log -Pattern "KGS_SKIN .*" | Select-Object -First 1
if ($line) { Write-Output $line.Matches[0].Value }
if (($run.failed -ne "0") -or ($run.total -eq "0")) { exit 1 }
