using System;
using System.Text;
using System.Threading.Tasks;
using Kukuri.GameSdk;
using Newtonsoft.Json.Linq;
using UnityEngine;

/// 最小 sample。KukuriRunner と同じ GameObject に付ける。起動の状態・利用規約への同意・新規アカウント・
/// kukuri app で暗号化 export したアカウントの明示 import と、接続の状態を表示する（ADR 0070 §4）。
public sealed class MinimalSample : MonoBehaviour
{
    [SerializeField] KukuriRunner runner;

    // 秘密の値は scene・prefab・PlayerPrefs に残さない（serialize しない）。
    [NonSerialized] string exportText = "";
    [NonSerialized] string passphrase = "";

    string status = "（未取得）";
    string identity = "";
    string message = "";
    JArray documents;

    void Start()
    {
        _ = Refresh();
    }

    async Task Refresh()
    {
        var client = runner.Client;
        if (client == null)
        {
            status = runner.OpenError != null ? $"起動できません: {runner.OpenError.Code}" : "未接続";
            return;
        }
        var startup = await client.SendAsync("get_desktop_startup_status");
        status = startup.Ok ? (string)startup.Data["status"] : $"失敗: {startup.ErrorCode}";
        Debug.Log($"kukuri sample: status={status}");
        if (status == "consent_required")
        {
            var consent = await client.SendAsync("get_app_consent_status");
            documents = consent.Data?["documents"] as JArray;
        }
        if (status == "ready")
        {
            var sync = await client.SendAsync("get_sync_status");
            identity = sync.Ok ? (string)sync.Data["local_author_pubkey"] : "";
        }
    }

    async Task Accept()
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
        message = response.Ok ? "同意しました。新しいアカウントで始めます" : $"同意できません: {response.ErrorCode}";
        await Refresh();
    }

    async Task Import()
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
        message = switched.Ok ? "取り込んだアカウントに切り替えました" : $"切り替えられません: {switched.ErrorCode}";
        await Refresh();
    }

    void OnGUI()
    {
        GUILayout.BeginArea(new Rect(16, 16, 520, 600), GUI.skin.box);
        GUILayout.Label($"状態: {status}");
        if (identity.Length > 0)
            GUILayout.Label($"公開鍵: {identity}");
        if (GUILayout.Button("状態を読み直す"))
            _ = Refresh();
        if (status == "consent_required" && documents != null)
        {
            GUILayout.Label("利用規約とプライバシーポリシーへの同意が必要です（18 歳以上）。");
            foreach (var document in documents)
                GUILayout.Label($"・{document["slug"]}（版 {document["currentVersion"]}）");
            if (GUILayout.Button("同意して新しいアカウントで始める"))
                _ = Accept();
        }
        if (status == "ready")
        {
            GUILayout.Label("kukuri app で暗号化 export したアカウントを使う（このゲームがそのアカウントで投稿・署名できるようになります）");
            exportText = GUILayout.TextArea(exportText, GUILayout.Height(60));
            passphrase = GUILayout.PasswordField(passphrase, '*');
            if (GUILayout.Button("取り込んで切り替える"))
                _ = Import();
        }
        if (message.Length > 0)
            GUILayout.Label(message);
        GUILayout.EndArea();
    }
}
