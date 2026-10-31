using System;
using System.Text;
using System.Threading.Tasks;
using Kukuri.GameSdk;
using Newtonsoft.Json.Linq;
using UnityEngine;

/// 最小 sample。KukuriRunner と同じ GameObject に付ける。利用規約への同意 →「はじめる」で、同意で作られたアカウントに
/// game player を結び付けて始め、次の起動からは同じ game player とアカウントで再開する（ADR 0070 §4.1）。「はじめる」の
/// 前は SNS・ロビーを開かない。kukuri app で暗号化 export したアカウントの取り込みは、端末を失ったときの「復旧」の上級の
/// 操作として開始の画面から分ける。
public sealed class MinimalSample : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;
    /// このゲームの application ID。1 つの profile は 1 つのゲームの game player だけを持つ。
    [SerializeField] string applicationId = "com.example.minimal";

    // 秘密の値は scene・prefab・PlayerPrefs に残さない（serialize しない）。
    [NonSerialized] string exportText = "";
    [NonSerialized] string passphrase = "";

    /// 今の画面（`loading`・`consent`・`start`・`recovery`・`playing`）。
    public string Screen { get; private set; } = "loading";
    /// 始めた game player の ID とアカウントの公開鍵（`playing` のとき）。ゲームはセーブの参照に game player ID を使う。
    public string GamePlayerId { get; private set; } = "";
    public string PublicKey { get; private set; } = "";

    string message = "";
    JArray documents;

    JObject ThisGame => new JObject { ["application_id"] = applicationId };

    void Start()
    {
        _ = Refresh();
    }

    async Task Refresh()
    {
        var client = runner.Client;
        if (client == null)
        {
            message = runner.OpenError != null ? $"起動できません: {runner.OpenError.Code}" : "未接続";
            return;
        }
        var startup = await client.SendAsync("get_desktop_startup_status");
        var status = startup.Ok ? (string)startup.Data["status"] : $"失敗: {startup.ErrorCode}";
        Debug.Log($"kukuri sample: status={status}");
        if (status == "consent_required")
        {
            var consent = await client.SendAsync("get_app_consent_status");
            documents = consent.Data?["documents"] as JArray;
            Screen = "consent";
            return;
        }
        if (status != "ready")
        {
            Screen = "loading";
            return;
        }
        var saved = await client.SendAsync("get_game_player", ThisGame);
        if (!saved.Ok)
        {
            message = $"読めません: {saved.ErrorCode}";
            return;
        }
        // 保存済みなら、同意と「はじめる」を繰り返さずに再開する。
        if (saved.Data is JObject)
            await Begin();
        else
            Screen = "start";
    }

    public async Task Accept()
    {
        var accepted = new JArray();
        foreach (var document in documents)
            accepted.Add(new JObject { ["slug"] = document["slug"], ["version"] = document["currentVersion"] });
        var response = await runner.Client.SendAsync("accept_app_consents", new JObject
        {
            ["documents"] = accepted,
            ["language"] = "ja",
            ["age_attested"] = true,
        });
        message = response.Ok ? "" : $"同意できません: {response.ErrorCode}";
        await Refresh();
    }

    /// 「はじめる」。保存済みの game player なら、そのアカウントで再開する。
    public async Task Begin()
    {
        var started = await runner.Client.SendAsync("start_game_player", ThisGame);
        if (!started.Ok)
        {
            message = $"始められません: {started.ErrorCode}";
            return;
        }
        GamePlayerId = (string)started.Data["game_player_id"];
        PublicKey = (string)started.Data["public_key"];
        message = "";
        Screen = "playing";
        Debug.Log($"kukuri sample: playing game_player_id={GamePlayerId}");
    }

    async Task Recover()
    {
        var secret = Encoding.UTF8.GetBytes(new JObject
        {
            ["export"] = exportText.Trim(),
            ["passphrase"] = passphrase,
        }.ToString());
        var imported = await runner.Client.SendAsync("import_account_key", new JObject(), secret);
        passphrase = "";
        if (!imported.Ok)
        {
            message = $"取り込めません: {imported.ErrorCode}";
            return;
        }
        exportText = "";
        var switched = await runner.Client.SendAsync("switch_account",
            new JObject { ["account_id"] = imported.Data["id"] });
        message = switched.Ok ? "復旧したアカウントに切り替えました" : $"切り替えられません: {switched.ErrorCode}";
        await Refresh();
    }

    void OnGUI()
    {
        GUILayout.BeginArea(new Rect(16, 16, 520, 600), GUI.skin.box);
        switch (Screen)
        {
            case "loading":
                GUILayout.Label("起動しています…");
                if (GUILayout.Button("状態を読み直す"))
                    _ = Refresh();
                break;
            case "consent":
                GUILayout.Label("利用規約とプライバシーポリシーへの同意が必要です（18 歳以上）。");
                foreach (var document in documents ?? new JArray())
                    GUILayout.Label($"・{document["slug"]}（版 {document["currentVersion"]}）");
                if (GUILayout.Button("同意する"))
                    _ = Accept();
                break;
            case "start":
                GUILayout.Label("登録なしで、このゲーム用の kukuri アカウントで始めます。");
                if (GUILayout.Button("はじめる", GUILayout.Height(48)))
                    _ = Begin();
                GUILayout.Space(24);
                if (GUILayout.Button("復旧（上級）"))
                    Screen = "recovery";
                break;
            case "recovery":
                GUILayout.Label("端末を失ったときの復旧: kukuri app で暗号化 export したアカウントを取り込みます（このゲームがそのアカウントで投稿・署名できるようになります）");
                exportText = GUILayout.TextArea(exportText, GUILayout.Height(60));
                passphrase = GUILayout.PasswordField(passphrase, '*');
                if (GUILayout.Button("取り込んで切り替える"))
                    _ = Recover();
                if (GUILayout.Button("戻る"))
                    Screen = "start";
                break;
            case "playing":
                GUILayout.Label("プレイ中");
                GUILayout.Label($"game player: {GamePlayerId}");
                GUILayout.Label($"公開鍵: {PublicKey}");
                break;
        }
        if (message.Length > 0)
            GUILayout.Label(message);
        GUILayout.EndArea();
    }
}
