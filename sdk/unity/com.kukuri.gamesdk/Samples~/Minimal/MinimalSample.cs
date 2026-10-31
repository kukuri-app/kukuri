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
/// §4.2）。Steam で配るゲームは「Steam Cloud に保存」で、ゲームで作ったアカウントと game player を Steam Cloud に置き、
/// 別の PC でも同じアカウントで再開できる（§4.3。同期はゲームの Steam Auto-Cloud の設定が行う）。「既存のアカウントへ
/// まとめる」は、このゲームの進行を既存の kukuri アカウントへ付け替え、以後はそのアカウントで遊ぶ（§4.2）。
public sealed class MinimalSample : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;
    /// このゲームの application ID。1 つの profile は 1 つのゲームの game player だけを持つ。
    [SerializeField] string applicationId = "com.example.minimal";
    /// Steam で動くとき true にする（「Steam Cloud に保存」を出す）。
    [SerializeField] bool steamCloud;

    // 秘密の値は scene・prefab・PlayerPrefs に残さない（serialize しない）。
    [NonSerialized] string exportText = "";
    [NonSerialized] string passphrase = "";

    /// 今の画面（`loading`・`consent`・`start`・`transfer`・`recovery`・`playing`・`merge`・`merge-confirm`）。
    public string Screen { get; private set; } = "loading";
    /// 始めた game player の ID とアカウントの公開鍵（`playing` のとき）。ゲームはセーブの参照に game player ID を使う。
    public string GamePlayerId { get; private set; } = "";
    public string PublicKey { get; private set; } = "";

    /// まとめた後に、前のアカウントのフォロー・ブロック・ミュートを付け直している途中か。
    public bool Copying { get; private set; }

    /// Steam Cloud への保存の状態（`get_cloud_account` の `state`。`off`・`on`・`other_account` など）。
    public string CloudState { get; private set; } = "off";
    /// 移行の状態（`get_account_transfer_status`。`transfer` の画面で読み直す）。
    public JObject Transfer { get; private set; }

    string message = "";
    JArray documents;
    /// 移行の画面に入るたびに増やす（「戻る」の後に入り直したとき、前の読み直しを止める）。
    int transferRun;
    /// 移行の画面が、このゲームのアカウントを kukuri app へ移す向き（「kukuriで続ける」）か。
    bool handingOff;
    /// 移行の画面が、まとめる先のアカウントを受ける向き（「既存のアカウントへまとめる」）か。
    bool merging;
    /// まとめる先の候補（profile にある、今のアカウント以外）と、選んだアカウント。
    JArray mergeCandidates = new JArray();
    string mergeTarget = "";

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
        Play(started.Data);
        message = "";
        await ReadCloud();
    }

    void Play(JToken player)
    {
        GamePlayerId = (string)player["game_player_id"];
        PublicKey = (string)player["public_key"];
        Copying = (bool)player["copying"];
        Screen = "playing";
        Debug.Log($"kukuri sample: playing game_player_id={GamePlayerId}");
        if (Copying)
            _ = WatchCopy();
    }

    /// 付け直しが終わるまで読み直す（他の画面を開いている間も）。
    async Task WatchCopy()
    {
        while (Copying)
        {
            await Task.Delay(2000);
            var read = await runner.Client.SendAsync("get_game_player", ThisGame);
            Copying = read.Ok && read.Data is JObject && (bool)read.Data["copying"];
        }
    }

    /// 「既存のアカウントへまとめる」。説明と、まとめる先の選択を出す。
    public async Task EnterMerge()
    {
        await ListCandidates();
        message = "";
        Screen = "merge";
    }

    async Task ListCandidates()
    {
        var listed = await runner.Client.SendAsync("list_accounts");
        mergeCandidates = new JArray();
        foreach (var account in listed.Data?["accounts"] as JArray ?? new JArray())
            if ((string)account["pubkey"] != PublicKey)
                mergeCandidates.Add(account);
    }

    /// 「kukuri app から受けてまとめる」。まとめる先を移行で受ける（受けた後に残す進行を選ぶ）。
    public void ReceiveForMerge() => EnterTransfer(false, true);

    async Task ChooseMerge(string accountId)
    {
        // 移行で受けたばかりのアカウントも名前を示せるよう、候補を読み直す。
        await ListCandidates();
        mergeTarget = accountId;
        Screen = "merge-confirm";
    }

    /// 選んだアカウントの表示名（label が無ければ公開鍵の先頭）。
    string MergeTargetName()
    {
        foreach (var account in mergeCandidates)
            if ((string)account["id"] == mergeTarget)
                return (string)account["label"] ?? ((string)account["pubkey"]).Substring(0, 12);
        return mergeTarget.Substring(0, Math.Min(12, mergeTarget.Length));
    }

    /// 確定。今の進行（この game player）を残して、選んだアカウントへまとめる。
    public async Task Merge()
    {
        var request = ThisGame;
        request["account_id"] = mergeTarget;
        var merged = await runner.Client.SendAsync("merge_game_player", request);
        if (!merged.Ok)
        {
            // 失敗の応答でも、結び付きを書き換えた後なら確定は済んでいる。開始し直すと、結び付いたアカウントへ切り替え、
            // 付け直しを続ける（確定の前なら今のアカウントのまま同じものを返す）。
            var current = await runner.Client.SendAsync("start_game_player", ThisGame);
            if (current.Ok && current.Data is JObject player && (string)player["public_key"] != PublicKey)
            {
                message = "まとめました。以後はまとめた先のアカウントで投稿・参加します";
                Play(player);
                return;
            }
            message = (string)merged.Raw?["error"]?["details"]?["reason"] == "merge_in_progress"
                ? "前のまとめの付け直しが終わってから、やり直してください"
                : $"まとめられません: {merged.ErrorCode}";
            Screen = "playing";
            return;
        }
        message = "まとめました。以後はまとめた先のアカウントで投稿・参加します";
        Play(merged.Data);
        await ReadCloud();
    }

    async Task ReadCloud()
    {
        var read = await runner.Client.SendAsync("get_cloud_account", ThisGame);
        CloudState = read.Ok ? (string)read.Data["state"] : CloudState;
    }

    /// 「Steam Cloud に保存」（`enabled: true`）・「保存をやめる」（`false`）。
    public async Task SetCloud(bool enabled)
    {
        var request = ThisGame;
        request["enabled"] = enabled;
        var set = await runner.Client.SendAsync("set_cloud_account", request);
        message = set.Ok ? "" : (string)set.Raw?["error"]?["details"]?["reason"] switch
        {
            "not_game_account" => "このゲームで作ったアカウントだけを Steam Cloud に保存できます",
            "integration_required" => "Steam Cloud には別の PC で保存したアカウントがあります",
            _ => $"保存を変えられません: {set.ErrorCode}",
        };
        await ReadCloud();
    }

    /// 「kukuriアカウントを使う」。移行で受ける前の説明と「貼り付け」の画面を出す。
    public void UseKukuriAccount() => EnterTransfer(false);

    void EnterTransfer(bool handOff, bool merge = false)
    {
        message = "";
        Transfer = null;
        transferRun++;
        handingOff = handOff;
        merging = merge;
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
            if (state == "completed" && merging)
            {
                // 受けたアカウントは profile に加わるだけなので、まとめる先として選ぶ。
                var received = (string)Transfer["account_id"];
                Transfer = null;
                await ChooseMerge(received);
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
                if (handingOff || merging)
                    Screen = "playing";
                return;
            }
            await Task.Delay(500);
        }
    }

    async Task LeaveTransfer()
    {
        Screen = handingOff || merging ? "playing" : "start";
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
                else if (merging)
                {
                    GUILayout.Label("kukuri app のアカウントメニューの「アカウント追加」→「別の端末へ移す」で「リンクをコピー」してから、貼り付けてください。");
                    if (GUILayout.Button("貼り付け", GUILayout.Height(40)))
                        _ = Paste();
                }
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
                if (Copying)
                    GUILayout.Label("前のアカウントのフォロー・ブロック・ミュートを付け直しています…");
                if (GUILayout.Button("kukuriで続ける"))
                    _ = HandOff();
                if (GUILayout.Button("既存のアカウントへまとめる"))
                    _ = EnterMerge();
                if (steamCloud && CloudState == "on")
                {
                    GUILayout.Label("Steam Cloud に保存しています。同じ Steam アカウントなら別の PC でも再開できます。");
                    if (GUILayout.Button("保存をやめる"))
                        _ = SetCloud(false);
                }
                else if (steamCloud && CloudState == "off")
                {
                    GUILayout.Label("Steam Cloud に保存すると、同じ Steam アカウントでログインした別の PC でも、このアカウントと進行で再開できます。Steam アカウントを使える人は、このゲームの kukuri のアカウントも使えます。保存をやめると別の PC では再開できません（この PC のアカウントと進行は残り、すでに Steam に渡った写しは取り消せません）。");
                    if (GUILayout.Button("Steam Cloud に保存"))
                        _ = SetCloud(true);
                }
                break;
            case "merge":
                GUILayout.Label("このゲームの進行を、持っている kukuri アカウントへまとめます。");
                GUILayout.Label("・以後はまとめた先のアカウントで投稿・参加し、ゲームの進行はそのアカウントに付きます。");
                GUILayout.Label("・このゲームのアカウントの過去の投稿は、このゲームのアカウントのまま残ります。他の人からは別の人に見え、フォロワーは移りません。");
                GUILayout.Label("・フォロー・ブロック・ミュートはまとめた先で付け直します。フォローとブロックは公開されるので、関係を推測されることがあります。");
                GUILayout.Label("・参加している private channel には、まとめた先のアカウントを招待し直してもらってください。");
                GUILayout.Label("・まとめた後、このゲームでは前のアカウントを使えません。前のアカウントの private channel への招待、投稿の取り下げ、プロフィールの変更をしたいときは、先に「kukuriで続ける」で kukuri app へ移してください。この端末が担っていた private channel の鍵の更新は止まります。");
                GUILayout.Label("・まとめた先のアカウントの鍵がこの端末に残り、kukuri app から後で取り消せません。");
                if (steamCloud && CloudState == "on")
                    GUILayout.Label("・Steam Cloud への保存はやめます（まとめた先の鍵は Steam に置きません）。");
                foreach (var account in mergeCandidates)
                    if (GUILayout.Button($"{(string)account["label"] ?? ((string)account["pubkey"]).Substring(0, 12)} へまとめる"))
                        _ = ChooseMerge((string)account["id"]);
                if (GUILayout.Button("kukuri app から受けてまとめる"))
                    ReceiveForMerge();
                if (GUILayout.Button("戻る"))
                    Screen = "playing";
                break;
            case "merge-confirm":
                GUILayout.Label($"まとめる先: {MergeTargetName()}");
                GUILayout.Label("残す進行: このゲームの今の進行（まとめた先のアカウントで続けます）");
                if (GUILayout.Button("まとめる", GUILayout.Height(40)))
                    _ = Merge();
                if (GUILayout.Button("やめる"))
                    Screen = "playing";
                break;
        }
        if (message.Length > 0)
            GUILayout.Label(message);
        GUILayout.EndArea();
    }
}
