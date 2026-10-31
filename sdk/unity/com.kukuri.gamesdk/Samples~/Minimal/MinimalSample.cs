using System;
using System.Text;
using System.Threading.Tasks;
using Kukuri.GameSdk;
using Newtonsoft.Json.Linq;
using UnityEngine;

/// 最小 sample。KukuriRunner と同じ GameObject に付ける。利用規約への同意 →「はじめる」で、同意で作られたアカウントに
/// game player を結び付けて始め、次の起動からは同じ game player とアカウントで再開する（ADR 0070 §4.1）。「はじめる」の
/// 前は SNS・ロビーを開かない。既存の kukuri アカウントを使う人は「kukuriアカウントを使う」で、kukuri app の「別の端末へ
/// 移す」のリンクを貼り付け、両方に出る確認コードの一致を承認して移行で受ける（鍵・export の文字列は扱わない）。kukuri app で
/// 暗号化 export したアカウントの取り込みは、端末を失ったときの「復旧」の上級の操作として開始の画面から分ける。
/// プレイ中の「kukuriで続ける」は、このゲームのアカウントを同じ鍵のまま kukuri app へ移すリンクをコピーする（ADR 0070
/// §4.2）。
public sealed class MinimalSample : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;
    /// このゲームの application ID。1 つの profile は 1 つのゲームの game player だけを持つ。
    [SerializeField] string applicationId = "com.example.minimal";

    // 秘密の値は scene・prefab・PlayerPrefs に残さない（serialize しない）。
    [NonSerialized] string exportText = "";
    [NonSerialized] string passphrase = "";

    /// 今の画面（`loading`・`consent`・`start`・`transfer`・`recovery`・`playing`）。
    public string Screen { get; private set; } = "loading";
    /// 始めた game player の ID とアカウントの公開鍵（`playing` のとき）。ゲームはセーブの参照に game player ID を使う。
    public string GamePlayerId { get; private set; } = "";
    public string PublicKey { get; private set; } = "";

    /// 移行の状態（`get_account_transfer_status`。`transfer` の画面で読み直す）。
    public JObject Transfer { get; private set; }

    string message = "";
    JArray documents;
    /// 移行の画面に入るたびに増やす（「戻る」の後に入り直したとき、前の読み直しを止める）。
    int transferRun;
    /// 移行の画面が、このゲームのアカウントを kukuri app へ移す向き（「kukuriで続ける」）か。
    bool handingOff;

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

    /// 「kukuriアカウントを使う」。移行で受ける前の説明と「貼り付け」の画面を出す。
    public void UseKukuriAccount() => EnterTransfer(false);

    void EnterTransfer(bool handOff)
    {
        message = "";
        Transfer = null;
        transferRun++;
        handingOff = handOff;
        Screen = "transfer";
    }

    /// 「kukuriで続ける」。移行のリンクを作って clipboard へコピーし、kukuri app が受けて終わるまで状態を読み直す。
    public async Task HandOff()
    {
        EnterTransfer(true);
        var invite = await runner.Client.SendAsync("create_account_transfer_invite", new JObject(), acceptsSecretOutput: true);
        if (!invite.Ok || invite.Secret == null)
        {
            Screen = "playing";
            message = $"リンクを作れません: {invite.ErrorCode}";
            return;
        }
        GUIUtility.systemCopyBuffer = Encoding.UTF8.GetString(invite.Secret);
        await Follow(transferRun);
    }

    /// 「貼り付け」。clipboard の移行のリンクを秘密の入力で渡し、終わるまで状態を読み直す。
    public async Task Paste()
    {
        var link = GUIUtility.systemCopyBuffer ?? "";
        if (link.Trim().Length == 0)
        {
            message = "kukuri app で「リンクをコピー」してから貼り付けてください";
            return;
        }
        var opened = await runner.Client.SendAsync("open_account_transfer", new JObject(), Encoding.UTF8.GetBytes(link));
        message = opened.Ok ? "" : "リンクを読めません。kukuri app でもう一度リンクをコピーしてください";
        if (opened.Ok)
            await Follow(transferRun);
    }

    /// 承認（一致する）・拒否（一致しない）。結果は `Follow` が読む。
    public Task Decide(bool accept) =>
        runner.Client.SendAsync("decide_account_transfer", new JObject { ["accept"] = accept });

    async Task Follow(int run)
    {
        while (Screen == "transfer" && run == transferRun)
        {
            var status = await runner.Client.SendAsync("get_account_transfer_status");
            Transfer = status.Data as JObject;
            var state = (string)Transfer?["state"];
            // 移す向きは、アカウントを送り終えたらプレイに戻る。kukuri app が投稿の履歴を選んでいれば、履歴は背景で送り続ける
            // （取り消さない）。
            if (handingOff && (state == "completed" || state == "history"))
            {
                message = "kukuri app へ移しました。kukuri app で「このアカウントを使う」を押すと、同じアカウントで会話を続けられます";
                Transfer = null;
                Screen = "playing";
                return;
            }
            if (state == "completed")
            {
                // 移行は受けたアカウントを足すだけなので、切り替えてから開始の画面に戻る。
                var switched = await runner.Client.SendAsync("switch_account",
                    new JObject { ["account_id"] = Transfer["account_id"] });
                var reason = (string)switched.Raw?["error"]?["details"]?["reason"];
                message = switched.Ok ? "kukuri アカウントを受け取りました"
                    : reason == "integration_required" ? "このゲームは別のアカウントで始めています。統合が必要です"
                    : $"切り替えられません: {switched.ErrorCode}";
                Transfer = null;
                await Refresh();
                return;
            }
            if (state == "failed")
            {
                message = (string)Transfer["reason"] switch
                {
                    "expired" => "リンクの期限が切れました。もう一度やり直してください",
                    "rejected" => "確認コードが一致しなかったので、やめました",
                    _ => "移せませんでした。もう一度試してください",
                };
                Transfer = null;
                if (handingOff)
                    Screen = "playing";
                return;
            }
            await Task.Delay(500);
        }
    }

    async Task LeaveTransfer()
    {
        Screen = handingOff ? "playing" : "start";
        Transfer = null;
        transferRun++;
        await runner.Client.SendAsync("cancel_account_transfer");
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
                if (GUILayout.Button("kukuriアカウントを使う"))
                    UseKukuriAccount();
                GUILayout.Space(24);
                if (GUILayout.Button("復旧（上級）"))
                    Screen = "recovery";
                break;
            case "transfer":
                var state = (string)Transfer?["state"];
                if (state == "confirming")
                {
                    GUILayout.Label("kukuri app に同じ確認コードが出ていることを確かめてください。");
                    GUILayout.Label((string)Transfer["code"], GUILayout.Height(40));
                    if ((bool)Transfer["local_accepted"])
                        GUILayout.Label("kukuri app の確認を待っています…");
                    else
                    {
                        if (GUILayout.Button("一致する"))
                            _ = Decide(true);
                        if (GUILayout.Button("一致しない"))
                            _ = Decide(false);
                    }
                }
                else if (handingOff)
                {
                    GUILayout.Label("このゲームのアカウントを、同じ鍵のまま kukuri app へ移します。");
                    GUILayout.Label("リンクをコピーしました。kukuri app のアカウントメニューの「アカウント追加」→「別の端末から移す」に貼り付けてください。リンクは 5 分間、1 回だけ使えます。他の人に見せないでください。");
                }
                else if (state != null)
                    GUILayout.Label("kukuri app とつないでいます…");
                else
                {
                    GUILayout.Label("持っている kukuri アカウントでこのゲームを始めます。");
                    GUILayout.Label("・このゲームがそのアカウントで投稿・署名できるようになります。");
                    GUILayout.Label("・この端末にアカウントの鍵が残り、kukuri app から後で取り消せません。やめるときは、このゲームのデータから外します。");
                    GUILayout.Label("kukuri app のアカウントメニューの「アカウント追加」→「別の端末へ移す」で「リンクをコピー」してから、貼り付けてください。");
                    if (GUILayout.Button("貼り付け", GUILayout.Height(40)))
                        _ = Paste();
                }
                if (GUILayout.Button("戻る"))
                    _ = LeaveTransfer();
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
                if (GUILayout.Button("kukuriで続ける"))
                    _ = HandOff();
                break;
        }
        if (message.Length > 0)
            GUILayout.Label(message);
        GUILayout.EndArea();
    }
}
