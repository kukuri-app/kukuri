# 空の Unity project に Game SDK の package（と Minimal sample）を入れ、EditMode の試験（lifecycle を含む）を流し、
# NGO などの Gameplay の package を入れないまま Windows x64 の Player を build して、次を確かめる（#1977 AC-1・AC-2）。
# - Player で Minimal sample が FFI で起動の状態を読める（daemon は不要）
# - 同じ profile で Player を 3 回起動・終了でき、終了 code が 0、Sidecar の process が無い。開始の画面の sample で、同意の後
#   「はじめる」の前に閉じても同じアカウントのまま開始の画面に戻り、「はじめる」で game player を作り、次の起動では同じ
#   game player と公開鍵で自動で再開する（#1996 AC-2）
# - -DesktopApp に desktop（配布と同じ build）を渡すと、desktop の「リンクをコピー」を Player の「kukuriアカウントを使う」→
#   「貼り付け」で受け、両方で同じ確認コードを承認して desktop の公開鍵で「はじめる」、次の起動で同じ game player と公開鍵で
#   再開する（#1996 AC-3）。続けて、Player の「kukuriで続ける」のリンクを desktop の「別の端末から移す」で受けて
#   desktop をゲームのアカウントに切り替え、desktop がゲームの投稿に返信し、Player がその返信を受けて同じ game player の
#   まま続く（#1998 AC-2）。desktop は transfer-desktop.mjs が画面から動かす。node が要る
# - Steam Cloud への保存を模して、保存した profile の game-account.json だけを一度も開いていない profile へ写し、同じ
#   公開鍵と game player ID で再開する（#1997 AC-3。実際の Steam は未確認）
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

# 開始の画面の確認用の probe。`-kgs-close`・`-kgs-begin`・`-kgs-resume`・`-kgs-transfer`・`-kgs-handoff` のときだけ sample
# を操作し、画面・公開鍵・game player ID を log に書き、画面を %TEMP%\kgs-start-*.png に撮って終わる。`-kgs-transfer` は、
# desktop がリンクをコピーした印（%KGS_TRANSFER_DIR%\copied）を待ってから貼り付ける。`-kgs-handoff` は試験の topic に投稿し、
# 「kukuriで続ける」でリンクをコピーしてから desktop と %KGS_TRANSFER_DIR% の file でやり取りする。`-kgs-board-post` は、始めてから掲示板の prefab で投稿し、desktop
# が読んだ印を待って終わる。`-kgs-board-resume` は、desktop の返信の印を待ってから掲示板でその投稿の thread を開き、返信を読む。
@"
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Newtonsoft.Json.Linq;
using Kukuri.GameSdk;
using UnityEngine;
public sealed class KgsStartProbe : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;
    [SerializeField] MinimalSample sample;
    [SerializeField] Kukuri.GameSdk.Social.KukuriBoard board;
    static string Mode()
    {
        var args = System.Environment.GetCommandLineArgs();
        return new[] { "close", "begin", "resume", "transfer", "handoff", "board-post", "board-resume", "cloud-save", "cloud-restore" }.FirstOrDefault(mode => args.Contains("-kgs-" + mode));
    }
    async Task Shot(string name)
    {
        for (var i = 0; i < 5; i++) await Task.Yield();
        await Awaitable.EndOfFrameAsync();
        ScreenCapture.CaptureScreenshot(System.IO.Path.Combine(System.IO.Path.GetTempPath(), "kgs-start-" + name + ".png"));
        for (var i = 0; i < 5; i++) await Awaitable.NextFrameAsync();
    }
    /// desktop と互いの ticket を取り込む（loopback の static peer）。
    async Task Exchange(string dir)
    {
        var ticket = await runner.Client.SendAsync("get_local_peer_ticket");
        File.WriteAllText(Path.Combine(dir, "game-ticket"), (string)ticket.Data);
        var desktopTicket = Path.Combine(dir, "desktop-ticket");
        while (!File.Exists(desktopTicket)) await Task.Delay(200);
        await runner.Client.SendAsync("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(desktopTicket).Trim() });
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
        if (mode == "handoff" && sample.Screen == "start")
        {
            var dir = System.Environment.GetEnvironmentVariable("KGS_TRANSFER_DIR");
            const string topic = "kukuri:topic:test";
            await sample.Begin();
            await runner.Client.SendAsync("set_topic_gossip_enabled", new JObject { ["topic"] = topic, ["enabled"] = true });
            var posted = await runner.Client.SendAsync("create_post", new JObject
            {
                ["topic"] = topic, ["content"] = "test: from the game", ["channel_ref"] = new JObject { ["kind"] = "public" },
            });
            var post = (string)posted.Data;
            string reply = null;
            runner.Client.EventReceived += item =>
            {
                if ((string)item["type"] == "post_received" && (string)item["post"]?["reply_to"] == post)
                    reply = (string)item["post"]["object_id"] + " " + (string)item["post"]["author_pubkey"];
            };
            File.WriteAllText(Path.Combine(dir, "game-post"), post);
            var handing = sample.HandOff();
            while ((string)sample.Transfer?["state"] != "waiting" && !handing.IsCompleted) await Task.Yield();
            File.WriteAllText(Path.Combine(dir, "handoff"), await Key());
            await Shot(mode + "-2");
            while ((string)sample.Transfer?["state"] != "confirming" && !handing.IsCompleted) await Task.Yield();
            Debug.Log("KGS_CODE " + (sample.Transfer?["code"] ?? "none"));
            await Shot(mode + "-3");
            await sample.Decide(true);
            await handing;
            Debug.Log("KGS_HANDED " + sample.Screen + " " + await Key());
            var ticket = await runner.Client.SendAsync("get_local_peer_ticket");
            File.WriteAllText(Path.Combine(dir, "game-ticket"), (string)ticket.Data);
            var desktopTicket = Path.Combine(dir, "desktop-ticket");
            while (!File.Exists(desktopTicket)) await Task.Delay(200);
            await runner.Client.SendAsync("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(desktopTicket).Trim() });
            for (var waited = 0; reply == null && waited < 1200; waited++) await Task.Delay(100);
            Debug.Log("KGS_REPLY " + (reply ?? "none none"));
            await Shot(mode + "-4");
            File.WriteAllText(Path.Combine(dir, "received"), reply ?? "none");
        }
        if (mode == "board-post" && sample.Screen == "start")
        {
            var dir = System.Environment.GetEnvironmentVariable("KGS_TRANSFER_DIR");
            await sample.Begin();
            // 画面には掲示板だけを出す（sample の OnGUI は Canvas の上に描かれる）。
            sample.enabled = false;
            board.gameObject.SetActive(true);
            await board.RefreshAsync();
            var post = await board.PostAsync("test: from the game board");
            File.WriteAllText(Path.Combine(dir, "game-post"), post);
            await Exchange(dir);
            var seen = Path.Combine(dir, "seen");
            for (var waited = 0; !File.Exists(seen) && waited < 1800; waited++) await Task.Delay(100);
            await Shot(mode + "-2");
            Debug.Log("KGS_BOARD_POST " + post + " " + await Key());
        }
        if (mode == "board-resume" && sample.Screen == "playing")
        {
            var dir = System.Environment.GetEnvironmentVariable("KGS_TRANSFER_DIR");
            var replied = Path.Combine(dir, "replied");
            while (!File.Exists(replied)) await Task.Delay(200);
            var reply = File.ReadAllText(replied).Trim();
            await Exchange(dir);
            sample.enabled = false;
            board.gameObject.SetActive(true);
            await board.OpenThreadAsync(File.ReadAllText(Path.Combine(dir, "game-post")).Trim());
            for (var waited = 0; board.Posts.All(item => item.ObjectId != reply) && waited < 240; waited++)
            {
                await Task.Delay(500);
                await board.ReadNewAsync();
            }
            var root = board.Posts.FirstOrDefault();
            var answer = board.Posts.FirstOrDefault(item => item.ObjectId == reply);
            Debug.Log("KGS_BOARD_THREAD " + (root?.ObjectId ?? "none") + " " + (root?.AuthorPubkey ?? "none") + " " +
                (answer?.ObjectId ?? "none") + " " + (answer?.AuthorPubkey ?? "none") + " " + (answer?.ThreadId ?? "none"));
            await Shot(mode + "-2");
            File.WriteAllText(Path.Combine(dir, "done"), "done");
        }
        if (mode == "cloud-save" && sample.Screen == "start")
        {
            await sample.Begin();
            await sample.SetCloud(true);
            Debug.Log("KGS_CLOUD " + sample.CloudState);
            await Shot(mode + "-2");
        }
        if (mode == "cloud-restore" && sample.Screen == "start")
        {
            var before = await runner.Client.SendAsync("get_cloud_account", new JObject { ["application_id"] = "com.example.minimal" });
            Debug.Log("KGS_CLOUD " + (string)before.Data?["state"]);
            await sample.Begin();
            Debug.Log("KGS_CLOUD " + sample.CloudState);
            await Shot(mode + "-2");
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
    static void Link(Object component, string field, object value)
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
        Link(sample, "steamCloud", true);
        var probe = host.AddComponent<KgsStartProbe>();
        Link(probe, "runner", runner);
        Link(probe, "sample", sample);
        // 掲示板は prefab のまま置き、probe が開くまで止めておく（使わない SNS の UI は動かさない）。
        var board = (GameObject)PrefabUtility.InstantiatePrefab(
            AssetDatabase.LoadAssetAtPath<GameObject>("Packages/com.kukuri.gamesdk/Runtime/Social/KukuriBoard.prefab"));
        var boardComponent = board.GetComponent<Kukuri.GameSdk.Social.KukuriBoard>();
        Link(boardComponent, "runner", runner);
        Link(boardComponent, "topic", "kukuri:topic:test");
        board.SetActive(false);
        Link(probe, "board", boardComponent);
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

# Steam Cloud の同期を模す: 保存した profile の game-account.json だけを残して、profile を一度も開いていない状態にする。
if (Test-Path $profileRoot) { Remove-Item -Recurse -Force $profileRoot }
$cloud = @()
$exits = @()
foreach ($mode in "cloud-save", "cloud-restore") {
    if ($mode -eq "cloud-restore") {
        $profileDir = Join-Path $profileRoot "profiles\sample"
        $saved = Join-Path $env:TEMP "kgs-game-account.json"
        Copy-Item (Join-Path $profileDir "game-account.json") $saved -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force $profileRoot
        New-Item -ItemType Directory -Force $profileDir | Out-Null
        Move-Item $saved (Join-Path $profileDir "game-account.json") -ErrorAction SilentlyContinue
    }
    $run = Start-Player "start-$mode" "-kgs-$mode"
    if (-not $run.Process.WaitForExit(120000)) { Stop-Process -Id $run.Process.Id -Force; $exits += "timeout" } else { $exits += $run.Process.ExitCode }
    $states = @(Select-String -Path $run.Log -Pattern "KGS_CLOUD (\S+)" | ForEach-Object { $_.Matches[0].Groups[1].Value })
    $player = Select-String -Path $run.Log -Pattern "KGS_PLAYER (\S+) (\S+) (\S+)" | Select-Object -First 1
    $cloud += ($states -join ",") + " " + $(if ($player) { ($player.Matches[0].Groups[1..3] | ForEach-Object Value) -join " " } else { "missing" })
}
$id = ($cloud[0] -split " ")[2]
$key = ($cloud[0] -split " ")[3]
$cloudOk = ($cloud[0] -eq "on playing $id $key") -and ($cloud[1] -eq "restorable,on playing $id $key") -and
    ($id -match "^[0-9a-f-]{36}$") -and ($key -match "^[0-9a-f]{64}$") -and (@($exits | Where-Object { $_ -ne 0 }).Count -eq 0)
Write-Output "Player cloud ($Backend): runs=$($cloud -join ' | ') exits=$($exits -join ',') ok=$cloudOk"

# log の最初の一致の 1 つ目の group（無ければ "missing"）。
function Find-Value([string]$path, [string]$pattern) {
    $found = Select-String -Path $path -Pattern $pattern -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($found) { $found.Matches[0].Groups[1].Value } else { "missing" }
}

# 新しい profile と作業 dir（%TEMP%\kgs-transfer-<mode>）を用意し、desktop を transfer-desktop.mjs で動かす。
function Start-Desktop([string]$mode) {
    if (Test-Path $profileRoot) { Remove-Item -Recurse -Force $profileRoot }
    $env:KGS_TRANSFER_DIR = Join-Path $env:TEMP "kgs-transfer-$mode"
    if (Test-Path $env:KGS_TRANSFER_DIR) { Remove-Item -Recurse -Force $env:KGS_TRANSFER_DIR }
    New-Item -ItemType Directory -Force $env:KGS_TRANSFER_DIR | Out-Null
    $log = Join-Path $env:KGS_TRANSFER_DIR "desktop.log"
    $process = Start-Process -FilePath "node" -ArgumentList "`"$(Join-Path $PSScriptRoot "transfer-desktop.mjs")`" `"$DesktopApp`" `"$env:KGS_TRANSFER_DIR`" $mode" `
        -RedirectStandardOutput $log -RedirectStandardError (Join-Path $env:KGS_TRANSFER_DIR "desktop.err") -NoNewWindow -PassThru
    # handle を先に開いておかないと、終了の後に ExitCode が読めない（Start-Process -PassThru の挙動）。
    $null = $process.Handle
    return @{ Process = $process; Log = $log }
}

$transferOk = $true
$handoffOk = $true
$boardOk = $true
if ($DesktopApp) {
    # 新しい profile で、desktop の既存のアカウントを移行で受けてから始め、次の起動で再開する（#1996 AC-3）。
    $source = Start-Desktop "source"
    $sourceLog = $source.Log
    $source = $source.Process
    $received = @()
    $exits = @()
    foreach ($mode in "transfer", "resume") {
        $run = Start-Player "start-$mode-transfer" "-kgs-$mode"
        if (-not $run.Process.WaitForExit(300000)) { Stop-Process -Id $run.Process.Id -Force; $exits += "timeout" } else { $exits += $run.Process.ExitCode }
        $player = Select-String -Path $run.Log -Pattern "KGS_PLAYER (\S+) (\S+) (\S+)" | Select-Object -First 1
        $received += if ($player) { ($player.Matches[0].Groups[1..3] | ForEach-Object Value) -join " " } else { "missing" }
        if ($mode -eq "transfer") {
            $code = Find-Value $run.Log "KGS_CODE (\S+)"
            $screen = Find-Value $run.Log "KGS_RECEIVED (\S+ \S+)"
        }
    }
    if (-not $source.WaitForExit(60000)) { Stop-Process -Id $source.Id -Force }
    $desktopKey = Get-Content (Join-Path $env:KGS_TRANSFER_DIR "copied") -ErrorAction SilentlyContinue
    $sourceCode = Find-Value $sourceLog "confirming (\d{6})"
    $id = ($received[0] -split " ")[1]
    $transferOk = ($desktopKey -match "^[0-9a-f]{64}$") -and ($code -eq $sourceCode) -and ($screen -eq "start $desktopKey") -and
        ($id -match "^[0-9a-f-]{36}$") -and ($received[0] -eq "playing $id $desktopKey") -and ($received[1] -eq "playing $id $desktopKey") -and
        (Select-String -Path $sourceLog -Pattern "^completed" -Quiet) -and (@($exits | Where-Object { $_ -ne 0 }).Count -eq 0)
    Write-Output "Player transfer ($Backend): desktop=$desktopKey codes=$code/$sourceCode received=$screen players=$($received -join ' | ') exits=$($exits -join ',') ok=$transferOk"

    # 新しい profile で始めて投稿し、「kukuriで続ける」で desktop へ移す。desktop が返信し、Player が受けて続く（#1998 AC-2）。
    $target = Start-Desktop "target"
    $handed = @()
    $exits = @()
    foreach ($mode in "handoff", "resume") {
        $run = Start-Player "start-$mode-handoff" "-kgs-$mode"
        if (-not $run.Process.WaitForExit(300000)) { Stop-Process -Id $run.Process.Id -Force; $exits += "timeout" } else { $exits += $run.Process.ExitCode }
        $player = Select-String -Path $run.Log -Pattern "KGS_PLAYER (\S+) (\S+) (\S+)" | Select-Object -First 1
        $handed += if ($player) { ($player.Matches[0].Groups[1..3] | ForEach-Object Value) -join " " } else { "missing" }
        if ($mode -eq "handoff") {
            $code = Find-Value $run.Log "KGS_CODE (\S+)"
            $reply = Find-Value $run.Log "KGS_REPLY (\S+ \S+)"
        }
    }
    if (-not $target.Process.WaitForExit(120000)) { Stop-Process -Id $target.Process.Id -Force; $exits += "desktop-timeout" } else { $exits += $target.Process.ExitCode }
    $gameKey = Get-Content (Join-Path $env:KGS_TRANSFER_DIR "handoff") -ErrorAction SilentlyContinue
    $targetCode = Find-Value $target.Log "confirming (\d{6})"
    $replyId = Find-Value $target.Log "replied (\S+)"
    $id = ($handed[0] -split " ")[1]
    $handoffOk = ($gameKey -match "^[0-9a-f]{64}$") -and ($code -eq $targetCode) -and ($reply -eq "$replyId $gameKey") -and
        (Select-String -Path $target.Log -Pattern "^switched to $gameKey" -Quiet) -and (Select-String -Path $target.Log -Pattern "^shown" -Quiet) -and
        ($id -match "^[0-9a-f-]{36}$") -and ($handed[0] -eq "playing $id $gameKey") -and ($handed[1] -eq "playing $id $gameKey") -and
        (@($exits | Where-Object { $_ -ne 0 }).Count -eq 0)
    Write-Output "Player handoff ($Backend): game=$gameKey codes=$code/$targetCode reply=$reply players=$($handed -join ' | ') exits=$($exits -join ',') ok=$handoffOk"

    # 掲示板の prefab で投稿してゲームを終える。desktop がその間に返信し、再起動したゲームの掲示板がその thread で返信を
    # 読む（同じ投稿の ID・thread。desktop が保持者）（#1981 AC-2）。
    $desktop = Start-Desktop "board"
    $exits = @()
    $run = Start-Player "start-board-post" "-kgs-board-post"
    if (-not $run.Process.WaitForExit(300000)) { Stop-Process -Id $run.Process.Id -Force; $exits += "timeout" } else { $exits += $run.Process.ExitCode }
    $posted = Find-Value $run.Log "KGS_BOARD_POST (\S+ \S+)"
    Set-Content (Join-Path $env:KGS_TRANSFER_DIR "game-exited") "exited"
    $run = Start-Player "start-board-resume" "-kgs-board-resume"
    if (-not $run.Process.WaitForExit(300000)) { Stop-Process -Id $run.Process.Id -Force; $exits += "timeout" } else { $exits += $run.Process.ExitCode }
    $thread = Find-Value $run.Log "KGS_BOARD_THREAD (\S+ \S+ \S+ \S+ \S+)"
    if (-not $desktop.Process.WaitForExit(120000)) { Stop-Process -Id $desktop.Process.Id -Force; $exits += "desktop-timeout" } else { $exits += $desktop.Process.ExitCode }
    $post, $gameKey = $posted -split " "
    $reply = Find-Value $desktop.Log "replied (\S+)"
    $desktopKey = Get-Content (Join-Path $env:KGS_TRANSFER_DIR "desktop-key") -ErrorAction SilentlyContinue
    $boardOk = ($gameKey -match "^[0-9a-f]{64}$") -and ($desktopKey -match "^[0-9a-f]{64}$") -and
        ($thread -eq "$post $gameKey $reply $desktopKey $post") -and (@($exits | Where-Object { $_ -ne 0 }).Count -eq 0)
    Write-Output "Player board ($Backend): post=$posted reply=$reply desktop=$desktopKey thread=$thread exits=$($exits -join ',') ok=$boardOk"
}

if ($failed -or -not $sampleOk -or -not $startOk -or -not $cloudOk -or -not $transferOk -or -not $handoffOk -or -not $boardOk) { exit 1 }
