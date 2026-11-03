# Cyber Disc Arena の対戦 ring とアカウントとスキンを確かめる（#1983 AC-1・AC-2a・AC-2b）。
# - EditMode: host の裁定の規則（当てる・受け止める・跳ねて止まる・5 点・時間切れと延長・自分の側）
# - 独立した 2 つの Windows Player（host と client）が、受信を片道 25 ms 遅らせ datagram を 1% 落とした状態で、入力を
#   流す bot で 1 試合を終える。両方が同じ出来事の列（通し番号・持ち主・得点）と勝敗を確定し、client の番号に飛びが無く、
#   試合の後の 2 人の位置が host と一致する。host が「ロビーへ戻る」と client に「ホストが部屋を閉じました」が出て、両方の
#   NGO が止まり、session から抜ける。続く 2 試合目で client が途中で退出すると、host に「相手が退出しました」が出る。
#   どちらも開始の画面（KukuriAccountScreen）で同意して「はじめる」までロビーを開かない。
# - スキン: 作者（開発用の Sidecar）が対戦の前にスキンを発行すると、host のロッカーに出て、選ぶと host の体に貼られる。
#   client も別のスキンを着る。対戦が始まると互いの体に相手のスキンを P2P で取って貼る。対戦中に作者が新しい版を発行すると、
#   両方の体が再起動せずに新しい版へ変わる。
# - 配信と応援: host は「配信する」を選んで部屋を作り、試合の開始で公開の Live を始める。ロビーにいる観戦の Player（3 人目）
#   はモニターでその配信を受けて映し、R の応援（🔥）を送る。`-DesktopApp` を渡すと、ゲームを入れていない desktop も配信に
#   参加して映像を受け、👏 で応援する。host の ring の上に応援の演出が出て、試合が終わると配信を止める。
# - 掲示板と結果: host は試合の後に「結果を投稿」で得点の文と画面の画像を投稿する。`-DesktopApp` なら、desktop はその投稿を
#   timeline で読み（画像の添付つき）、ゲームが終わった後に返信する。再起動した host が掲示板（Enter）でその thread を開くと、
#   同じ投稿 ID の下に desktop の返信が並ぶ。
# - desktop・Web との相互運用（#1984 AC-1）: `-WebOrigin` に試験の Community Node の fixture（harness の `web_e2e_fixture`）の
#   配信の origin を渡すと、その Web の build を開いた Chrome も desktop と同じく配信を受けて応援（🎉 GG!）し、結果を読んで
#   返信する。host と観戦の Player は fixture の Community Node に同意して Web とつながる（再起動した host は保存した設定で
#   つながる）。host が 2 試合を終えると、desktop と Web はそれぞれ camera で配信し、観戦の Player のモニターがその配信を
#   受けて映す。
# - 4 つの Prefab の最小の scene（土台と、その Prefab だけ）: 観戦の Player は ArenaStreamViewer の scene で観戦する。
#   LiveSkinLoader の scene は作者が発行したスキンを選んで見本の体に着せ、InGameBoardUI の scene は掲示板に投稿して読み、
#   DiscNetworkSync の scene の 2 つの Player は部屋を作って参加し、試合を始めて host の終了を受ける。
# - 状態の継続（1 つの Player を同じ profile で起動し直す）: 「はじめる」で選んだスキンと game player ID が再起動の後も
#   続く。Steam Cloud に保存した file（game-account.json とゲームのセーブ）だけを写した新しい profile でも、同じ ID・公開鍵・
#   スキンで再開する。profile に足したアカウント B に、この端末で遊んだ別の進行があれば、まとめる確定の画面で B の進行を
#   残すことを選べ、まとめた後は B の公開鍵・B の進行の ID とスキンで続き、選ばなかった進行もセーブに残る。
# 先に sdk/unity/build-native.ps1 で native を置く。Unity の project は短い path へ写して使う（深い path は MAX_PATH に当たる）。
param(
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Unity.exe",
    [string]$ProjectDir = (Join-Path $env:TEMP "kcda-test"),
    [ValidateSet("Mono2x", "IL2CPP")] [string]$Backend = "Mono2x",
    [switch]$SkipEditMode,
    [string]$DesktopApp,
    [string]$WebOrigin,
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
    [SerializeField] KukuriAccountScreen account;
    [SerializeField] ArenaProfile profile;
    [SerializeField] ArenaSkins skins;
    [SerializeField] ArenaBroadcast broadcast;
    [SerializeField] ArenaCheer cheer;
    [SerializeField] ArenaView view;
    [SerializeField] ArenaBoard arenaBoard;
    readonly System.Collections.Generic.HashSet<string> cheers = new System.Collections.Generic.HashSet<string>();
    string mutedSession;
    KukuriClient publisher;

    string role;
    int throws;
    float heldSince = -1;
    bool catching;
    bool dodging;
    int dodges;

    static string Arg(string name)
    {
        var args = Environment.GetCommandLineArgs();
        var index = Array.IndexOf(args, name);
        return index >= 0 && index + 1 < args.Length ? args[index + 1] : null;
    }

    /// 最初の空の scene（Boot）から、役の scene（`-arena-scene`、無ければ Arena）へ移る。Arena を経由すると、NGO の
    /// NetworkManager が DontDestroyOnLoad で対戦の Prefab ごと次の scene へ残るので、空の scene から移る。
    static bool Elsewhere() => UnityEngine.SceneManagement.SceneManager.GetActiveScene().name == "Boot";

    void Awake()
    {
        if (Elsewhere())
        {
            UnityEngine.SceneManagement.SceneManager.LoadScene(Arg("-arena-scene") ?? "ArenaTest");
            return;
        }
        role = Arg("-arena-role");
        if (role == null) return;
        runner.Settings = KukuriSettings.Create(KukuriMode.Ffi, role, Arg("-arena-profile-dir"));
        // 試験では音を出さない（ゲームの音。Live の音は視聴を始めたらすぐ音量を 0 にする）。
        AudioListener.volume = 0;
        if (cheer != null) cheer.Spawned += label => cheers.Add(label);
        runner.enabled = true;
        if (session == null) return;
        session.InputSource = Bot;
        session.Confirmed += @event =>
        {
            Debug.Log($"KGS_EVENT {@event.Sequence} {@event.Kind} {@event.Phase} {@event.Player} {@event.Target} {@event.Score0} {@event.Score1}");
            // しゃがんでかわした投げが、当たらずに終わった。
            if (dodging && @event.Kind != ArenaEventKind.Throw)
            {
                if (@event.Kind != ArenaEventKind.Hit) dodges++;
                dodging = false;
            }
        };
    }

    async void Start()
    {
        if (role == null || Elsewhere()) return;
        try
        {
            await (role == "host" ? Host() : role == "client" ? Client() : role == "spectator" ? Spectator() : role == "resume" ? Resume() :
                role == "mini-skin" ? MiniSkin() : role == "mini-board" ? MiniBoard() : role.StartsWith("mini-") ? MiniDisc() : State());
            Application.Quit(0);
        }
        catch (Exception error)
        {
            Debug.Log("KGS_FAILED " + error);
            Application.Quit(1);
        }
    }

    /// 相手へ向き、左右に動きながら、持てば少し待って投げる。ディスクが向かってくれば、3 回に 1 回は受け止め、1 回は
    /// しゃがんでかわし、1 回は当たる。
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
        var choice = (throws + local) % 3;
        if (coming && distance < 3 && !catching)
        {
            catching = true;
            if (choice == 0) session.Act();
            if (choice == 1) dodging = true;
        }
        if (!coming) catching = false;
        input.Crouch = dodging && coming && distance < 5;
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

    /// 開始の画面で同意して「はじめる」（保存済みなら再開を待つ）。ロビーは開始の後に開く。
    async Task StartGame(bool resume)
    {
        if (runner.Client == null) throw new Exception("the SDK did not open: " + runner.OpenError?.Code);
        if (!resume)
        {
            await Consent();
            await account.Begin();
        }
        await Until(() => account.Screen == "playing" && (profile == null || profile.Ready) && (lobby == null || lobby.gameObject.activeInHierarchy),
            60, "the game did not start");
        // Web は Community Node でしかつながらないので、fixture の Community Node の文書に同意する（#1984 AC-1）。
        var node = Arg("-arena-cn");
        if (node != null)
        {
            await Call("set_community_node_config", new JObject { ["nodes"] = new JArray(new JObject { ["base_url"] = node }) });
            var policies = (JArray)(await Call("fetch_community_node_policies", new JObject { ["base_url"] = node, ["language"] = "ja" }))["policies"];
            await Call("accept_community_node_consents", new JObject
            {
                ["base_url"] = node,
                ["language"] = "ja",
                ["documents"] = new JArray(policies.Select(policy => new JObject
                {
                    ["policy_slug"] = policy["policy_slug"],
                    ["policy_version"] = policy["policy_version"],
                    ["policy_snapshot_revision"] = policy["policy_snapshot_revision"],
                })),
            });
        }
    }

    async Task Consent()
    {
        await Until(() => account.Screen == "consent", 60, "the consent screen did not open");
        Debug.Log("KGS_LOBBY_BEFORE_START " + (lobby != null && lobby.gameObject.activeInHierarchy));
        await account.Accept();
        await Until(() => account.Screen == "start", 60, "the start screen did not open");
    }

    async Task Prepare()
    {
        await StartGame(false);
        lobby.Topic = Arg("-arena-topic");
        skins.Topic = Arg("-arena-topic");
        arenaBoard.Board.Topic = Arg("-arena-topic");
        broadcast.Topic = Arg("-arena-topic");
        var exchange = Arg("-arena-exchange");
        var other = role == "host" ? "client" : "host";
        var mine = Path.Combine(exchange, role + ".ticket");
        File.WriteAllText(mine + ".tmp", (string)await Call("get_local_peer_ticket"));
        File.Move(mine + ".tmp", mine);
        var path = Path.Combine(exchange, other + ".ticket");
        await Until(() => File.Exists(path), 60, "no ticket from the " + other);
        await Call("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(path) });
        // host は観戦の Player と desktop の ticket も取り込む（desktop にはゲームの ticket を渡す）。
        if (role == "host")
        {
            if (Arg("-arena-desktop") != null)
                File.WriteAllText(Path.Combine(exchange, "game-ticket"), File.ReadAllText(mine));
            foreach (var peer in new[] { Arg("-arena-spectator") != null ? "spectator.ticket" : null, Arg("-arena-desktop") != null ? "desktop-ticket" : null })
            {
                if (peer == null) continue;
                await Until(() => File.Exists(Path.Combine(exchange, peer)), 120, "no ticket from " + peer);
                await Call("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(Path.Combine(exchange, peer)) });
            }
        }
        // 作者（開発用の Sidecar）は host が開いて、互いの ticket を取り込む。client は host が書いた ticket と公開鍵を読む。
        if (role == "host")
        {
            publisher = KukuriClient.Open(KukuriMode.Sidecar, "publisher", Arg("-arena-publisher-dir"));
            await Send(publisher, "accept_app_consents", new JObject
            {
                ["documents"] = new JArray(((JArray)(await Send(publisher, "get_app_consent_status"))["documents"]).Select(document =>
                    new JObject { ["slug"] = document["slug"], ["version"] = document["currentVersion"] })),
                ["language"] = "ja",
                ["age_attested"] = true,
            });
            foreach (var peer in new[] { mine, path })
                await Send(publisher, "import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(peer) });
            var ticket = (string)await Send(publisher, "get_local_peer_ticket");
            await Call("import_peer_ticket", new JObject { ["ticket"] = ticket });
            File.WriteAllText(Path.Combine(exchange, "publisher.tmp"), ticket + "\n" + (string)(await Send(publisher, "get_sync_status"))["local_author_pubkey"]);
            File.Move(Path.Combine(exchange, "publisher.tmp"), Path.Combine(exchange, "publisher.info"));
        }
        var published = Path.Combine(exchange, "publisher.info");
        await Until(() => File.Exists(published), 60, "the publisher did not start");
        var lines = File.ReadAllLines(published);
        if (role == "client")
            await Call("import_peer_ticket", new JObject { ["ticket"] = lines[0] });
        skins.PublisherPubkey = lines[1];
    }

    static async Task<JToken> Send(KukuriClient client, string command, JObject payload = null)
    {
        var response = await client.SendAsync(command, payload);
        if (!response.Ok) throw new Exception(command + ": " + response.ErrorCode);
        return response.Data;
    }

    /// 単色の PNG をスキンとして発行する（作者の操作）。
    async Task<long> PublishSkin(Color color, string asset = "neon-red")
    {
        var texture = new Texture2D(64, 64);
        texture.SetPixels(Enumerable.Repeat(color, 64 * 64).ToArray());
        var file = Path.Combine(Arg("-arena-exchange"), $"skin-{Guid.NewGuid():N}.png");
        File.WriteAllBytes(file, texture.EncodeToPNG());
        Destroy(texture);
        var release = await Send(publisher, "publish_game_asset", new JObject
        {
            ["topic"] = Arg("-arena-topic"),
            ["application_id"] = "com.kukuri.cyber-disc-arena",
            ["asset_id"] = asset,
            ["format"] = new JObject { ["kind"] = "png" },
            ["path"] = file,
        });
        return (long)release["revision"];
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
        Debug.Log("KGS_DODGES " + dodges);
        Debug.Log($"KGS_FINAL {session.Phase} {session.Winner} {session.Scores[0]} {session.Scores[1]} {session.LastEvent} {session.EventGaps} " +
            string.Join(" ", players.Select(player => $"{player.Position.x:F3},{player.Position.y:F3},{player.Position.z:F3}")));
    }

    async Task Host()
    {
        await Prepare();
        // 作者がスキンを発行すると、ロッカーに出る。選ぶと自分の体に貼られる。
        var plain = skins.Loader(0).GetComponent<Renderer>().sharedMaterial;
        await PublishSkin(new Color(1f, 0.15f, 0.2f));
        await PublishSkin(new Color(1f, 0.8f, 0.1f), "gold");
        File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "skins-published"), "done");
        await Until(() => skins.Published.Contains("neon-red"), 60, "the locker did not show the published skin");
        // 着ているスキンから別のスキン、standard へ着替えてから、neon-red を着る。
        await Until(() => skins.Published.Contains("gold"), 60, "the locker did not show the second skin");
        skins.Select("gold");
        await Until(() => skins.Loader(0).AssetId == "gold" && skins.Loader(0).AppliedRevision >= 1, 60, "gold was not worn");
        skins.Select(ArenaProfile.StandardSkin);
        await Until(() => skins.Loader(0).AssetId == "", 10, "standard was not worn");
        Debug.Log("KGS_SKIN_STANDARD " + (skins.Loader(0).AssetId == "" && skins.Loader(0).AppliedRevision == 0 &&
            skins.Loader(0).GetComponent<Renderer>().sharedMaterial == plain));
        skins.Select("neon-red");
        await Until(() => skins.Loader(0).AppliedRevision >= 1, 60, "the skin was not worn");
        Debug.Log("KGS_SKIN_WORN " + profile.Skin + " " + skins.Loader(0).AppliedRevision);
        await Shot("0-locker");
        broadcast.BroadcastNext = true;
        // 対戦中は隠したロビーの button が Space で押されない（選択と navigation を外す）。
        lobby.GetComponentsInChildren<UnityEngine.UI.Button>(true).First(button => button.name.StartsWith("Create")).Select();
        await lobby.CreateRoomAsync("test arena 1");
        await Until(() => session.Phase == ArenaPhase.Playing, 120, "the match did not start");
        // client が着たスキンを、client の体に P2P で取って貼る。
        await Until(() => session.RemoteSkin == "gold" && skins.Loader(1).AppliedRevision >= 1, 60, "the client's skin did not arrive");
        Debug.Log("KGS_SKIN_FROM_CLIENT " + session.RemoteSkin + " " + skins.Loader(1).AppliedRevision);
        await Pause(4);
        var events = UnityEngine.EventSystems.EventSystem.current;
        Debug.Log($"KGS_UI_LOCKED {!events.sendNavigationEvents && events.currentSelectedGameObject == null}");
        await Shot("1-playing");
        // client が host のスキンを貼ったら、対戦中に新しい版を発行する。両方の体が新しい版に変わる。
        await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "client-skin")), 60, "the client did not wear the host's skin");
        var revision = await PublishSkin(new Color(0.2f, 0.4f, 1f));
        await Until(() => skins.Loader(0).AppliedRevision >= revision, 60, "the new revision was not worn");
        Debug.Log("KGS_SKIN_UPDATED " + skins.Loader(0).AppliedRevision);
        // 試合は公開の Live で配信され、観戦の Player と desktop の応援が ring の上に出る。
        await Until(() => broadcast.LiveSession != null && broadcast.LivePost != null, 60, "the live did not start");
        Debug.Log("KGS_LIVE_STARTED " + broadcast.LiveSession);
        File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "live.tmp"), $"{Arg("-arena-topic")}\n{broadcast.LiveSession}\n{broadcast.LivePost}");
        File.Move(Path.Combine(Arg("-arena-exchange"), "live.tmp"), Path.Combine(Arg("-arena-exchange"), "live"));
        var expected = new[] { "FIRE!", Arg("-arena-desktop") != null ? "CLAP!" : null, Arg("-arena-web") != null ? "GG!" : null }.Where(label => label != null).ToArray();
        await Until(() => expected.All(cheers.Contains), 180, "the cheers did not arrive");
        await Shot("1c-cheer");
        Debug.Log("KGS_CHEERS " + string.Join(",", cheers.OrderBy(label => label)));
        // 対戦中に掲示板を開いて入力欄を選ぶと、対戦の操作に使わない（arena の他の IMGUI は出さない）。
        var boardObject = arenaBoard.Board.gameObject;
        boardObject.SetActive(true);
        await Pause(1);
        UnityEngine.EventSystems.EventSystem.current.SetSelectedGameObject(
            boardObject.GetComponentsInChildren<UnityEngine.UI.InputField>(true).First(field => field.name == "PostInput").gameObject);
        await Pause(0.5f);
        Debug.Log($"KGS_BOARD_TYPING {ArenaControls.Typing()} {ArenaBoard.Open}");
        await Shot("1d-board");
        UnityEngine.EventSystems.EventSystem.current.SetSelectedGameObject(null);
        boardObject.SetActive(false);
        await Until(() => session.Phase == ArenaPhase.Result, 480, "the match did not end");
        await Pause(1.5f);
        Final();
        await Until(() => broadcast.LiveSession == null, 30, "the live did not stop");
        Debug.Log("KGS_LIVE_ENDED True");
        await Shot("2-result");
        // 結果を投稿する（得点の文と画面の画像）。
        var resultPost = await view.PostResult();
        Debug.Log("KGS_RESULT_POST " + (resultPost ?? "none") + " " + arenaBoard.Notice);
        if (resultPost != null)
        {
            File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "result-post.tmp"), resultPost);
            File.Move(Path.Combine(Arg("-arena-exchange"), "result-post.tmp"), Path.Combine(Arg("-arena-exchange"), "result-post"));
        }
        File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "host-result"), "done");
        await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "client-result")), 60, "the client did not see the result");
        // 「ロビーへ戻る」: host は room を終える。
        session.Leave();
        await Until(() => !network.IsListening, 30, "the host NGO did not stop");
        Debug.Log($"KGS_HOST_LEFT listening={network.IsListening} in_match={session.InMatch}");

        // 2 試合目: client が途中で退出する。
        await lobby.CreateRoomAsync("test arena 2");
        await Until(() => session.Phase == ArenaPhase.Playing, 120, "the second match did not start");
        // 2 試合目でも client の入力を使う（前の試合の通し番号で捨てない）。
        await Pause(1.5f);
        Debug.Log("KGS_SECOND_INPUT " + session.Players[1].LastInput);
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
        await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "skins-published")), 120, "the skins were not published");
        await Until(() => skins.Published.Contains("gold"), 60, "the client's locker did not show gold");
        skins.Select("gold");
        await Until(() => lobby.Rooms.Any(room => room.Title == "test arena 1"), 120, "the room did not arrive");
        await lobby.JoinRoomAsync(lobby.Rooms.First(room => room.Title == "test arena 1").RoomId);
        await Until(() => session.Phase == ArenaPhase.Playing, 120, "the match did not start");
        // host の体に、host が選んだスキンを P2P で取って貼る。
        await Until(() => session.RemoteSkin == "neon-red" && skins.Loader(0).AppliedRevision >= 1, 60, "the host's skin did not arrive");
        Debug.Log("KGS_SKIN_REMOTE " + session.RemoteSkin + " " + skins.Loader(0).AppliedRevision);
        await Pause(4);
        await Shot("1-playing");
        File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "client-skin"), "seen");
        await Until(() => skins.Loader(0).AppliedRevision >= 2, 90, "the new revision did not arrive");
        Debug.Log("KGS_SKIN_UPDATED " + skins.Loader(0).AppliedRevision);
        await Shot("1b-updated");
        await Until(() => session.Phase == ArenaPhase.Result, 480, "the match did not end");
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
        // 観戦の Player が 2 試合目の配信を受け直すまで続ける。
        if (Arg("-arena-spectator") != null)
            await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "rewatched")), 180, "the spectator did not watch the second live");
        session.Leave();
        await Until(() => !network.IsListening, 30, "the client NGO did not stop");
        await Pause(1);
        Debug.Log($"KGS_CLIENT_LEFT listening={network.IsListening} joined={lobby.JoinedSessionId ?? "none"}");
        await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "host-done")), 60, "the host did not finish");
    }

    /// 状態の継続。`-arena-step` の 0: 同意だけして閉じる。1: 開始の画面で起動し直し、ロビーが動いていないのを確かめてから
    /// 始め、スキンを選び、Steam Cloud に保存し、アカウント B を足す。2: 再開し、B の進行がある確定の画面を取り消してから、
    /// B の進行を残してまとめる。3: Steam Cloud の file だけを写した新しい profile で再開する。
    async Task State()
    {
        var step = Arg("-arena-step");
        if (step == "0")
        {
            await Consent();
            return;
        }
        if (step == "1")
        {
            await Until(() => account.Screen == "start", 60, "the start screen did not open");
            await Pause(3);
            var sync = await Call("get_sync_status");
            var idle = !lobby.gameObject.activeInHierarchy &&
                !((JArray)sync["subscribed_topics"]).Any(name => (string)name == "kukuri:topic:cyber-disc-arena");
            Debug.Log("KGS_LOBBY_IDLE " + idle);
            await account.Begin();
        }
        await StartGame(step != "3");
        Debug.Log($"KGS_STATE{step} {profile.GamePlayerId} {profile.Skin} {account.PublicKey}");
        if (step == "1")
        {
            profile.Skin = "neon-red";
            await account.SetCloud(true);
            var current = (string)(await Call("list_accounts"))["active_account_id"];
            var b = await Call("create_account", new JObject { ["account_id"] = current, ["operation_id"] = Guid.NewGuid().ToString() });
            var accounts = await Call("list_accounts");
            var pubkey = (string)((JArray)accounts["accounts"]).First(item => (string)item["id"] == (string)b["id"])["pubkey"];
            Debug.Log($"KGS_STATE_SAVED {profile.Skin} {account.CloudState} {(string)b["id"]} {pubkey}");
        }
        if (step == "2")
        {
            // 確定の画面で取り消すと何も変わらない。
            var before = $"{profile.GamePlayerId} {profile.Skin} {account.PublicKey}";
            await account.EnterMerge();
            await account.ChooseMerge(Arg("-arena-merge-to"));
            var other = account.OtherProgress(Arg("-arena-merge-pubkey"));
            account.Keep(other?.id);
            await Shot("5-merge-confirm");
            account.CancelMerge();
            await Pause(1);
            Debug.Log($"KGS_STATE_CANCELLED {account.Screen} {before == $"{profile.GamePlayerId} {profile.Skin} {account.PublicKey}"}");
            await account.EnterMerge();
            await account.ChooseMerge(Arg("-arena-merge-to"));
            Debug.Log($"KGS_STATE_OFFERED {account.Screen} {other?.id ?? "none"} {other?.label ?? "none"}");
            account.Keep(other?.id);
            await account.Merge();
            await Until(() => account.Screen == "playing" && account.GamePlayerId == other?.id && profile.Ready, 60, "the merge did not keep the other progress");
            Debug.Log($"KGS_STATE_MERGED {profile.GamePlayerId} {profile.Skin} {account.PublicKey}");
            await Shot("6-merged");
        }
    }

    /// ゲームを終えた後に再起動した host: 掲示板（Enter）で結果の投稿の thread を開き、desktop・Web の返信を読む。
    async Task Resume()
    {
        await StartGame(true);
        var exchange = Arg("-arena-exchange");
        if (File.Exists(Path.Combine(exchange, "desktop-ticket")))
            await Call("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(Path.Combine(exchange, "desktop-ticket")) });
        var board = arenaBoard.Board;
        board.Topic = Arg("-arena-topic");
        board.gameObject.SetActive(true);
        var post = File.ReadAllText(Path.Combine(exchange, "result-post")).Trim();
        await Until(() => board.Posts.Any(item => item.ObjectId == post), 120, "the result was not on the board");
        await board.OpenThreadAsync(post);
        foreach (var name in new[] { "desktop", "web" }.Where(name => File.Exists(Path.Combine(exchange, name + "-replied"))))
        {
            var reply = File.ReadAllText(Path.Combine(exchange, name + "-replied")).Trim();
            await Until(() => board.Posts.Any(item => item.ObjectId == reply), 120, "the reply from " + name + " did not arrive");
            var root = board.Posts[0];
            var answer = board.Posts.First(item => item.ObjectId == reply);
            Debug.Log($"KGS_RESULT_THREAD {name} {root.ObjectId} {answer.ObjectId} {answer.ThreadId} {root.AuthorPubkey != answer.AuthorPubkey}");
        }
        await Shot("resume-thread");
        File.WriteAllText(Path.Combine(exchange, "done"), "done");
    }

    /// 観戦の Player: ロビーのモニターで配信中の試合を受けて映し、R の応援（🔥）を送る。
    /// 最小の scene の役: 始めて、ticket を書き、相手（host など）の ticket を取り込む。
    async Task Join(params string[] peers)
    {
        await StartGame(false);
        var exchange = Arg("-arena-exchange");
        var mine = Path.Combine(exchange, role + ".ticket");
        File.WriteAllText(mine + ".tmp", (string)await Call("get_local_peer_ticket"));
        File.Move(mine + ".tmp", mine);
        foreach (var peer in peers)
        {
            var path = Path.Combine(exchange, peer + ".ticket");
            await Until(() => File.Exists(path), 120, "no ticket from " + peer);
            await Call("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(path) });
        }
    }

    /// LiveSkinLoader の最小の scene: 作者が発行したスキンをロッカーで選び、見本の体に着せる。
    async Task MiniSkin()
    {
        await Join("host");
        var published = Path.Combine(Arg("-arena-exchange"), "publisher.info");
        await Until(() => File.Exists(published), 120, "the publisher did not start");
        var lines = File.ReadAllLines(published);
        await Call("import_peer_ticket", new JObject { ["ticket"] = lines[0] });
        skins.PublisherPubkey = lines[1];
        skins.Topic = Arg("-arena-topic");
        await Until(() => skins.Published.Contains("neon-red"), 120, "the minimal locker did not show the skin");
        skins.Select("neon-red");
        await Until(() => skins.Loader(0).AppliedRevision >= 1, 60, "the mannequin did not wear the skin");
        Debug.Log("KGS_MINI_SKIN " + profile.Skin + " " + (skins.Loader(0).AppliedRevision >= 1));
        await Shot("skin");
    }

    /// InGameBoardUI の最小の scene: 掲示板を開いて投稿し、一覧で読む。
    async Task MiniBoard()
    {
        await StartGame(false);
        var board = arenaBoard.Board;
        board.Topic = Arg("-arena-topic") + "-mini";
        board.gameObject.SetActive(true);
        var post = await board.PostAsync("test: from the minimal board");
        await Until(() => board.Posts.Any(item => item.ObjectId == post), 60, "the minimal board did not show the post");
        Debug.Log("KGS_MINI_BOARD " + (post != null));
        await Shot("board");
    }

    /// DiscNetworkSync の最小の scene: 部屋を作って参加し、試合を始める。host が部屋を閉じると client に知らせが出る。
    async Task MiniDisc()
    {
        var host = role == "mini-host";
        await Join(host ? "mini-client" : "mini-host");
        lobby.Topic = Arg("-arena-topic") + "-disc";
        var threw = false;
        session.Confirmed += @event => threw |= @event.Kind == ArenaEventKind.Throw;
        if (host)
            await lobby.CreateRoomAsync("mini arena");
        else
        {
            await Until(() => lobby.Rooms.Any(room => room.Title == "mini arena"), 120, "the minimal room did not arrive");
            await lobby.JoinRoomAsync(lobby.Rooms.First(room => room.Title == "mini arena").RoomId);
        }
        await Until(() => session.Phase == ArenaPhase.Playing && threw, 120, "the minimal match did not start");
        await Shot("disc");
        if (host)
        {
            await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "mini-played")), 60, "the client did not play");
            session.Leave();
            await Until(() => !network.IsListening, 30, "the minimal host did not stop");
            Debug.Log("KGS_MINI_DISC host");
            return;
        }
        File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "mini-played"), "done");
        await Until(() => !session.InMatch, 60, "the minimal client did not return");
        Debug.Log("KGS_MINI_DISC " + session.Notice);
    }

    async Task Spectator()
    {
        // ArenaStreamViewer の最小の scene で観戦する（対戦の Prefab は無い）。desktop は host が終えた後にも配信するので、
        // desktop とも直接つながる（#1984 AC-1）。
        await Join("host");
        var desktopTicket = Path.Combine(Arg("-arena-exchange"), "desktop-ticket");
        if (Arg("-arena-desktop") != null)
        {
            await Until(() => File.Exists(desktopTicket), 120, "no ticket from the desktop");
            await Call("import_peer_ticket", new JObject { ["ticket"] = File.ReadAllText(desktopTicket) });
        }
        broadcast.Topic = Arg("-arena-topic");
        await Until(() => broadcast.Viewer.SessionId != null && broadcast.Viewer.FramesShown > 10, 180, "the monitors did not show the live");
        var first = broadcast.Viewer.SessionId;
        Debug.Log("KGS_SPECTATED " + broadcast.Viewer.FramesShown);
        await Shot("spectator-monitors");
        await broadcast.React("🔥");
        Debug.Log("KGS_REACTED 🔥");
        // 試合が終わって配信が止まると、モニターはその配信から離れ、次の試合の配信を受け直す。
        await Until(() => broadcast.Viewer.SessionId == null, 600, "the monitors did not leave the finished live");
        Debug.Log("KGS_LEFT_FINISHED True");
        var frames = broadcast.Viewer.FramesShown;
        await Until(() => broadcast.Viewer.SessionId != null && broadcast.Viewer.SessionId != first && broadcast.Viewer.FramesShown > frames + 10,
            180, "the monitors did not show the next live");
        Debug.Log("KGS_REWATCHED True");
        File.WriteAllText(Path.Combine(Arg("-arena-exchange"), "rewatched"), "done");
        await Until(() => File.Exists(Path.Combine(Arg("-arena-exchange"), "host-done")), 600, "the host did not finish");
        // host が終えた後に desktop・Web が始める配信も、モニターが 1 つずつ受けて映す（#1984 AC-1）。映ったら印を書き、
        // その配信が終わると、モニターは残りの配信へ移る。
        var external = new[] { "desktop", "web" }.Where(name => Arg("-arena-" + name) != null).ToList();
        while (external.Count > 0)
        {
            string current = null, seen = null;
            var since = 0;
            await Until(() =>
            {
                if (broadcast.Viewer.SessionId != current)
                    (current, since) = (broadcast.Viewer.SessionId, broadcast.Viewer.FramesShown);
                seen = external.FirstOrDefault(name =>
                    current != null && File.Exists(Path.Combine(Arg("-arena-exchange"), name + "-live")) &&
                    File.ReadAllText(Path.Combine(Arg("-arena-exchange"), name + "-live")).Trim() == current);
                return seen != null && broadcast.Viewer.FramesShown > since + 10;
            }, 300, "the monitors did not show the live of " + string.Join(" or ", external));
            Debug.Log($"KGS_EXTERNAL_WATCHED {seen} {broadcast.Viewer.FramesShown - since}");
            await Shot("spectator-" + seen);
            File.WriteAllText(Path.Combine(Arg("-arena-exchange"), seen + "-live-seen"), "done");
            external.Remove(seen);
        }
    }

    void Update()
    {
        var watching = broadcast != null ? broadcast.Viewer.SessionId : null;
        if (watching != null && watching != mutedSession && runner.Client != null)
        {
            mutedSession = watching;
            _ = runner.Client.SendAsync("set_live_volume", new JObject { ["session_id"] = watching, ["volume"] = 0.0 });
        }
        publisher?.Pump();
        // 部屋の一覧は、待っている部屋が来るまで短い間隔で読み直す。
        if ((role == "client" || role == "mini-client") && runner.Client != null && !session.InMatch && Time.frameCount % 30 == 0)
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
        Link(probe, "account", Object.FindAnyObjectByType<KukuriAccountScreen>());
        Link(probe, "profile", Object.FindAnyObjectByType<ArenaProfile>());
        Link(probe, "skins", Object.FindAnyObjectByType<ArenaSkins>());
        Link(probe, "broadcast", Object.FindAnyObjectByType<ArenaBroadcast>());
        Link(probe, "cheer", Object.FindAnyObjectByType<ArenaCheer>());
        Link(probe, "view", Object.FindAnyObjectByType<ArenaView>());
        Link(probe, "arenaBoard", Object.FindAnyObjectByType<ArenaBoard>());
        EditorSceneManager.SaveScene(scene, "Assets/ArenaProbe/ArenaTest.unity");
        // Prefab ごとの最小の scene にも同じ probe を置く（`-arena-scene` で移る）。
        var boot = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);
        new GameObject("probe").AddComponent<ArenaProbe>();
        EditorSceneManager.SaveScene(boot, "Assets/ArenaProbe/Boot.unity");
        var scenes = new System.Collections.Generic.List<string> { "Assets/ArenaProbe/Boot.unity", "Assets/ArenaProbe/ArenaTest.unity" };
        foreach (var name in new[] { "DiscNetworkSync", "LiveSkinLoader", "ArenaStreamViewer", "InGameBoardUI" })
        {
            var minimal = EditorSceneManager.OpenScene($"Assets/CyberDiscArena/Scenes/Minimal/{name}.unity");
            var minimalRunner = Object.FindAnyObjectByType<KukuriRunner>();
            minimalRunner.enabled = false;
            var minimalProbe = new GameObject("probe").AddComponent<ArenaProbe>();
            Link(minimalProbe, "runner", minimalRunner);
            Link(minimalProbe, "lobby", Object.FindAnyObjectByType<KukuriLobby>());
            Link(minimalProbe, "session", Object.FindAnyObjectByType<ArenaSession>());
            Link(minimalProbe, "network", Object.FindAnyObjectByType<NetworkManager>());
            Link(minimalProbe, "account", Object.FindAnyObjectByType<KukuriAccountScreen>());
            Link(minimalProbe, "profile", Object.FindAnyObjectByType<ArenaProfile>());
            Link(minimalProbe, "skins", Object.FindAnyObjectByType<ArenaSkins>());
            Link(minimalProbe, "broadcast", Object.FindAnyObjectByType<ArenaBroadcast>());
            Link(minimalProbe, "cheer", Object.FindAnyObjectByType<ArenaCheer>());
            Link(minimalProbe, "view", Object.FindAnyObjectByType<ArenaView>());
            Link(minimalProbe, "arenaBoard", Object.FindAnyObjectByType<ArenaBoard>());
            EditorSceneManager.SaveScene(minimal, $"Assets/ArenaProbe/{name}.unity");
            scenes.Add($"Assets/ArenaProbe/{name}.unity");
        }
        PlayerSettings.SetScriptingBackend(NamedBuildTarget.Standalone, ScriptingImplementation.__BACKEND__);
        var report = BuildPipeline.BuildPlayer(new BuildPlayerOptions
        {
            scenes = scenes.ToArray(),
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
# スキンの作者は開発用の Sidecar（発行は Sidecar・CLI だけ）。
$sidecarExe = Join-Path $package "Editor\Sidecar~\kukuri-sdk-sidecar.exe"
$publisherDir = Join-Path $work "publisher"
$sidecar = Start-Process -FilePath $sidecarExe -ArgumentList "run --profile publisher --profile-dir `"$publisherDir`"" -PassThru -RedirectStandardError (Join-Path $work "sidecar.err")
for ($i = 0; $i -lt 120 -and -not (Select-String -Path (Join-Path $work "sidecar.err") -Pattern "^ready" -Quiet -ErrorAction SilentlyContinue); $i++) { Start-Sleep -Milliseconds 500 }
$players = @{}
$desktop = $null
if ($DesktopApp) {
    $driver = Join-Path $PSScriptRoot "..\..\sdk\unity\transfer-desktop.mjs"
    $desktopLog = Join-Path $work "desktop.log"
    $desktop = Start-Process -FilePath "node" -ArgumentList "`"$driver`" `"$DesktopApp`" `"$work`" live" `
        -RedirectStandardOutput $desktopLog -RedirectStandardError (Join-Path $work "desktop.err") -NoNewWindow -PassThru
    $null = $desktop.Handle
}
$web = $null
$communityNode = $null
if ($WebOrigin) {
    # WebRTC の ICE の候補は bind の IP で、Chrome は loopback の候補と組まない（組めないと relay を通る）。Web とつなぐ回は
    # 127.0.0.1 に bind しない（既定の経路の IP になる。ticket の宛先は 127.0.0.1 のまま）。
    Remove-Item env:KUKURI_BIND_ADDR
    $communityNode = (Invoke-RestMethod "$WebOrigin/fixture/info").community_node
    $driver = Join-Path $PSScriptRoot "..\..\sdk\unity\transfer-desktop.mjs"
    $webLog = Join-Path $work "web.log"
    $web = Start-Process -FilePath "node" -ArgumentList "`"$driver`" $WebOrigin `"$work`" web" `
        -RedirectStandardOutput $webLog -RedirectStandardError (Join-Path $work "web.err") -NoNewWindow -PassThru
    $null = $web.Handle
}
$watchers = "-arena-spectator 1" + $(if ($DesktopApp) { " -arena-desktop 1" } else { "" }) + $(if ($WebOrigin) { " -arena-web 1" } else { "" })
$roles = [ordered]@{ "host" = ""; "client" = ""; "spectator" = "ArenaStreamViewer"; "mini-skin" = "LiveSkinLoader";
    "mini-board" = "InGameBoardUI"; "mini-host" = "DiscNetworkSync"; "mini-client" = "DiscNetworkSync" }
foreach ($role in $roles.Keys) {
    $log = Join-Path $work "$role.log"
    $scene = if ($roles[$role]) { "-arena-scene $($roles[$role]) " } else { "" }
    if ($communityNode -and $role -in "host", "spectator") { $scene += "-arena-cn $communityNode " }
    $arguments = "-screen-fullscreen 0 -screen-width 1280 -screen-height 720 -arena-role $role $scene-arena-profile-dir `"$(Join-Path $work $role)`" " +
        "-arena-topic $topic -arena-exchange `"$work`" -arena-publisher-dir `"$publisherDir`" $watchers -arena-delay $DelayMs -arena-loss $LossPercent -logFile `"$log`""
    $players[$role] = @{ Process = (Start-Process -FilePath $exe -ArgumentList $arguments -PassThru); Log = $log }
}
$exits = @{}
foreach ($role in $roles.Keys) {
    $process = $players[$role].Process
    if (-not $process.WaitForExit(600000)) { Stop-Process -Id $process.Id -Force; $exits[$role] = "timeout" } else { $exits[$role] = $process.ExitCode }
}

Start-Process -FilePath $sidecarExe -ArgumentList "stop --profile publisher --profile-dir `"$publisherDir`"" -Wait | Out-Null
if (-not $sidecar.WaitForExit(15000)) { Stop-Process -Id $sidecar.Id -Force }
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
$dodges = "$((Lines "host" "KGS_DODGES (\d+)") | Select-Object -First 1),$((Lines "client" "KGS_DODGES (\d+)") | Select-Object -First 1)"
$dodged = @($dodges -split "," | Where-Object { $_ -match "^\d+$" -and [int]$_ -ge 1 }).Count -eq 2
$uiLocked = (Lines "host" "KGS_UI_LOCKED (\S+)") | Select-Object -First 1
$secondInput = [int]((Lines "host" "KGS_SECOND_INPUT (\d+)") | Select-Object -First 1)
$skinWorn = (Lines "host" "KGS_SKIN_WORN (.*)") | Select-Object -First 1
$skinRemote = (Lines "client" "KGS_SKIN_REMOTE (.*)") | Select-Object -First 1
$skinUpdated = "$((Lines "host" "KGS_SKIN_UPDATED (\d+)") | Select-Object -First 1),$((Lines "client" "KGS_SKIN_UPDATED (\d+)") | Select-Object -First 1)"
# ゲームを終えた後に desktop・Web が結果へ返信し、再起動した host が掲示板の thread で読む。
$resultPost = (Lines "host" "KGS_RESULT_POST (\S+)") | Select-Object -First 1
$externals = @(@{ Name = "desktop"; Process = $desktop; Log = $desktopLog }, @{ Name = "web"; Process = $web; Log = $webLog }) | Where-Object { $_.Process }
if ($externals) {
    Set-Content (Join-Path $work "game-exited") "exited"
    $resumeLog = Join-Path $work "resume.log"
    foreach ($external in $externals) {
        for ($i = 0; $i -lt 240 -and -not (Test-Path (Join-Path $work "$($external.Name)-replied")); $i++) { Start-Sleep -Milliseconds 500 }
    }
    $arguments = "-screen-fullscreen 0 -screen-width 1280 -screen-height 720 -arena-role resume -arena-profile-dir `"$(Join-Path $work "host")`" " +
        "-arena-topic $topic -arena-exchange `"$work`" -logFile `"$resumeLog`""
    $resume = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru
    $exits["resume"] = if ($resume.WaitForExit(300000)) { $resume.ExitCode } else { Stop-Process -Id $resume.Id -Force; "timeout" }
}
$liveStarted = (Lines "host" "KGS_LIVE_STARTED (\S+)") | Select-Object -First 1
$liveEnded = (Lines "host" "KGS_LIVE_ENDED (\S+)") | Select-Object -First 1
$cheersShown = (Lines "host" "KGS_CHEERS (\S+)") | Select-Object -First 1
$spectated = (Lines "spectator" "KGS_SPECTATED (\d+)") | Select-Object -First 1
$rewatched = ((Lines "spectator" "KGS_LEFT_FINISHED (\S+)") | Select-Object -First 1) + "," + ((Lines "spectator" "KGS_REWATCHED (\S+)") | Select-Object -First 1)
# desktop・Web: 配信を受けて応援し、結果を画像つきで読み、自分の配信が観戦の Player のモニターに映り、返信が再起動した host の
# 掲示板の thread に並ぶ。topic の経路は Relay Fallback でない（desktop は Direct P2P、Web は Relay Supported P2P）。
$externalsOk = $true
foreach ($external in $externals) {
    if (-not $external.Process.WaitForExit(120000)) { Stop-Process -Id $external.Process.Id -Force }
    $name = $external.Name
    $logged = { param($pattern) Select-String -Path $external.Log -Pattern $pattern | Select-Object -First 1 | ForEach-Object { $_.Matches[0].Groups[1].Value } }
    $resultSeen = & $logged "^result-seen (\d+)"
    $replied = & $logged "^replied (\S+)"
    $thread = Select-String -Path $resumeLog -Pattern "KGS_RESULT_THREAD $name (.*)" | Select-Object -First 1 | ForEach-Object { $_.Matches[0].Groups[1].Value }
    $monitor = (Lines "spectator" "KGS_EXTERNAL_WATCHED $name (\d+)") | Select-Object -First 1
    $path = & $logged "^path (\S+)"
    $externalOk = (Select-String -Path $external.Log -Pattern "^watched" -Quiet) -and (Select-String -Path $external.Log -Pattern "^reacted" -Quiet) -and
        ([int]$resultSeen -ge 1) -and (Select-String -Path $external.Log -Pattern "^broadcast-seen" -Quiet) -and ([int]$monitor -gt 10) -and
        ($thread -eq "$resultPost $replied $resultPost True") -and ($path -in "direct_p2p", "relay_supported_p2p")
    $externalsOk = $externalsOk -and $externalOk
    Write-Output "Player $name ($Backend): watched_and_reacted=$((Select-String -Path $external.Log -Pattern '^reacted' -Quiet)) result_attachments=$resultSeen monitor_frames=$monitor path=$path reply=$replied thread=[$thread] ok=$externalOk"
}
$typing = (Lines "host" "KGS_BOARD_TYPING (.*)") | Select-Object -First 1
$boardOk = ($typing -eq "True True") -and ($resultPost -match "^[0-9a-f]{64}$") -and ((-not $externals) -or ($exits["resume"] -eq 0))
Write-Output "Player board ($Backend): typing_blocks_match=[$typing] result=$resultPost exits_resume=$($exits['resume']) ok=$boardOk"
$expectedCheers = (@("FIRE!") + $(if ($DesktopApp) { "CLAP!" }) + $(if ($WebOrigin) { "GG!" }) | Sort-Object) -join ","
$liveOk = ($liveStarted -match "\S") -and ($liveEnded -eq "True") -and ($cheersShown -eq $expectedCheers) -and ([int]$spectated -gt 10) -and ($rewatched -eq "True,True") -and
    $externalsOk -and ($exits["spectator"] -eq 0)
Write-Output "Player live ($Backend): started=$liveStarted spectator_frames=$spectated left_and_rewatched=$rewatched cheers=$cheersShown externals=$(if ($externals) { $externalsOk } else { 'skipped' }) ended=$liveEnded exits_spectator=$($exits['spectator']) ok=$liveOk"
$skinStandard = (Lines "host" "KGS_SKIN_STANDARD (\S+)") | Select-Object -First 1
$skinFromClient = (Lines "host" "KGS_SKIN_FROM_CLIENT (.*)") | Select-Object -First 1
$skinOk = ($skinWorn -eq "neon-red 1") -and ($skinStandard -eq "True") -and ($skinFromClient -eq "gold 1") -and ($skinRemote -eq "neon-red 1") -and ($skinUpdated -eq "2,2")
$hostLeft = (Lines "host" "KGS_HOST_LEFT (.*)") | Select-Object -First 1
$clientClosed = (Lines "client" "KGS_CLIENT_CLOSED (.*)") | Select-Object -First 1
$hostSawLeave = (Lines "host" "KGS_HOST_SAW_LEAVE (.*)") | Select-Object -First 1
$clientLeft = (Lines "client" "KGS_CLIENT_LEFT (.*)") | Select-Object -First 1
$failure = Select-String -Path $players["host"].Log, $players["client"].Log -Pattern "KGS_FAILED.*" | Select-Object -First 1
$miniSkin = (Lines "mini-skin" "KGS_MINI_SKIN (.*)") | Select-Object -First 1
$miniBoard = (Lines "mini-board" "KGS_MINI_BOARD (\S+)") | Select-Object -First 1
$miniDisc = "$((Lines "mini-host" "KGS_MINI_DISC (.*)") | Select-Object -First 1),$((Lines "mini-client" "KGS_MINI_DISC (.*)") | Select-Object -First 1)"
$miniOk = ($miniSkin -eq "neon-red True") -and ($miniBoard -eq "True") -and ($miniDisc -eq "host,ホストが部屋を閉じました") -and
    (@("mini-skin", "mini-board", "mini-host", "mini-client" | Where-Object { $exits[$_] -ne 0 }).Count -eq 0)
Write-Output "Player prefabs ($Backend): stream_viewer=spectator skin=[$miniSkin] board=$miniBoard disc=[$miniDisc] exits=$(("mini-skin", "mini-board", "mini-host", "mini-client" | ForEach-Object { $exits[$_] }) -join ',') ok=$miniOk"
$ok = $miniOk -and $boardOk -and $liveOk -and $skinOk -and $sameEvents -and $sameResult -and $positionsMatch -and $dodged -and ($uiLocked -eq "True") -and ($secondInput -ge 20) -and ($secondInput -le 600) -and ("Throw" -in $kinds) -and ("Catch" -in $kinds) -and ("Hit" -in $kinds) -and
    ($hostLeft -eq "listening=False in_match=False") -and ($clientClosed -eq "listening=False joined=none notice=ホストが部屋を閉じました") -and
    ($hostSawLeave -eq "Left") -and ($clientLeft -eq "listening=False joined=none") -and ($exits["host"] -eq 0) -and ($exits["client"] -eq 0)
Write-Output "Player match ($Backend, delay=${DelayMs}ms loss=$LossPercent%): events=$($hostFirst.Count) same=$sameEvents kinds=$($kinds -join ',') dodges=$dodges ui_locked=$uiLocked host=[$hostFinal] client=[$clientFinal] positions=$positionsMatch"
Write-Output "Player skin ($Backend): worn=[$skinWorn] standard_restored=$skinStandard from_client=[$skinFromClient] remote=[$skinRemote] updated=$skinUpdated ok=$skinOk"
Write-Output "Player leave ($Backend): second_match_inputs=$secondInput host_left=[$hostLeft] client_closed=[$clientClosed] host_saw_leave=[$hostSawLeave] client_left=[$clientLeft] exits=$($exits['host']),$($exits['client']) ok=$ok"
if ($failure) { Write-Output "  $($failure.Line)" }

# 状態の継続: 同じ profile で起動し直し、Steam Cloud の file だけを写した新しい profile でも再開する。
function Start-State([string]$step, [string]$dir, [string]$extra = "") {
    $log = Join-Path $work "state$step.log"
    $arguments = "-screen-fullscreen 0 -screen-width 1280 -screen-height 720 -arena-role state -arena-step $step " +
        "-arena-profile-dir `"$dir`" -arena-exchange `"$work`" $extra -logFile `"$log`""
    $process = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru
    $code = if ($process.WaitForExit(180000)) { $process.ExitCode } else { Stop-Process -Id $process.Id -Force; "timeout" }
    return @{ Log = $log; Exit = $code }
}
function Value($run, [string]$pattern) {
    $match = Select-String -Path $run.Log -Pattern $pattern | Select-Object -First 1
    if ($match) { $match.Matches[0].Groups[1].Value } else { "" }
}
$stateDir = Join-Path $work "state"
$restoreDir = Join-Path $work "restore"
$consented = Start-State "0" $stateDir
$first = Start-State "1" $stateDir
$started = Value $first "KGS_STATE1 (\S+ \S+ \S+)"
$saved = (Value $first "KGS_STATE_SAVED (.*)") -split " "
$id, $skin, $key = $started -split " "
# Steam Cloud の同期を模す: ゲームの開始の file とゲームのセーブだけを、一度も開いていない profile へ写す。
New-Item -ItemType Directory -Force $restoreDir | Out-Null
Copy-Item (Join-Path $stateDir "game-account.json") $restoreDir -ErrorAction SilentlyContinue
Copy-Item "$stateDir.cyber-disc-arena.json" "$restoreDir.cyber-disc-arena.json" -ErrorAction SilentlyContinue
# B がこの端末で遊んだ別の進行（ゲームのセーブにある）を足す。
$otherId = [guid]::NewGuid().ToString()
$savePath = "$stateDir.cyber-disc-arena.json"
$save = Get-Content $savePath -Raw | ConvertFrom-Json
$save | Add-Member -NotePropertyName $otherId -NotePropertyValue ([pscustomobject]@{ Pubkey = $saved[3]; Skin = "gold" })
[IO.File]::WriteAllText($savePath, ($save | ConvertTo-Json -Depth 4), (New-Object System.Text.UTF8Encoding $false))
$second = Start-State "2" $stateDir "-arena-merge-to $($saved[2]) -arena-merge-pubkey $($saved[3])"
$resumed = Value $second "KGS_STATE2 (\S+ \S+ \S+)"
$offered = Value $second "KGS_STATE_OFFERED (\S+ \S+)"
$cancelled = Value $second "KGS_STATE_CANCELLED (\S+ \S+)"
$idle = Value $first "KGS_LOBBY_IDLE (\S+)"
$merged = Value $second "KGS_STATE_MERGED (\S+ \S+ \S+)"
$kept = (Get-Content $savePath -Raw | ConvertFrom-Json).$id.Skin
$third = Start-State "3" $restoreDir
$restored = Value $third "KGS_STATE3 (\S+ \S+ \S+)"
$lobbyClosed = (Select-String -Path $players["host"].Log, $consented.Log -Pattern "KGS_LOBBY_BEFORE_START False").Count -eq 2
$stateOk = ($skin -eq "standard") -and ($saved[0] -eq "neon-red") -and ($saved[1] -eq "on") -and ($resumed -eq "$id neon-red $key") -and
    ($offered -eq "merge-confirm $otherId") -and ($cancelled -eq "playing True") -and ($idle -eq "True") -and ($consented.Exit -eq 0) -and ($merged -eq "$otherId gold $($saved[3])") -and ($kept -eq "neon-red") -and
    ($restored -eq "$id neon-red $key") -and $lobbyClosed -and ($first.Exit -eq 0) -and ($second.Exit -eq 0) -and ($third.Exit -eq 0)
Write-Output "Player state ($Backend): started=[$started] saved=[$($saved[0..1] -join ' ')] resumed=[$resumed] idle_before_start=$idle cancelled=[$cancelled] offered=[$offered] merged=[$merged] kept=$kept restored=[$restored] lobby_closed_before_start=$lobbyClosed exits=$($first.Exit),$($second.Exit),$($third.Exit) ok=$stateOk"
$failure = Select-String -Path $first.Log, $second.Log, $third.Log -Pattern "KGS_FAILED.*" | Select-Object -First 1
if ($failure) { Write-Output "  $($failure.Line)" }
if ($failed -or -not $ok -or -not $stateOk) { exit 1 }
