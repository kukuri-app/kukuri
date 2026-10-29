# 空の Unity project に Game SDK の package（と Minimal sample）を入れ、EditMode の試験を流し、
# NGO などの Gameplay の package を入れないまま Windows x64 の Player を build する（#1977 AC-1）。
# 先に build-native.ps1 で native を置く。Unity の project は短い path に作る（深い path は MAX_PATH に当たる）。
param(
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Unity.exe",
    [string]$ProjectDir = (Join-Path $env:TEMP "kgs-test"),
    [ValidateSet("Mono2x", "IL2CPP")] [string]$Backend = "Mono2x"
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

$results = Join-Path $ProjectDir "editmode-results.xml"
& $Unity -batchmode -nographics -projectPath $ProjectDir -runTests -testPlatform EditMode -testResults $results -logFile (Join-Path $ProjectDir "editmode.log") | Out-Null
if (-not (Test-Path $results)) { throw "EditMode tests did not run; see $ProjectDir\editmode.log" }
[xml]$xml = Get-Content $results
$run = $xml.'test-run'
Write-Output "EditMode: total=$($run.total) passed=$($run.passed) failed=$($run.failed)"

# Gameplay の package（NGO 等）が入っていないことを確かめてから、Player を build する。
$lock = Get-Content (Join-Path $ProjectDir "Packages\packages-lock.json") -Raw
if ($lock -match "com\.unity\.netcode") { throw "the test project unexpectedly depends on Netcode" }
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
    public static void Build()
    {
        // KukuriRunner（FFI）と Minimal sample だけの scene。
        var scene = EditorSceneManager.NewScene(NewSceneSetup.DefaultGameObjects, NewSceneMode.Single);
        var settings = KukuriSettings.Create(KukuriMode.Ffi, "sample", "");
        AssetDatabase.CreateAsset(settings, "Assets/KukuriSettings.asset");
        var host = new GameObject("kukuri");
        var runner = host.AddComponent<KukuriRunner>();
        runner.Settings = settings;
        var sample = host.AddComponent<MinimalSample>();
        var field = typeof(MinimalSample).GetField("runner", System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);
        field.SetValue(sample, runner);
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

# Player で sample を動かし、FFI で起動・要求・結果が通ることを log で確かめる（daemon は不要）。
$sampleOk = $false
if ($built) {
    $playerLog = Join-Path $ProjectDir "player.log"
    $player = Start-Process -FilePath $exe -ArgumentList "-screen-fullscreen 0 -screen-width 640 -screen-height 480 -logFile `"$playerLog`"" -PassThru
    for ($i = 0; $i -lt 60 -and -not $sampleOk; $i++) {
        Start-Sleep -Milliseconds 500
        if (Test-Path $playerLog) { $sampleOk = (Select-String -Path $playerLog -Pattern "kukuri sample: status=consent_required" -Quiet) }
    }
    Stop-Process -Id $player.Id -Force -ErrorAction SilentlyContinue
}
Write-Output "Player sample (FFI): status_read=$sampleOk"
if ($run.failed -ne "0" -or -not $built -or -not $dll -or -not $sampleOk) { exit 1 }
