# 部分導入の確認（#1984 AC-3）。NGO の標準の Unity Transport で 2 peer が同期する小さなゲームへ、ゲームの通信を変えずに
# 掲示板だけを足す。
# 1. 空の project に NGO を入れ、Unity Transport で cube の位置を同期するゲームを作り、2 つの Player で確かめる。
# 2. 手順書（docs/runbooks/game-sdk-quickstart.md の「自分のゲームに掲示板だけを足す」）のとおりに、manifest に配布物の .tgz を
#    1 行足し、設定 asset・KukuriRunner・開始の画面・掲示板の prefab・掲示板を開く script を足して build する。ゲームの script は
#    変えない（SHA-256 で確かめる）。
# 3. 2 つの Player で、ゲームの同期を Unity Transport のまま続けながら、掲示板の投稿が相手に届くことを確かめる。host は設定 asset の
#    既定の profile で開く。同じ PC で同じ profile を 2 つの process が使えないので、client だけ試験の script が profile の dir を替える。
# 足した manifest の行、scene の object、段ごとの時間を出力する。
param(
    [Parameter(Mandatory = $true)][string]$Tarball,
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f2\Editor\Unity.exe",
    [string]$ProjectDir = (Join-Path $env:TEMP "kgs-partial"),
    [ValidateSet("Mono2x", "IL2CPP")] [string]$Backend = "Mono2x"
)
$ErrorActionPreference = "Stop"
$env:KUKURI_DISABLE_KEYRING = "1"
$env:KUKURI_DISCOVERY_MODE = "static_peer"
$env:KUKURI_BIND_ADDR = "127.0.0.1:0"
$env:KUKURI_ADVERTISE_HOST = "127.0.0.1"
foreach ($name in "KUKURI_APP_DATA_DIR", "KUKURI_INSTANCE", "KUKURI_DISCOVERY_SEEDS", "KUKURI_ADVERTISE_PORT") { Remove-Item "env:$name" -ErrorAction SilentlyContinue }

function Add-Package([string]$name, [string]$value) {
    $path = Join-Path $ProjectDir "Packages\manifest.json"
    $manifest = Get-Content $path -Raw | ConvertFrom-Json
    $manifest.dependencies | Add-Member -NotePropertyName $name -NotePropertyValue $value -Force
    # Unity の Package Manager は BOM のある JSON を読めない。
    [IO.File]::WriteAllText($path, ($manifest | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding $false))
}

function Invoke-Build([string]$method, [string]$log, [string]$exe) {
    & $Unity -batchmode -nographics -quit -projectPath $ProjectDir -executeMethod $method -logFile (Join-Path $ProjectDir $log) | Out-Null
    if (-not (Test-Path $exe)) { throw "the Player was not built; see $ProjectDir\$log" }
}

# host と client の Player を起動して終わりを待つ。role ごとの log の path を返す。
function Invoke-Players([string]$name, [string]$exe, [int]$port) {
    $work = Join-Path $ProjectDir "runs\$name"
    New-Item -ItemType Directory -Force $work | Out-Null
    $topic = "kukuri:topic:test-partial-" + [guid]::NewGuid().ToString("N").Substring(0, 8)
    $players = @{}
    foreach ($role in "host", "client") {
        $log = Join-Path $work "$role.log"
        $arguments = "-batchmode -nographics -partial-role $role -partial-port $port -partial-topic $topic -partial-exchange `"$work`" -partial-profile-dir `"$(Join-Path $work $role)`" -logFile `"$log`""
        $players[$role] = @{ Process = (Start-Process -FilePath $exe -ArgumentList $arguments -PassThru); Log = $log }
    }
    foreach ($role in "host", "client") {
        $process = $players[$role].Process
        if (-not $process.WaitForExit(300000)) { Stop-Process -Id $process.Id -Force; throw "the $role of $name did not finish; see $work" }
        if ($process.ExitCode -ne 0) { throw "the $role of $name failed; see $work" }
    }
    return $work
}

function Get-Marker([string]$log, [string]$pattern) {
    Select-String -Path $log -Pattern $pattern | Select-Object -First 1 | ForEach-Object { $_.Matches[0].Groups[1].Value }
}

$clock = [Diagnostics.Stopwatch]::StartNew()
function Write-Step([string]$name) {
    Write-Output ("{0}: {1:n0} s" -f $name, $clock.Elapsed.TotalSeconds)
    $clock.Restart()
}

# 1. Unity Transport で同期するゲーム。
if (Test-Path $ProjectDir) { Remove-Item -Recurse -Force $ProjectDir }
& $Unity -batchmode -nographics -quit -createProject $ProjectDir -logFile (Join-Path $env:TEMP "kgs-partial-create.log") | Out-Null
Add-Package "com.unity.netcode.gameobjects" "2.13.3"
@'
using System;
using Unity.Collections;
using Unity.Netcode;
using Unity.Netcode.Transports.UTP;
using UnityEngine;
/// host が cube の位置を毎 frame 送り、client が受ける。通信は NGO の標準の Unity Transport だけを使う。
public sealed class PartialGame : MonoBehaviour
{
    public const string Message = "cube";
    [SerializeField] NetworkManager network;
    [SerializeField] Transform cube;
    public int Received { get; private set; }
    public static string Arg(string name)
    {
        var args = Environment.GetCommandLineArgs();
        var index = Array.IndexOf(args, name);
        return index >= 0 && index + 1 < args.Length ? args[index + 1] : null;
    }
    void Start()
    {
        var role = Arg("-partial-role");
        if (role == null) return;
        ((UnityTransport)network.NetworkConfig.NetworkTransport).SetConnectionData("127.0.0.1", ushort.Parse(Arg("-partial-port")));
        if (role == "host") network.StartHost();
        else
        {
            network.StartClient();
            network.OnClientConnectedCallback += _ => network.CustomMessagingManager.RegisterNamedMessageHandler(Message, (_, reader) =>
            {
                reader.ReadValueSafe(out Vector3 position);
                cube.position = position;
                Received++;
            });
        }
    }
    void Update()
    {
        if (!network.IsHost || network.ConnectedClientsIds.Count < 2) return;
        cube.position = new Vector3(Mathf.Sin(Time.time), 0, 0);
        using var writer = new FastBufferWriter(32, Allocator.Temp);
        writer.WriteValueSafe(cube.position);
        network.CustomMessagingManager.SendNamedMessageToAll(Message, writer, NetworkDelivery.Unreliable);
    }
}
'@ | Set-Content (Join-Path $ProjectDir "Assets\PartialGame.cs") -Encoding utf8
@'
using System;
using System.IO;
using System.Threading.Tasks;
using Unity.Netcode;
using UnityEngine;
/// ゲームだけの確認: client が位置を受けたら印を書き、host はその印で終わる。
public sealed class PartialGameCheck : MonoBehaviour
{
    [SerializeField] PartialGame game;
    async void Start()
    {
        var role = PartialGame.Arg("-partial-role");
        if (role == null) return;
        var done = Path.Combine(PartialGame.Arg("-partial-exchange"), "synced");
        var deadline = Time.realtimeSinceStartup + 120;
        while (role == "host" ? !File.Exists(done) : game.Received < 30)
        {
            if (Time.realtimeSinceStartup > deadline) { Debug.Log("KGS_FAILED no sync"); Application.Quit(1); return; }
            await Task.Yield();
        }
        if (role == "client") { Debug.Log("KGS_SYNCED " + game.Received); File.WriteAllText(done, "done"); await Task.Delay(1000); }
        Application.Quit(0);
    }
}
'@ | Set-Content (Join-Path $ProjectDir "Assets\PartialGameCheck.cs") -Encoding utf8
$builder = Join-Path $ProjectDir "Assets\Editor"
New-Item -ItemType Directory -Force $builder | Out-Null
(@'
using Unity.Netcode;
using Unity.Netcode.Transports.UTP;
using UnityEditor;
using UnityEditor.Build;
using UnityEditor.SceneManagement;
using UnityEngine;
public static class PartialGameBuild
{
    public static void Link(Object component, string field, Object value)
    {
        var serialized = new SerializedObject(component);
        serialized.FindProperty(field).objectReferenceValue = value;
        serialized.ApplyModifiedPropertiesWithoutUndo();
    }
    public static void Finish(string name)
    {
        PlayerSettings.runInBackground = true;
        PlayerSettings.SetScriptingBackend(NamedBuildTarget.Standalone, ScriptingImplementation.__BACKEND__);
        var report = BuildPipeline.BuildPlayer(new BuildPlayerOptions
        {
            scenes = new[] { "Assets/Game.unity" },
            locationPathName = "Builds/" + name + "/Game.exe",
            target = BuildTarget.StandaloneWindows64,
        });
        EditorApplication.Exit(report.summary.result == UnityEditor.Build.Reporting.BuildResult.Succeeded ? 0 : 1);
    }
}
'@ -replace "__BACKEND__", $Backend) | Set-Content (Join-Path $builder "PartialGameBuild.cs") -Encoding utf8
@'
using Unity.Netcode;
using Unity.Netcode.Transports.UTP;
using UnityEditor.SceneManagement;
using UnityEngine;
public static class PartialGameScene
{
    public static void Build()
    {
        var scene = EditorSceneManager.NewScene(NewSceneSetup.DefaultGameObjects, NewSceneMode.Single);
        var root = new GameObject("game");
        var network = root.AddComponent<NetworkManager>();
        var transport = root.AddComponent<UnityTransport>();
        network.NetworkConfig = new NetworkConfig { NetworkTransport = transport };
        var game = root.AddComponent<PartialGame>();
        PartialGameBuild.Link(game, "network", network);
        PartialGameBuild.Link(game, "cube", GameObject.CreatePrimitive(PrimitiveType.Cube).transform);
        PartialGameBuild.Link(root.AddComponent<PartialGameCheck>(), "game", game);
        EditorSceneManager.SaveScene(scene, "Assets/Game.unity");
        PartialGameBuild.Finish("Game");
    }
}
'@ | Set-Content (Join-Path $builder "PartialGameScene.cs") -Encoding utf8
$gameExe = Join-Path $ProjectDir "Builds\Game\Game.exe"
Invoke-Build "PartialGameScene.Build" "build-game.log" $gameExe
Write-Step "the game without the SDK: created and built"
$work = Invoke-Players "game" $gameExe 17801
$synced = Get-Marker (Join-Path $work "client.log") "KGS_SYNCED (\d+)"
if (-not $synced) { throw "the game did not sync" }
Write-Output "the game alone: synced positions=$synced over Unity Transport"
Write-Step "the game without the SDK: two Players"
$gameScript = Join-Path $ProjectDir "Assets\PartialGame.cs"
$gameHash = (Get-FileHash $gameScript).Hash

# 2. 配布物の .tgz を足す。ゲームの script は変えず、確認の script だけを置き換える。
$before = Get-Content (Join-Path $ProjectDir "Packages\manifest.json") -Raw | ConvertFrom-Json
Add-Package "com.kukuri.gamesdk" ("file:" + ($Tarball -replace '\\', '/'))
$after = Get-Content (Join-Path $ProjectDir "Packages\manifest.json") -Raw | ConvertFrom-Json
$added = @($after.dependencies.PSObject.Properties.Name | Where-Object { $before.dependencies.PSObject.Properties.Name -notcontains $_ })
foreach ($file in "Assets\PartialGameCheck.cs", "Assets\Editor\PartialGameScene.cs") { Remove-Item (Join-Path $ProjectDir $file), (Join-Path $ProjectDir "$file.meta") }
@'
using System;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Kukuri.GameSdk;
using Kukuri.GameSdk.Social;
using Newtonsoft.Json.Linq;
using Unity.Netcode;
using Unity.Netcode.Transports.UTP;
using UnityEngine;
/// 掲示板を足した確認: ゲームの同期を続けたまま、host の投稿が client の掲示板に届く。KukuriRunner より先に動かす。
[DefaultExecutionOrder(-1000)]
public sealed class PartialBoardCheck : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;
    [SerializeField] KukuriAccountScreen account;
    [SerializeField] KukuriBoard board;
    [SerializeField] PartialGame game;
    [SerializeField] NetworkManager network;
    string exchange;
    void Awake()
    {
        var role = PartialGame.Arg("-partial-role");
        if (role == null) return;
        board.Topic = PartialGame.Arg("-partial-topic");
        if (role == "client") runner.Settings = KukuriSettings.Create(KukuriMode.Ffi, "partial", PartialGame.Arg("-partial-profile-dir"));
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
    async void Start()
    {
        var role = PartialGame.Arg("-partial-role");
        if (role == null) return;
        try
        {
            exchange = PartialGame.Arg("-partial-exchange");
            await Until(() => account.Screen == "consent", 60, "the consent screen did not open");
            await account.Accept();
            await Until(() => account.Screen == "start", 60, "the start screen did not open");
            await account.Begin();
            await Until(() => account.Screen == "playing", 60, "the game did not start");
            var other = role == "host" ? "client" : "host";
            var mine = Path.Combine(exchange, role + ".ticket");
            File.WriteAllText(mine + ".tmp", (string)(await runner.Client.SendAsync("get_local_peer_ticket")).Data);
            File.Move(mine + ".tmp", mine);
            var theirs = Path.Combine(exchange, other + ".ticket");
            await Until(() => File.Exists(theirs), 60, "no ticket from the " + other);
            await runner.Client.SendAsync("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(theirs) });
            await Until(() => board.Subscribed, 60, "the board did not subscribe");
            Debug.Log("KGS_TRANSPORT " + network.NetworkConfig.NetworkTransport.GetType().Name);
            var postFile = Path.Combine(exchange, "post");
            var done = Path.Combine(exchange, "seen");
            if (role == "host")
            {
                await Until(() => network.ConnectedClientsIds.Count >= 2, 60, "the client did not connect over Unity Transport");
                var post = await board.PostAsync("test: from the partial host");
                File.WriteAllText(postFile + ".tmp", post);
                File.Move(postFile + ".tmp", postFile);
                await Until(() => File.Exists(done), 180, "the client did not see the post");
            }
            else
            {
                await Until(() => game.Received >= 30, 60, "the game did not sync");
                await Until(() => File.Exists(postFile), 120, "the host did not post");
                var post = File.ReadAllText(postFile);
                await Until(() => board.Posts.Any(item => item.ObjectId == post), 120, "the post did not reach the client");
                var received = game.Received;
                await Until(() => game.Received >= received + 30, 60, "the game stopped syncing");
                Debug.Log($"KGS_BOARD_SEEN {post} {game.Received}");
                File.WriteAllText(done, "done");
                await Task.Delay(1000);
            }
            Application.Quit(0);
        }
        catch (Exception error)
        {
            Debug.Log("KGS_FAILED " + error);
            Application.Quit(1);
        }
    }
}
'@ | Set-Content (Join-Path $ProjectDir "Assets\PartialBoardCheck.cs") -Encoding utf8
# 手順書の script と同じ。
@'
using Kukuri.GameSdk;
using Kukuri.GameSdk.Social;
using UnityEngine;
public sealed class BoardOpener : MonoBehaviour
{
    [SerializeField] KukuriAccountScreen account;
    [SerializeField] KukuriBoard board;
    void Update()
    {
        if (account.Screen != "playing" || board.gameObject.activeSelf) return;
        account.Visible = false;
        board.gameObject.SetActive(true);
    }
}
'@ | Set-Content (Join-Path $ProjectDir "Assets\BoardOpener.cs") -Encoding utf8
@'
using Kukuri.GameSdk;
using Kukuri.GameSdk.Social;
using Unity.Netcode;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
public static class PartialBoardBuild
{
    public static void Build()
    {
        var scene = EditorSceneManager.OpenScene("Assets/Game.unity");
        GameObjectUtility.RemoveMonoBehavioursWithMissingScript(Object.FindAnyObjectByType<PartialGame>().gameObject);
        var host = new GameObject("kukuri");
        var settings = ScriptableObject.CreateInstance<KukuriSettings>();
        AssetDatabase.CreateAsset(settings, "Assets/KukuriSettings.asset");
        var runner = host.AddComponent<KukuriRunner>();
        PartialGameBuild.Link(runner, "settings", settings);
        var account = host.AddComponent<KukuriAccountScreen>();
        PartialGameBuild.Link(account, "runner", runner);
        var boardObject = (GameObject)PrefabUtility.InstantiatePrefab(
            AssetDatabase.LoadAssetAtPath<GameObject>("Packages/com.kukuri.gamesdk/Runtime/Social/KukuriBoard.prefab"));
        var board = boardObject.GetComponent<KukuriBoard>();
        PartialGameBuild.Link(board, "runner", runner);
        boardObject.SetActive(false);
        var opener = host.AddComponent<BoardOpener>();
        PartialGameBuild.Link(opener, "account", account);
        PartialGameBuild.Link(opener, "board", board);
        var check = host.AddComponent<PartialBoardCheck>();
        PartialGameBuild.Link(check, "runner", runner);
        PartialGameBuild.Link(check, "account", account);
        PartialGameBuild.Link(check, "board", board);
        PartialGameBuild.Link(check, "game", Object.FindAnyObjectByType<PartialGame>());
        PartialGameBuild.Link(check, "network", Object.FindAnyObjectByType<NetworkManager>());
        EditorSceneManager.SaveScene(scene);
        PartialGameBuild.Finish("Board");
    }
}
'@ | Set-Content (Join-Path $builder "PartialBoardBuild.cs") -Encoding utf8
$boardExe = Join-Path $ProjectDir "Builds\Board\Game.exe"
Invoke-Build "PartialBoardBuild.Build" "build-board.log" $boardExe
Write-Step "the board added: imported and built"

# 3. Unity Transport のままで、掲示板が動く。host は設定 asset の既定の profile で開く。
$productData = Join-Path $env:USERPROFILE "AppData\LocalLow\DefaultCompany\$(Split-Path $ProjectDir -Leaf)"
if (Test-Path $productData) { Remove-Item -Recurse -Force $productData }
$work = Invoke-Players "board" $boardExe 17802
$defaultProfile = Test-Path (Join-Path $productData "kukuri\profiles\default")
Remove-Item -Recurse -Force $productData -ErrorAction SilentlyContinue
$seen = Get-Marker (Join-Path $work "client.log") "KGS_BOARD_SEEN (\S+ \d+)"
$transports = foreach ($role in "host", "client") { Get-Marker (Join-Path $work "$role.log") "KGS_TRANSPORT (\S+)" }
Write-Step "the board added: two Players"
$unchanged = (Get-FileHash $gameScript).Hash -eq $gameHash
Write-Output "packages added: $($added -join ', ')"
Write-Output "added: Assets/KukuriSettings.asset, Assets/BoardOpener.cs, scene objects kukuri (KukuriRunner, KukuriAccountScreen, BoardOpener), KukuriBoard"
Write-Output "the host opened the default profile of the settings asset: $defaultProfile"
Write-Output "the game script unchanged: $unchanged"
Write-Output "transports: $($transports -join ',')"
Write-Output "the board post seen by the client while the game synced: $seen"
if (-not $seen -or -not $unchanged -or -not $defaultProfile -or ($added -join ",") -ne "com.kukuri.gamesdk" -or
    @($transports | Where-Object { $_ -eq "UnityTransport" }).Count -ne 2) { exit 1 }
