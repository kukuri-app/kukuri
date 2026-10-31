# 空の Unity project に Game SDK の package（と Minimal sample）を入れ、EditMode の試験（lifecycle を含む）を流し、
# NGO などの Gameplay の package を入れないまま Windows x64 の Player を build して、次を確かめる（#1977 AC-1・AC-2）。
# - Player で Minimal sample が FFI で起動の状態を読める（daemon は不要）
# - 同じ profile で Player を 3 回起動・終了でき、終了 code が 0、Sidecar の process が無い。開始の画面の sample で、同意の後
#   「はじめる」の前に閉じても同じアカウントのまま開始の画面に戻り、「はじめる」で game player を作り、次の起動では同じ
#   game player と公開鍵で自動で再開する（#1996 AC-2）
# - -DesktopApp に desktop（配布と同じ build）を渡すと、desktop の「リンクをコピー」を Player の「kukuriアカウントを使う」→
#   「貼り付け」で受け、両方で同じ確認コードを承認して desktop の公開鍵で「はじめる」、次の起動で同じ game player と公開鍵で
#   再開する（#1996 AC-3。desktop は transfer-source.mjs が画面から動かす。node が要る）
# 先に build-native.ps1 で native を置く。Unity の project は短い path に作る（深い path は MAX_PATH に当たる）。
param(
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Unity.exe",
    [string]$ProjectDir = (Join-Path $env:TEMP "kgs-test"),
    [ValidateSet("Mono2x", "IL2CPP")] [string]$Backend = "Mono2x",
    [switch]$SkipEditMode,
    [string]$DesktopApp
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

# 開始の画面の確認用の probe。`-kgs-close`・`-kgs-begin`・`-kgs-resume`・`-kgs-transfer` のときだけ sample を操作し、画面・
# 公開鍵・game player ID を log に書き、画面を %TEMP%\kgs-start-*.png に撮って終わる。`-kgs-transfer` は、desktop が
# リンクをコピーした印（%KGS_TRANSFER_DIR%\copied）を待ってから貼り付ける。
@"
using System.Linq;
using System.Threading.Tasks;
using Kukuri.GameSdk;
using UnityEngine;
public sealed class KgsStartProbe : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;
    [SerializeField] MinimalSample sample;
    static string Mode()
    {
        var args = System.Environment.GetCommandLineArgs();
        return new[] { "close", "begin", "resume", "transfer" }.FirstOrDefault(mode => args.Contains("-kgs-" + mode));
    }
    async Task Shot(string name)
    {
        for (var i = 0; i < 5; i++) await Task.Yield();
        await Awaitable.EndOfFrameAsync();
        ScreenCapture.CaptureScreenshot(System.IO.Path.Combine(System.IO.Path.GetTempPath(), "kgs-start-" + name + ".png"));
        for (var i = 0; i < 5; i++) await Awaitable.NextFrameAsync();
    }
    async Task<string> Key()
    {
        var sync = await runner.Client.SendAsync("get_sync_status");
        return sync.Ok ? (string)sync.Data["local_author_pubkey"] : "error:" + sync.ErrorCode;
    }
    async void Start()
    {
        var mode = Mode();
        if (mode == null) return;
        while (sample.Screen == "loading") await Task.Yield();
        if (sample.Screen == "consent") await sample.Accept();
        while (sample.Screen == "loading") await Task.Yield();
        Debug.Log("KGS_SCREEN " + sample.Screen + " " + await Key());
        await Shot(mode + "-1");
        if (mode == "begin" && sample.Screen == "start")
        {
            await sample.Begin();
            await Shot(mode + "-2");
        }
        if (mode == "transfer" && sample.Screen == "start")
        {
            sample.UseKukuriAccount();
            await Shot(mode + "-2");
            var copied = System.IO.Path.Combine(System.Environment.GetEnvironmentVariable("KGS_TRANSFER_DIR"), "copied");
            while (!System.IO.File.Exists(copied)) await Task.Delay(200);
            var pasting = sample.Paste();
            while ((string)sample.Transfer?["state"] != "confirming" && !pasting.IsCompleted) await Task.Yield();
            Debug.Log("KGS_CODE " + (sample.Transfer?["code"] ?? "none"));
            await Shot(mode + "-3");
            await sample.Decide(true);
            await pasting;
            Debug.Log("KGS_RECEIVED " + sample.Screen + " " + await Key());
            await sample.Begin();
            await Shot(mode + "-4");
        }
        Debug.Log("KGS_PLAYER " + sample.Screen + " " + (sample.GamePlayerId == "" ? "none" : sample.GamePlayerId) + " " + await Key());
        Application.Quit(0);
    }
}
"@ | Set-Content (Join-Path $ProjectDir "Assets\KgsStartProbe.cs") -Encoding utf8

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
        // KukuriRunner（FFI）と Minimal sample と開始の画面の probe だけの scene。
        var scene = EditorSceneManager.NewScene(NewSceneSetup.DefaultGameObjects, NewSceneMode.Single);
        var settings = KukuriSettings.Create(KukuriMode.Ffi, "sample", "");
        AssetDatabase.CreateAsset(settings, "Assets/KukuriSettings.asset");
        var host = new GameObject("kukuri");
        var runner = host.AddComponent<KukuriRunner>();
        runner.Settings = settings;
        var sample = host.AddComponent<MinimalSample>();
        Link(sample, "runner", runner);
        var probe = host.AddComponent<KgsStartProbe>();
        Link(probe, "runner", runner);
        Link(probe, "sample", sample);
        EditorSceneManager.SaveScene(scene, "Assets/Sample.unity");
        PlayerSettings.SetScriptingBackend(NamedBuildTarget.Standalone, ScriptingImplementation.$Backend);
        // 試験の Player の window が focus を取れなくても frame を止めない（既定では focus を失うと止まる）。
        PlayerSettings.runInBackground = true;
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

# 同じ profile で 3 回起動・終了する。1 回目は同意して開始の画面で閉じ、2 回目は同じアカウントの開始の画面から「はじめる」、
# 3 回目は同じ game player と公開鍵で自動で再開する。どれも終了 code 0 で、Sidecar の process は動かない。
$screens = @()
$players = @()
$exits = @()
$sidecars = 0
foreach ($mode in "close", "begin", "resume") {
    $run = Start-Player "start-$mode" "-kgs-$mode"
    if (-not $run.Process.WaitForExit(120000)) { Stop-Process -Id $run.Process.Id -Force; $exits += "timeout" } else { $exits += $run.Process.ExitCode }
    $sidecars += @(Get-Process -Name "kukuri-sdk-sidecar" -ErrorAction SilentlyContinue).Count
    $screen = Select-String -Path $run.Log -Pattern "KGS_SCREEN (\S+) (\S+)" | Select-Object -First 1
    $screens += if ($screen) { $screen.Matches[0].Groups[1].Value + " " + $screen.Matches[0].Groups[2].Value } else { "missing" }
    $player = Select-String -Path $run.Log -Pattern "KGS_PLAYER (\S+) (\S+) (\S+)" | Select-Object -First 1
    $players += if ($player) { ($player.Matches[0].Groups[1..3] | ForEach-Object Value) -join " " } else { "missing" }
}
$key = ($screens[0] -split " ")[1]
$id = ($players[1] -split " ")[1]
$startOk = ($key -match "^[0-9a-f]{64}$") -and ($screens[0] -eq "start $key") -and ($players[0] -eq "start none $key") -and
    ($screens[1] -eq "start $key") -and ($id -match "^[0-9a-f-]{36}$") -and ($players[1] -eq "playing $id $key") -and
    ($screens[2] -eq "playing $key") -and ($players[2] -eq "playing $id $key") -and
    (@($exits | Where-Object { $_ -ne 0 }).Count -eq 0) -and ($sidecars -eq 0)
Write-Output "Player start screen ($Backend): screens=$($screens -join ' | ') players=$($players -join ' | ') exits=$($exits -join ',') sidecar_processes=$sidecars ok=$startOk"

$transferOk = $true
if ($DesktopApp) {
    # 新しい profile で、desktop の既存のアカウントを移行で受けてから始め、次の起動で再開する。
    if (Test-Path $profileRoot) { Remove-Item -Recurse -Force $profileRoot }
    $env:KGS_TRANSFER_DIR = Join-Path $env:TEMP "kgs-transfer"
    if (Test-Path $env:KGS_TRANSFER_DIR) { Remove-Item -Recurse -Force $env:KGS_TRANSFER_DIR }
    New-Item -ItemType Directory -Force $env:KGS_TRANSFER_DIR | Out-Null
    $sourceLog = Join-Path $env:KGS_TRANSFER_DIR "source.log"
    $source = Start-Process -FilePath "node" -ArgumentList "`"$(Join-Path $PSScriptRoot "transfer-source.mjs")`" `"$DesktopApp`" `"$env:KGS_TRANSFER_DIR`"" `
        -RedirectStandardOutput $sourceLog -RedirectStandardError (Join-Path $env:KGS_TRANSFER_DIR "source.err") -NoNewWindow -PassThru
    $received = @()
    $exits = @()
    foreach ($mode in "transfer", "resume") {
        $run = Start-Player "start-$mode-transfer" "-kgs-$mode"
        if (-not $run.Process.WaitForExit(300000)) { Stop-Process -Id $run.Process.Id -Force; $exits += "timeout" } else { $exits += $run.Process.ExitCode }
        $player = Select-String -Path $run.Log -Pattern "KGS_PLAYER (\S+) (\S+) (\S+)" | Select-Object -First 1
        $received += if ($player) { ($player.Matches[0].Groups[1..3] | ForEach-Object Value) -join " " } else { "missing" }
        if ($mode -eq "transfer") {
            $code = (Select-String -Path $run.Log -Pattern "KGS_CODE (\S+)" | Select-Object -First 1).Matches[0].Groups[1].Value
            $screen = (Select-String -Path $run.Log -Pattern "KGS_RECEIVED (\S+ \S+)" | Select-Object -First 1).Matches[0].Groups[1].Value
        }
    }
    if (-not $source.WaitForExit(60000)) { Stop-Process -Id $source.Id -Force }
    $desktopKey = Get-Content (Join-Path $env:KGS_TRANSFER_DIR "copied") -ErrorAction SilentlyContinue
    $sourceCode = (Select-String -Path $sourceLog -Pattern "confirming (\d{6})" | Select-Object -First 1).Matches[0].Groups[1].Value
    $id = ($received[0] -split " ")[1]
    $transferOk = ($desktopKey -match "^[0-9a-f]{64}$") -and ($code -eq $sourceCode) -and ($screen -eq "start $desktopKey") -and
        ($id -match "^[0-9a-f-]{36}$") -and ($received[0] -eq "playing $id $desktopKey") -and ($received[1] -eq "playing $id $desktopKey") -and
        (Select-String -Path $sourceLog -Pattern "^completed" -Quiet) -and (@($exits | Where-Object { $_ -ne 0 }).Count -eq 0)
    Write-Output "Player transfer ($Backend): desktop=$desktopKey codes=$code/$sourceCode received=$screen players=$($received -join ' | ') exits=$($exits -join ',') ok=$transferOk"
}

if ($failed -or -not $sampleOk -or -not $startOk -or -not $transferOk) { exit 1 }
