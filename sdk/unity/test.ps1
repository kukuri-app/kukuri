# 空の Unity project に Game SDK の package（と Minimal sample）を入れ、EditMode の試験（lifecycle を含む）を流し、
# NGO などの Gameplay の package を入れないまま Windows x64 の Player を build して、次を確かめる（#1977 AC-1・AC-2）。
# - Player で Minimal sample が FFI で起動の状態を読める（daemon は不要）
# - 同じ profile で Player を 2 回起動・終了でき、同意したアカウントの公開鍵が同じで、終了 code が 0、Sidecar の process が無い
# 先に build-native.ps1 で native を置く。Unity の project は短い path に作る（深い path は MAX_PATH に当たる）。
param(
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Unity.exe",
    [string]$ProjectDir = (Join-Path $env:TEMP "kgs-test"),
    [ValidateSet("Mono2x", "IL2CPP")] [string]$Backend = "Mono2x",
    [switch]$SkipEditMode
)
$ErrorActionPreference = "Stop"
$package = (Resolve-Path (Join-Path $PSScriptRoot "com.kukuri.gamesdk")).Path
if (-not (Test-Path (Join-Path $package "Runtime\Plugins\x86_64\kukuri_game_sdk.dll"))) { throw "run build-native.ps1 first" }

if (Test-Path $ProjectDir) { Remove-Item -Recurse -Force $ProjectDir }
& $Unity -batchmode -nographics -quit -createProject $ProjectDir -logFile (Join-Path $env:TEMP "kgs-create.log") | Out-Null
$manifest = Get-Content (Join-Path $ProjectDir "Packages\manifest.json") -Raw | ConvertFrom-Json
$manifest.dependencies | Add-Member -NotePropertyName "com.kukuri.gamesdk" -NotePropertyValue ("file:" + ($package -replace '\\', '/')) -Force
$manifest.dependencies | Add-Member -NotePropertyName "com.unity.test-framework" -NotePropertyValue "1.6.0" -Force
$manifest | Add-Member -NotePropertyName "testables" -NotePropertyValue @("com.kukuri.gamesdk") -Force
# Unity の Package Manager は BOM のある JSON を読めないので、BOM なしの UTF-8 で書く。
[IO.File]::WriteAllText((Join-Path $ProjectDir "Packages\manifest.json"), ($manifest | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding $false))
# sample は package の Samples~ から写す（Package Manager の Import と同じ）。
$sample = Join-Path $ProjectDir "Assets\Samples\Minimal"
New-Item -ItemType Directory -Force $sample | Out-Null
Copy-Item (Join-Path $package "Samples~\Minimal\*.cs") $sample

# runtime を組み立てる試験は keyring を使わず loopback だけで動かす（CLI の process 試験と同じ）。
$env:KUKURI_DISABLE_KEYRING = "1"
$env:KUKURI_DISCOVERY_MODE = "static_peer"
$env:KUKURI_BIND_ADDR = "127.0.0.1:0"
$env:KUKURI_ADVERTISE_HOST = "127.0.0.1"
foreach ($name in "KUKURI_APP_DATA_DIR", "KUKURI_INSTANCE", "KUKURI_DISCOVERY_SEEDS", "KUKURI_ADVERTISE_PORT") { Remove-Item "env:$name" -ErrorAction SilentlyContinue }

$failed = $false
if (-not $SkipEditMode) {
    $results = Join-Path $ProjectDir "editmode-results.xml"
    & $Unity -batchmode -nographics -projectPath $ProjectDir -runTests -testPlatform EditMode -testResults $results -logFile (Join-Path $ProjectDir "editmode.log") | Out-Null
    if (-not (Test-Path $results)) { throw "EditMode tests did not run; see $ProjectDir\editmode.log" }
    [xml]$xml = Get-Content $results
    $run = $xml.'test-run'
    Write-Output "EditMode: total=$($run.total) passed=$($run.passed) failed=$($run.failed)"
    if ($run.failed -ne "0") { $failed = $true }
}

# Gameplay の package（NGO 等）が入っていないことを確かめてから、Player を build する。
$lock = Get-Content (Join-Path $ProjectDir "Packages\packages-lock.json") -Raw
if ($lock -match "com\.unity\.netcode") { throw "the test project unexpectedly depends on Netcode" }

# 再起動の確認用の probe。`-kgs-restart` のときだけ、同意して公開鍵を log に書き、終わる。
@"
using System.Linq;
using Kukuri.GameSdk;
using Newtonsoft.Json.Linq;
using UnityEngine;
public sealed class KgsRestartProbe : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;
    async void Start()
    {
        if (!System.Environment.GetCommandLineArgs().Contains("-kgs-restart")) return;
        while (runner.Client == null) await System.Threading.Tasks.Task.Yield();
        var client = runner.Client;
        var startup = await client.SendAsync("get_desktop_startup_status");
        if ((string)startup.Data["status"] == "consent_required")
        {
            var consent = await client.SendAsync("get_app_consent_status");
            var documents = new JArray();
            foreach (var document in (JArray)consent.Data["documents"])
                documents.Add(new JObject { ["slug"] = document["slug"], ["version"] = document["currentVersion"] });
            await client.SendAsync("accept_app_consents", new JObject { ["documents"] = documents, ["language"] = "ja", ["age_attested"] = true });
        }
        var sync = await client.SendAsync("get_sync_status");
        Debug.Log("KGS_PUBKEY " + (sync.Ok ? (string)sync.Data["local_author_pubkey"] : "error:" + sync.ErrorCode));
        Application.Quit(0);
    }
}
"@ | Set-Content (Join-Path $ProjectDir "Assets\KgsRestartProbe.cs") -Encoding utf8

$builder = Join-Path $ProjectDir "Assets\Editor"
New-Item -ItemType Directory -Force $builder | Out-Null
@"
using Kukuri.GameSdk;
using UnityEditor;
using UnityEditor.Build;
using UnityEditor.SceneManagement;
using UnityEngine;
public static class KgsBuild
{
    static void Link(Object component, string field, Object value)
    {
        component.GetType().GetField(field, System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance).SetValue(component, value);
    }

    public static void Build()
    {
        // KukuriRunner（FFI）と Minimal sample と再起動の probe だけの scene。
        var scene = EditorSceneManager.NewScene(NewSceneSetup.DefaultGameObjects, NewSceneMode.Single);
        var settings = KukuriSettings.Create(KukuriMode.Ffi, "sample", "");
        AssetDatabase.CreateAsset(settings, "Assets/KukuriSettings.asset");
        var host = new GameObject("kukuri");
        var runner = host.AddComponent<KukuriRunner>();
        runner.Settings = settings;
        Link(host.AddComponent<MinimalSample>(), "runner", runner);
        Link(host.AddComponent<KgsRestartProbe>(), "runner", runner);
        EditorSceneManager.SaveScene(scene, "Assets/Sample.unity");
        PlayerSettings.SetScriptingBackend(NamedBuildTarget.Standalone, ScriptingImplementation.$Backend);
        var report = BuildPipeline.BuildPlayer(new BuildPlayerOptions
        {
            scenes = new[] { "Assets/Sample.unity" },
            locationPathName = "Builds/$Backend/Kgs.exe",
            target = BuildTarget.StandaloneWindows64,
        });
        Debug.Log("KGS_BUILD " + report.summary.result);
        EditorApplication.Exit(report.summary.result == UnityEditor.Build.Reporting.BuildResult.Succeeded ? 0 : 1);
    }
}
"@ | Set-Content (Join-Path $builder "KgsBuild.cs") -Encoding utf8
& $Unity -batchmode -nographics -quit -projectPath $ProjectDir -executeMethod KgsBuild.Build -logFile (Join-Path $ProjectDir "build.log") | Out-Null
$exe = Join-Path $ProjectDir "Builds\$Backend\Kgs.exe"
$built = Test-Path $exe
$dll = Test-Path (Join-Path $ProjectDir "Builds\$Backend\Kgs_Data\Plugins\x86_64\kukuri_game_sdk.dll")
Write-Output "Player ($Backend): built=$built native_dll_included=$dll"
if (-not $built -or -not $dll) { exit 1 }

# Player の profile は persistentDataPath（LocalLow\DefaultCompany\kgs-test）の下。前の回の profile を消してから始める。
$profileRoot = Join-Path $env:USERPROFILE "AppData\LocalLow\DefaultCompany\kgs-test\kukuri"
if (Test-Path $profileRoot) { Remove-Item -Recurse -Force $profileRoot }

function Start-Player([string]$name, [string]$extra) {
    $log = Join-Path $ProjectDir "$name.log"
    $process = Start-Process -FilePath $exe -ArgumentList "-screen-fullscreen 0 -screen-width 640 -screen-height 480 $extra -logFile `"$log`"" -PassThru
    return @{ Process = $process; Log = $log }
}

# Player で sample を動かし、FFI で起動・要求・結果が通ることを log で確かめる（daemon は不要）。
$player = Start-Player "player" ""
$sampleOk = $false
for ($i = 0; $i -lt 60 -and -not $sampleOk; $i++) {
    Start-Sleep -Milliseconds 500
    if (Test-Path $player.Log) { $sampleOk = (Select-String -Path $player.Log -Pattern "kukuri sample: status=consent_required" -Quiet) }
}
Stop-Process -Id $player.Process.Id -Force -ErrorAction SilentlyContinue
$player.Process.WaitForExit()
Write-Output "Player sample (FFI): status_read=$sampleOk"

# 同じ profile で 2 回起動・終了する。2 回目も同じ公開鍵で、どちらも終了 code 0、Sidecar の process は動かない。
$keys = @()
$exits = @()
$sidecars = 0
foreach ($round in 1, 2) {
    $run = Start-Player "restart$round" "-kgs-restart"
    if (-not $run.Process.WaitForExit(120000)) { Stop-Process -Id $run.Process.Id -Force; $exits += "timeout" } else { $exits += $run.Process.ExitCode }
    $sidecars += @(Get-Process -Name "kukuri-sdk-sidecar" -ErrorAction SilentlyContinue).Count
    $line = Select-String -Path $run.Log -Pattern "KGS_PUBKEY (\S+)" | Select-Object -First 1
    $keys += if ($line) { $line.Matches[0].Groups[1].Value } else { "missing" }
}
$restartOk = ($keys[0] -match "^[0-9a-f]{64}$") -and ($keys[0] -eq $keys[1]) -and ($exits[0] -eq 0) -and ($exits[1] -eq 0) -and ($sidecars -eq 0)
Write-Output "Player restart ($Backend): keys=$($keys -join ',') exits=$($exits -join ',') sidecar_processes=$sidecars ok=$restartOk"

if ($failed -or -not $sampleOk -or -not $restartOk) { exit 1 }
