# Netcode for GameObjects 2.13.3 を入れた Unity project で、Game SDK の NGO の Transport を確かめる（#1979 AC-2）。
# - EditMode: Transport の契約（connect・send・poll・disconnect と各配送）
# - 独立した 2 つの Windows Player（host と client）が Netcode Sync sample で cube の位置を同期し、client が切ると host に
#   切断が届く。client は FFI と Sidecar の 2 回
# 先に build-native.ps1 で native を置く。NGO を入れない build は test.ps1 が確かめる。
param(
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Unity.exe",
    [string]$ProjectDir = (Join-Path $env:TEMP "kgs-ngo"),
    [ValidateSet("Mono2x", "IL2CPP")] [string]$Backend = "Mono2x"
)
$ErrorActionPreference = "Stop"
$package = (Resolve-Path (Join-Path $PSScriptRoot "com.kukuri.gamesdk")).Path
if (-not (Test-Path (Join-Path $package "Runtime\Plugins\x86_64\kukuri_game_sdk.dll"))) { throw "run build-native.ps1 first" }

if (Test-Path $ProjectDir) { Remove-Item -Recurse -Force $ProjectDir }
& $Unity -batchmode -nographics -quit -createProject $ProjectDir -logFile (Join-Path $env:TEMP "kgs-ngo-create.log") | Out-Null
$manifest = Get-Content (Join-Path $ProjectDir "Packages\manifest.json") -Raw | ConvertFrom-Json
$manifest.dependencies | Add-Member -NotePropertyName "com.kukuri.gamesdk" -NotePropertyValue ("file:" + ($package -replace '\\', '/')) -Force
$manifest.dependencies | Add-Member -NotePropertyName "com.unity.netcode.gameobjects" -NotePropertyValue "2.13.3" -Force
$manifest.dependencies | Add-Member -NotePropertyName "com.unity.test-framework" -NotePropertyValue "1.6.0" -Force
$manifest | Add-Member -NotePropertyName "testables" -NotePropertyValue @("com.kukuri.gamesdk") -Force
[IO.File]::WriteAllText((Join-Path $ProjectDir "Packages\manifest.json"), ($manifest | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding $false))
$sample = Join-Path $ProjectDir "Assets\Samples\NetcodeSync"
New-Item -ItemType Directory -Force $sample | Out-Null
Copy-Item (Join-Path $package "Samples~\NetcodeSync\*.cs") $sample

$env:KUKURI_DISABLE_KEYRING = "1"
$env:KUKURI_DISCOVERY_MODE = "static_peer"
$env:KUKURI_BIND_ADDR = "127.0.0.1:0"
$env:KUKURI_ADVERTISE_HOST = "127.0.0.1"
foreach ($name in "KUKURI_APP_DATA_DIR", "KUKURI_INSTANCE", "KUKURI_DISCOVERY_SEEDS", "KUKURI_ADVERTISE_PORT") { Remove-Item "env:$name" -ErrorAction SilentlyContinue }

$results = Join-Path $ProjectDir "editmode-results.xml"
& $Unity -batchmode -nographics -projectPath $ProjectDir -runTests -testPlatform EditMode -assemblyNames "Kukuri.GameSdk.Netcode.Tests" -testResults $results -logFile (Join-Path $ProjectDir "editmode.log") | Out-Null
if (-not (Test-Path $results)) { throw "EditMode tests did not run; see $ProjectDir\editmode.log" }
[xml]$xml = Get-Content $results
$run = $xml.'test-run'
Write-Output "EditMode (NGO): total=$($run.total) passed=$($run.passed) failed=$($run.failed)"
$failed = ($run.failed -ne "0") -or ($run.total -eq "0")

# Player の probe。`-kgs-role host|client` のときだけ動き、結果を log に書いて終わる。
@'
using System;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Kukuri.GameSdk;
using Kukuri.GameSdk.Lobby;
using Newtonsoft.Json.Linq;
using Unity.Netcode;
using UnityEngine;
public sealed class KgsNetcodeProbe : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;
    [SerializeField] KukuriLobby lobby;
    [SerializeField] NetcodeSyncSample sample;
    [SerializeField] NetworkManager network;
    [SerializeField] Transform cube;

    static string Arg(string name)
    {
        var args = Environment.GetCommandLineArgs();
        var index = Array.IndexOf(args, name);
        return index >= 0 && index + 1 < args.Length ? args[index + 1] : null;
    }

    void Awake()
    {
        if (Arg("-kgs-role") == null) return;
        var mode = Environment.GetCommandLineArgs().Contains("-kgs-sidecar") ? KukuriMode.Sidecar : KukuriMode.Ffi;
        runner.Settings = KukuriSettings.Create(mode, Arg("-kgs-profile"), Arg("-kgs-profile-dir"));
        runner.enabled = true;
    }

    async void Start()
    {
        var role = Arg("-kgs-role");
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

    async Task Prepare(string role)
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
        lobby.ApplicationId = "com.kukuri.test.ngo";
        lobby.Topic = Arg("-kgs-topic");
        var exchange = Arg("-kgs-exchange");
        var other = role == "host" ? "client" : "host";
        // 書き終えてから名前を変える（相手が書きかけの file を読まない）。
        var mine = Path.Combine(exchange, role + ".ticket");
        File.WriteAllText(mine + ".tmp", (string)await Call("get_local_peer_ticket"));
        File.Move(mine + ".tmp", mine);
        var path = Path.Combine(exchange, other + ".ticket");
        await Until(() => File.Exists(path), 60, "no ticket from the " + other);
        await Call("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(path) });
    }

    async Task Host()
    {
        await Prepare("host");
        var left = false;
        network.OnClientConnectedCallback += id => { if (id != NetworkManager.ServerClientId) Debug.Log("KGS_CLIENT_CONNECTED"); };
        network.OnClientDisconnectCallback += id => { if (id != NetworkManager.ServerClientId) { Debug.Log("KGS_CLIENT_DISCONNECTED"); left = true; } };
        await lobby.CreateRoomAsync("ngo");
        Debug.Log("KGS_HOST_STARTED " + network.IsHost);
        await Until(() => left, 180, "the client did not leave");
    }

    async Task Client()
    {
        await Prepare("client");
        await Until(() => lobby.Rooms.Count > 0, 120, "the room did not arrive");
        await lobby.JoinRoomAsync(lobby.Rooms[0].RoomId);
        await Until(() => network.IsConnectedClient, 60, "NGO did not connect");
        Debug.Log("KGS_CONNECTED");
        var first = cube.position;
        await Until(() => sample.ReceivedPositions >= 30 && sample.SnapshotReceived && cube.position != first, 60, "the cube did not sync");
        Debug.Log($"KGS_SYNCED positions={sample.ReceivedPositions} snapshot={sample.SnapshotReceived}");
        network.Shutdown();
        await Task.Delay(2000);
    }

    void Update()
    {
        // 部屋の一覧は client が来るまで読み直す（Lobby の自動の読み直しより短い間隔）。
        if (Arg("-kgs-role") == "client" && lobby.Rooms.Count == 0 && runner.Client != null && Time.frameCount % 30 == 0)
            _ = lobby.RefreshAsync();
    }
}
'@ | Set-Content (Join-Path $ProjectDir "Assets\KgsNetcodeProbe.cs") -Encoding utf8

$builder = Join-Path $ProjectDir "Assets\Editor"
New-Item -ItemType Directory -Force $builder | Out-Null
(@'
using Kukuri.GameSdk;
using Kukuri.GameSdk.Lobby;
using Kukuri.GameSdk.Netcode;
using Unity.Netcode;
using UnityEditor;
using UnityEditor.Build;
using UnityEditor.SceneManagement;
using UnityEngine;
public static class KgsNetcodeBuild
{
    static void Link(Object component, string field, Object value)
    {
        var serialized = new SerializedObject(component);
        serialized.FindProperty(field).objectReferenceValue = value;
        serialized.ApplyModifiedPropertiesWithoutUndo();
    }

    public static void Build()
    {
        var scene = EditorSceneManager.NewScene(NewSceneSetup.DefaultGameObjects, NewSceneMode.Single);
        var root = new GameObject("kukuri");
        var runner = root.AddComponent<KukuriRunner>();
        runner.enabled = false;
        var network = root.AddComponent<NetworkManager>();
        var transport = root.AddComponent<KukuriNetcodeTransport>();
        Link(transport, "runner", runner);
        network.NetworkConfig = new NetworkConfig { NetworkTransport = transport };
        var lobbyObject = (GameObject)PrefabUtility.InstantiatePrefab(
            AssetDatabase.LoadAssetAtPath<GameObject>("Packages/com.kukuri.gamesdk/Runtime/Lobby/KukuriLobby.prefab"));
        var lobby = lobbyObject.GetComponent<KukuriLobby>();
        Link(lobby, "runner", runner);
        var cube = GameObject.CreatePrimitive(PrimitiveType.Cube).transform;
        var sample = root.AddComponent<NetcodeSyncSample>();
        Link(sample, "network", network);
        Link(sample, "transport", transport);
        Link(sample, "lobby", lobby);
        Link(sample, "cube", cube);
        var probe = root.AddComponent<KgsNetcodeProbe>();
        Link(probe, "runner", runner);
        Link(probe, "lobby", lobby);
        Link(probe, "sample", sample);
        Link(probe, "network", network);
        Link(probe, "cube", cube);
        EditorSceneManager.SaveScene(scene, "Assets/NetcodeSync.unity");
        PlayerSettings.runInBackground = true;
        PlayerSettings.SetScriptingBackend(NamedBuildTarget.Standalone, ScriptingImplementation.__BACKEND__);
        var report = BuildPipeline.BuildPlayer(new BuildPlayerOptions
        {
            scenes = new[] { "Assets/NetcodeSync.unity" },
            locationPathName = "Builds/__BACKEND__/KgsNgo.exe",
            target = BuildTarget.StandaloneWindows64,
        });
        Debug.Log("KGS_BUILD " + report.summary.result);
        EditorApplication.Exit(report.summary.result == UnityEditor.Build.Reporting.BuildResult.Succeeded ? 0 : 1);
    }
}
'@ -replace "__BACKEND__", $Backend) | Set-Content (Join-Path $builder "KgsNetcodeBuild.cs") -Encoding utf8
& $Unity -batchmode -nographics -quit -projectPath $ProjectDir -executeMethod KgsNetcodeBuild.Build -logFile (Join-Path $ProjectDir "build.log") | Out-Null
$exe = Join-Path $ProjectDir "Builds\$Backend\KgsNgo.exe"
if (-not (Test-Path $exe)) { Write-Output "Player (NGO, $Backend): built=False; see $ProjectDir\build.log"; exit 1 }
Write-Output "Player (NGO, $Backend): built=True"

$sidecarExe = Join-Path $package "Editor\Sidecar~\kukuri-sdk-sidecar.exe"

# host と client の Player を起動し、両方が終わるまで待って log の印を確かめる。`-Sidecar` なら client は Sidecar を使う。
function Test-Sync([string]$name, [bool]$sidecar) {
    $work = Join-Path $ProjectDir "runs\$name"
    New-Item -ItemType Directory -Force $work | Out-Null
    $topic = "kukuri:topic:test-ngo-" + [guid]::NewGuid().ToString("N").Substring(0, 8)
    $sidecarProcess = $null
    $clientDir = Join-Path $work "client"
    if ($sidecar) {
        $sidecarProcess = Start-Process -FilePath $sidecarExe -ArgumentList "run --profile client --profile-dir `"$clientDir`"" -PassThru -RedirectStandardError (Join-Path $work "sidecar.err")
        for ($i = 0; $i -lt 120 -and -not (Select-String -Path (Join-Path $work "sidecar.err") -Pattern "^ready" -Quiet -ErrorAction SilentlyContinue); $i++) { Start-Sleep -Milliseconds 500 }
    }
    $players = @{}
    foreach ($role in "host", "client") {
        $extra = if ($role -eq "client" -and $sidecar) { "-kgs-sidecar" } else { "" }
        $log = Join-Path $work "$role.log"
        $arguments = "-batchmode -nographics -kgs-role $role -kgs-profile $role -kgs-profile-dir `"$(Join-Path $work $role)`" -kgs-topic $topic -kgs-exchange `"$work`" $extra -logFile `"$log`""
        $players[$role] = @{ Process = (Start-Process -FilePath $exe -ArgumentList $arguments -PassThru); Log = $log }
    }
    $exits = @{}
    foreach ($role in "host", "client") {
        $process = $players[$role].Process
        if (-not $process.WaitForExit(300000)) { Stop-Process -Id $process.Id -Force; $exits[$role] = "timeout" } else { $exits[$role] = $process.ExitCode }
    }
    if ($sidecarProcess) {
        Start-Process -FilePath $sidecarExe -ArgumentList "stop --profile client --profile-dir `"$clientDir`"" -Wait | Out-Null
        if (-not $sidecarProcess.WaitForExit(15000)) { Stop-Process -Id $sidecarProcess.Id -Force }
    }
    $synced = Select-String -Path $players["client"].Log -Pattern "KGS_SYNCED positions=(\d+) snapshot=True" | Select-Object -First 1
    $connected = Select-String -Path $players["host"].Log -Pattern "KGS_CLIENT_CONNECTED" -Quiet
    $disconnected = Select-String -Path $players["host"].Log -Pattern "KGS_CLIENT_DISCONNECTED" -Quiet
    $failure = Select-String -Path $players["host"].Log, $players["client"].Log -Pattern "KGS_FAILED.*" | Select-Object -First 1
    $ok = ($null -ne $synced) -and $connected -and $disconnected -and ($exits["host"] -eq 0) -and ($exits["client"] -eq 0)
    $positions = if ($synced) { $synced.Matches[0].Groups[1].Value } else { "0" }
    Write-Host "Player sync ($name): positions=$positions host_saw_connect=$connected host_saw_disconnect=$disconnected exits=$($exits['host']),$($exits['client']) ok=$ok"
    if ($failure) { Write-Host "  $($failure.Line)" }
    return $ok
}

$ffi = Test-Sync "ffi" $false
$viaSidecar = Test-Sync "sidecar" $true
if ($failed -or -not $ffi -or -not $viaSidecar) { exit 1 }
