# Cyber Disc Arena の対戦 ring を確かめる（#1983 AC-1）。
# - EditMode: host の裁定の規則（当てる・受け止める・跳ねて止まる・5 点・時間切れと延長・自分の側）
# - 独立した 2 つの Windows Player（host と client）が、受信を片道 25 ms 遅らせ datagram を 1% 落とした状態で、入力を
#   流す bot で 1 試合を終える。両方が同じ出来事の列（通し番号・持ち主・得点）と勝敗を確定し、client の番号に飛びが無く、
#   試合の後の 2 人の位置が host と一致する。host が「ロビーへ戻る」と client に「ホストが部屋を閉じました」が出て、両方の
#   NGO が止まり、session から抜ける。続く 2 試合目で client が途中で退出すると、host に「相手が退出しました」が出る。
# 先に sdk/unity/build-native.ps1 で native を置く。Unity の project は短い path へ写して使う（深い path は MAX_PATH に当たる）。
param(
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Unity.exe",
    [string]$ProjectDir = (Join-Path $env:TEMP "kcda-test"),
    [ValidateSet("Mono2x", "IL2CPP")] [string]$Backend = "Mono2x",
    [switch]$SkipEditMode,
    [int]$DelayMs = 25,
    [double]$LossPercent = 1
)
$ErrorActionPreference = "Stop"
$package = (Resolve-Path (Join-Path $PSScriptRoot "..\..\sdk\unity\com.kukuri.gamesdk")).Path
if (-not (Test-Path (Join-Path $package "Runtime\Plugins\x86_64\kukuri_game_sdk.dll"))) { throw "run sdk/unity/build-native.ps1 first" }

if (Test-Path $ProjectDir) { Remove-Item -Recurse -Force $ProjectDir }
New-Item -ItemType Directory -Force $ProjectDir | Out-Null
foreach ($part in "Assets", "Packages", "ProjectSettings") { Copy-Item -Recurse (Join-Path $PSScriptRoot $part) $ProjectDir }
$manifestPath = Join-Path $ProjectDir "Packages\manifest.json"
$manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
$manifest.dependencies."com.kukuri.gamesdk" = "file:" + ($package -replace '\\', '/')
[IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding $false))

$env:KUKURI_DISABLE_KEYRING = "1"
$env:KUKURI_DISCOVERY_MODE = "static_peer"
$env:KUKURI_BIND_ADDR = "127.0.0.1:0"
$env:KUKURI_ADVERTISE_HOST = "127.0.0.1"
foreach ($name in "KUKURI_APP_DATA_DIR", "KUKURI_INSTANCE", "KUKURI_DISCOVERY_SEEDS", "KUKURI_ADVERTISE_PORT") { Remove-Item "env:$name" -ErrorAction SilentlyContinue }

$failed = $false
if (-not $SkipEditMode) {
    $results = Join-Path $ProjectDir "editmode-results.xml"
    & $Unity -batchmode -nographics -projectPath $ProjectDir -runTests -testPlatform EditMode -assemblyNames "CyberDiscArena.Tests" -testResults $results -logFile (Join-Path $ProjectDir "editmode.log") | Out-Null
    if (-not (Test-Path $results)) { throw "EditMode tests did not run; see $ProjectDir\editmode.log" }
    [xml]$xml = Get-Content $results
    $run = $xml.'test-run'
    Write-Output "EditMode (arena): total=$($run.total) passed=$($run.passed) failed=$($run.failed)"
    $failed = ($run.failed -ne "0") -or ($run.total -eq "0")
}

# Player の probe。`-arena-role host|client` のときだけ動く。入力は bot が流し、出来事と結果を log に書いて終わる。
$probeDir = Join-Path $ProjectDir "Assets\ArenaProbe"
New-Item -ItemType Directory -Force (Join-Path $probeDir "Editor") | Out-Null
@'
using System;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using CyberDiscArena;
using Kukuri.GameSdk;
using Kukuri.GameSdk.Lobby;
using Newtonsoft.Json.Linq;
using Unity.Netcode;
using UnityEngine;

public sealed class ArenaProbe : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;
    [SerializeField] KukuriLobby lobby;
    [SerializeField] ArenaSession session;
    [SerializeField] NetworkManager network;

    string role;
    int throws;
    float heldSince = -1;
    bool catching;

    static string Arg(string name)
    {
        var args = Environment.GetCommandLineArgs();
        var index = Array.IndexOf(args, name);
        return index >= 0 && index + 1 < args.Length ? args[index + 1] : null;
    }

    void Awake()
    {
        role = Arg("-arena-role");
        if (role == null) return;
        runner.Settings = KukuriSettings.Create(KukuriMode.Ffi, role, Arg("-arena-profile-dir"));
        runner.enabled = true;
        session.InputSource = Bot;
        session.Confirmed += @event => Debug.Log($"KGS_EVENT {@event.Sequence} {@event.Kind} {@event.Phase} {@event.Player} {@event.Target} {@event.Score0} {@event.Score1}");
    }

    async void Start()
    {
        if (role == null) return;
        try
        {
            await (role == "host" ? Host() : Client());
            Application.Quit(0);
        }
        catch (Exception error)
        {
            Debug.Log("KGS_FAILED " + error);
            Application.Quit(1);
        }
    }

    /// 相手へ向き、左右に動きながら、持てば少し待って投げる。ディスクが向かってくれば、1 回おきに受け止める。
    ArenaInput Bot(int local)
    {
        var t = Time.time;
        var input = new ArenaInput { Yaw = ArenaControls.FacingYaw(session), Move = new Vector2(Mathf.Sin(t * (local == 0 ? 1.3f : 1.7f)), 0) };
        if (session.Phase != ArenaPhase.Playing) return input;
        if (session.Holder == local)
        {
            catching = false;
            if (heldSince < 0) heldSince = t;
            if (t - heldSince > 0.6f)
            {
                heldSince = -1;
                throws++;
                session.Act();
            }
            return input;
        }
        heldSince = -1;
        var me = session.Players[local].Position;
        var disc = session.Disc;
        var coming = disc.Mode == DiscMode.Flying && Vector3.Dot(disc.Velocity, me - disc.Position) > 0;
        var distance = Vector2.Distance(new Vector2(me.x, me.z), new Vector2(disc.Position.x, disc.Position.z));
        if (coming && distance < 3 && !catching)
        {
            catching = true;
            if ((throws + local) % 2 == 0) session.Act();
        }
        if (!coming) catching = false;
        return input;
    }

    static async Task Until(Func<bool> condition, double seconds, string what)
    {
        var deadline = Time.realtimeSinceStartup + seconds;
        while (!condition())
        {
            if (Time.realtimeSinceStartup > deadline) throw new TimeoutException(what);
            await Task.Yield();
        }
    }

    async Task<JToken> Call(string command, JObject payload = null)
    {
        var response = await runner.Client.SendAsync(command, payload);
        if (!response.Ok) throw new Exception(command + ": " + response.ErrorCode);
        return response.Data;
    }

    async Task Prepare()
    {
        if (runner.Client == null) throw new Exception("the SDK did not open: " + runner.OpenError?.Code);
        var startup = await Call("get_desktop_startup_status");
        if ((string)startup["status"] == "consent_required")
        {
            var consent = await Call("get_app_consent_status");
            var documents = new JArray(((JArray)consent["documents"]).Select(document =>
                new JObject { ["slug"] = document["slug"], ["version"] = document["currentVersion"] }));
            await Call("accept_app_consents", new JObject { ["documents"] = documents, ["language"] = "ja", ["age_attested"] = true });
        }
        lobby.Topic = Arg("-arena-topic");
        var exchange = Arg("-arena-exchange");
        var other = role == "host" ? "client" : "host";
        var mine = Path.Combine(exchange, role + ".ticket");
        File.WriteAllText(mine + ".tmp", (string)await Call("get_local_peer_ticket"));
        File.Move(mine + ".tmp", mine);
        var path = Path.Combine(exchange, other + ".ticket");
        await Until(() => File.Exists(path), 60, "no ticket from the " + other);
        await Call("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(path) });
    }

    async Task Pause(float seconds)
    {
        var until = Time.realtimeSinceStartup + seconds;
        await Until(() => Time.realtimeSinceStartup >= until, seconds + 1, "pause");
    }

    async Task Shot(string name)
    {
        await Task.Yield();
        ScreenCapture.CaptureScreenshot(Path.Combine(Arg("-arena-exchange"), $"{role}-{name}.png"));
        await Pause(0.5f);
    }

    void Final()
    {
        var players = session.Players;
        Debug.Log($"KGS_FINAL {session.Phase} {session.Winner} {session.Scores[0]} {session.Scores[1]} {session.LastEvent} {session.EventGaps} " +
            string.Join(" ", players.Select(player => $"{player.Position.x:F3},{player.Position.y:F3},{player.Position.z:F3}")));
    }

    async Task Host()
    {
        await Prepare();
        await lobby.CreateRoomAsync("test arena 1");
        await Until(() => session.Phase == ArenaPhase.Playing, 120, "the match did not start");
        await Pause(4);
        await Shot("1-playing");
        await Until(() => session.Phase == ArenaPhase.Result, 240, "the match did not end");
        await Pause(1.5f);
        Final();
        await Shot("2-result");
        File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "host-result"), "done");
        await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "client-result")), 60, "the client did not see the result");
        // 「ロビーへ戻る」: host は room を終える。
        session.Leave();
        await Until(() => !network.IsListening, 30, "the host NGO did not stop");
        Debug.Log($"KGS_HOST_LEFT listening={network.IsListening} in_match={session.InMatch}");

        // 2 試合目: client が途中で退出する。
        await lobby.CreateRoomAsync("test arena 2");
        await Until(() => session.Phase == ArenaPhase.Playing, 120, "the second match did not start");
        await Until(() => session.Phase == ArenaPhase.Left, 120, "the host did not see the client leave");
        Debug.Log($"KGS_HOST_SAW_LEAVE {session.Phase}");
        await Shot("3-opponent-left");
        session.Leave();
        await Until(() => !network.IsListening, 30, "the host NGO did not stop");
        File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "host-done"), "done");
    }

    async Task Client()
    {
        await Prepare();
        await Until(() => lobby.Rooms.Any(room => room.Title == "test arena 1"), 120, "the room did not arrive");
        await lobby.JoinRoomAsync(lobby.Rooms.First(room => room.Title == "test arena 1").RoomId);
        await Until(() => session.Phase == ArenaPhase.Playing, 120, "the match did not start");
        await Pause(4);
        await Shot("1-playing");
        await Until(() => session.Phase == ArenaPhase.Result, 240, "the match did not end");
        await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "host-result")), 60, "the host did not see the result");
        Final();
        await Shot("2-result");
        File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "client-result"), "done");
        await Until(() => !session.InMatch, 60, "the client did not return to the lobby");
        await Pause(1);
        Debug.Log($"KGS_CLIENT_CLOSED listening={network.IsListening} joined={lobby.JoinedSessionId ?? "none"} notice={session.Notice}");
        await Shot("4-host-closed");

        await Until(() => lobby.Rooms.Any(room => room.Title == "test arena 2" && room.Status != "Ended"), 120, "the second room did not arrive");
        await lobby.JoinRoomAsync(lobby.Rooms.First(room => room.Title == "test arena 2" && room.Status != "Ended").RoomId);
        await Until(() => session.Phase == ArenaPhase.Playing, 120, "the second match did not start");
        await Pause(2);
        session.Leave();
        await Until(() => !network.IsListening, 30, "the client NGO did not stop");
        await Pause(1);
        Debug.Log($"KGS_CLIENT_LEFT listening={network.IsListening} joined={lobby.JoinedSessionId ?? "none"}");
        await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "host-done")), 60, "the host did not finish");
    }

    void Update()
    {
        // 部屋の一覧は、待っている部屋が来るまで短い間隔で読み直す。
        if (role == "client" && runner.Client != null && !session.InMatch && Time.frameCount % 30 == 0)
            _ = lobby.RefreshAsync();
    }
}
'@ | Set-Content (Join-Path $probeDir "ArenaProbe.cs") -Encoding utf8
(@'
using Kukuri.GameSdk;
using Kukuri.GameSdk.Lobby;
using CyberDiscArena;
using Unity.Netcode;
using UnityEditor;
using UnityEditor.Build;
using UnityEditor.SceneManagement;
using UnityEngine;
public static class ArenaProbeBuild
{
    static void Link(Object component, string field, Object value)
    {
        var serialized = new SerializedObject(component);
        serialized.FindProperty(field).objectReferenceValue = value;
        serialized.ApplyModifiedPropertiesWithoutUndo();
    }

    public static void Build()
    {
        var scene = EditorSceneManager.OpenScene("Assets/CyberDiscArena/Scenes/Arena.unity");
        var runner = Object.FindAnyObjectByType<KukuriRunner>();
        runner.enabled = false;
        var probe = new GameObject("probe").AddComponent<ArenaProbe>();
        Link(probe, "runner", runner);
        Link(probe, "lobby", Object.FindAnyObjectByType<KukuriLobby>());
        Link(probe, "session", Object.FindAnyObjectByType<ArenaSession>());
        Link(probe, "network", Object.FindAnyObjectByType<NetworkManager>());
        EditorSceneManager.SaveScene(scene, "Assets/ArenaProbe/ArenaTest.unity");
        PlayerSettings.SetScriptingBackend(NamedBuildTarget.Standalone, ScriptingImplementation.__BACKEND__);
        var report = BuildPipeline.BuildPlayer(new BuildPlayerOptions
        {
            scenes = new[] { "Assets/ArenaProbe/ArenaTest.unity" },
            locationPathName = "Builds/__BACKEND__/CyberDiscArena.exe",
            target = BuildTarget.StandaloneWindows64,
        });
        Debug.Log("KGS_BUILD " + report.summary.result);
        EditorApplication.Exit(report.summary.result == UnityEditor.Build.Reporting.BuildResult.Succeeded ? 0 : 1);
    }
}
'@ -replace "__BACKEND__", $Backend) | Set-Content (Join-Path $probeDir "Editor\ArenaProbeBuild.cs") -Encoding utf8
& $Unity -batchmode -nographics -quit -projectPath $ProjectDir -executeMethod ArenaProbeBuild.Build -logFile (Join-Path $ProjectDir "build.log") | Out-Null
$exe = Join-Path $ProjectDir "Builds\$Backend\CyberDiscArena.exe"
if (-not (Test-Path $exe)) { Write-Output "Player (arena, $Backend): built=False; see $ProjectDir\build.log"; exit 1 }
Write-Output "Player (arena, $Backend): built=True"

$work = Join-Path $ProjectDir "runs"
New-Item -ItemType Directory -Force $work | Out-Null
$topic = "kukuri:topic:test-arena-" + [guid]::NewGuid().ToString("N").Substring(0, 8)
$players = @{}
foreach ($role in "host", "client") {
    $log = Join-Path $work "$role.log"
    $arguments = "-screen-fullscreen 0 -screen-width 1280 -screen-height 720 -arena-role $role -arena-profile-dir `"$(Join-Path $work $role)`" " +
        "-arena-topic $topic -arena-exchange `"$work`" -arena-delay $DelayMs -arena-loss $LossPercent -logFile `"$log`""
    $players[$role] = @{ Process = (Start-Process -FilePath $exe -ArgumentList $arguments -PassThru); Log = $log }
}
$exits = @{}
foreach ($role in "host", "client") {
    $process = $players[$role].Process
    if (-not $process.WaitForExit(600000)) { Stop-Process -Id $process.Id -Force; $exits[$role] = "timeout" } else { $exits[$role] = $process.ExitCode }
}

function Lines([string]$role, [string]$pattern) {
    @(Select-String -Path $players[$role].Log -Pattern $pattern | ForEach-Object { $_.Matches[0].Groups[1].Value })
}
# 1 試合目の出来事（2 試合目の分は除く）。
$hostEvents = Lines "host" "KGS_EVENT (.*)"
$clientEvents = Lines "client" "KGS_EVENT (.*)"
$firstEnd = if ($hostEvents.Count -eq 0) { -1 } else { [array]::FindIndex([string[]]$hostEvents, [Predicate[string]] { param($line) $line -match " Phase Result " }) }
$hostFirst = if ($firstEnd -ge 0) { $hostEvents[0..$firstEnd] } else { @() }
$clientFirst = if ($firstEnd -ge 0 -and $clientEvents.Count -gt $firstEnd) { $clientEvents[0..$firstEnd] } else { @() }
$sameEvents = ($hostFirst.Count -gt 0) -and (($hostFirst -join "|") -eq ($clientFirst -join "|"))
$kinds = ($hostFirst | ForEach-Object { ($_ -split " ")[1] }) | Sort-Object -Unique
$hostFinal = (Lines "host" "KGS_FINAL (.*)") | Select-Object -First 1
$clientFinal = (Lines "client" "KGS_FINAL (.*)") | Select-Object -First 1
$hostParts = "$hostFinal" -split " "
$clientParts = "$clientFinal" -split " "
$sameResult = ($hostParts.Count -ge 8) -and ($hostParts[0] -eq "Result") -and (($hostParts[0..4] -join " ") -eq ($clientParts[0..4] -join " ")) -and ($clientParts[5] -eq "0")
$positionsMatch = $false
if ($sameResult) {
    $positionsMatch = $true
    foreach ($index in 6, 7) {
        $a = $hostParts[$index] -split "," | ForEach-Object { [double]$_ }
        $b = $clientParts[$index] -split "," | ForEach-Object { [double]$_ }
        for ($axis = 0; $axis -lt 3; $axis++) { if ([math]::Abs($a[$axis] - $b[$axis]) -gt 0.05) { $positionsMatch = $false } }
    }
}
$hostLeft = (Lines "host" "KGS_HOST_LEFT (.*)") | Select-Object -First 1
$clientClosed = (Lines "client" "KGS_CLIENT_CLOSED (.*)") | Select-Object -First 1
$hostSawLeave = (Lines "host" "KGS_HOST_SAW_LEAVE (.*)") | Select-Object -First 1
$clientLeft = (Lines "client" "KGS_CLIENT_LEFT (.*)") | Select-Object -First 1
$failure = Select-String -Path $players["host"].Log, $players["client"].Log -Pattern "KGS_FAILED.*" | Select-Object -First 1
$ok = $sameEvents -and $sameResult -and $positionsMatch -and ("Throw" -in $kinds) -and ("Catch" -in $kinds) -and ("Hit" -in $kinds) -and
    ($hostLeft -eq "listening=False in_match=False") -and ($clientClosed -eq "listening=False joined=none notice=ホストが部屋を閉じました") -and
    ($hostSawLeave -eq "Left") -and ($clientLeft -eq "listening=False joined=none") -and ($exits["host"] -eq 0) -and ($exits["client"] -eq 0)
Write-Output "Player match ($Backend, delay=${DelayMs}ms loss=$LossPercent%): events=$($hostFirst.Count) same=$sameEvents kinds=$($kinds -join ',') host=[$hostFinal] client=[$clientFinal] positions=$positionsMatch"
Write-Output "Player leave ($Backend): host_left=[$hostLeft] client_closed=[$clientClosed] host_saw_leave=[$hostSawLeave] client_left=[$clientLeft] exits=$($exits['host']),$($exits['client']) ok=$ok"
if ($failure) { Write-Output "  $($failure.Line)" }
if ($failed -or -not $ok) { exit 1 }
