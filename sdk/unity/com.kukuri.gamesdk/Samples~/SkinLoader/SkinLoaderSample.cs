using Kukuri.GameSdk.Assets;
using UnityEngine;

/// スキンのローダーの sample（#1980 AC-2）。KukuriRunner と KukuriSkinLoader と同じ GameObject に付け、loader に topic・
/// 信頼する発行者の公開鍵・application ID・asset ID を設定する。適用中の版と、新しい版を読めなかった理由を表示する。
/// 新しい版が発行されると、loader が自動で取得・検証して、再起動せずに差し替える。
public sealed class SkinLoaderSample : MonoBehaviour
{
    [SerializeField] KukuriSkinLoader loader;

    string status = "取得中";

    void OnEnable()
    {
        loader.Applied += OnApplied;
        loader.Failed += OnFailed;
    }

    void OnDisable()
    {
        loader.Applied -= OnApplied;
        loader.Failed -= OnFailed;
    }

    void OnApplied(long revision) => status = $"版 {revision} を表示しています";

    void OnFailed(string reason) => status = loader.AppliedRevision > 0
        ? $"新しい版を読めませんでした（{reason}）。版 {loader.AppliedRevision} を表示しています"
        : $"スキンを取得できませんでした（{reason}）";

    void OnGUI()
    {
        GUI.Label(new Rect(16, 16, 640, 24), status);
    }
}
