using System;
using System.IO;
using System.Threading.Tasks;
using GLTFast;
using GLTFast.Loading;
using Newtonsoft.Json.Linq;
using UnityEngine;

namespace Kukuri.GameSdk.Assets
{
    /// 信頼する発行者が署名したゲームのアセット（スキン）を P2P で取得して表示し、新しい版が届いたら再起動せずに差し替える
    /// （ADR 0070 §5.3、#1980 AC-2）。glTFast を入れた project だけで compile される。
    ///
    /// SDK が検証した file だけを読む（未検証の bytes は受け取らない）。.glb は glTFast で `parent` の下に置き、PNG・JPEG は
    /// `textureTarget` の material の texture にし、AssetBundle は自分の Unity の版と build の対象に合うときだけ最初の
    /// GameObject を置く。新しい版を読めなかったときは今のスキンを保ち、差し替えたら前の版の資源を解放する。
    [RequireComponent(typeof(TimeBudgetPerFrameDeferAgent))]
    public sealed class KukuriSkinLoader : MonoBehaviour
    {
        [SerializeField] KukuriRunner runner;
        [Tooltip("アセットの更新の知らせが届く、ゲームの公開 topic")]
        [SerializeField] string topic = "";
        [Tooltip("信頼する発行者の公開鍵（Unity の設定に固定し、秘密鍵は持たない）")]
        [SerializeField] string publisherPubkey = "";
        [SerializeField] string applicationId = "";
        [Tooltip("Unity Editor からの発行のときに登録した asset ID")]
        [SerializeField] string assetId = "";
        [Tooltip(".glb と AssetBundle を置く親（空ならこの GameObject）")]
        [SerializeField] Transform parent;
        [Tooltip("PNG・JPEG を texture にする renderer")]
        [SerializeField] Renderer textureTarget;

        KukuriClient client;
        KukuriClient subscribed;
        /// PNG・JPEG を貼るために複製した material（loader が持ち、破棄する）。
        Material ownMaterial;
        bool loading;
        bool again;
        GltfImport currentImport;
        Texture2D currentTexture;
        AssetBundle currentBundle;

        /// 適用中の版（まだ無ければ 0）。
        public long AppliedRevision { get; private set; }

        /// 表示中の .glb・AssetBundle の root（PNG・JPEG では null）。
        public GameObject Instance { get; private set; }

        /// 差し替えた（版）。
        public event Action<long> Applied;

        /// 新しい版を読めなかった（理由）。今のスキンはそのまま。
        public event Action<string> Failed;

        /// 設定すると（`runner` を使うときは runner が開いた後の最初の frame に）購読を始め、最新の版を読む。
        public KukuriClient Client
        {
            get => client ?? (runner != null ? runner.Client : null);
            set
            {
                client = value;
                Subscribe();
            }
        }

        public string Topic { get => topic; set => topic = value ?? ""; }
        public string PublisherPubkey { get => publisherPubkey; set => publisherPubkey = value ?? ""; }
        public string ApplicationId { get => applicationId; set => applicationId = value ?? ""; }
        public string AssetId { get => assetId; set => assetId = value ?? ""; }

        void Update() => Subscribe();

        void Subscribe()
        {
            if (Client == null || subscribed == Client)
                return;
            if (subscribed != null)
                subscribed.EventReceived -= OnEvent;
            subscribed = Client;
            subscribed.EventReceived += OnEvent;
            _ = LoadLatestAsync();
        }

        void OnDestroy()
        {
            if (subscribed != null)
                subscribed.EventReceived -= OnEvent;
            Release(Instance, currentImport, currentTexture, currentBundle);
            if (ownMaterial != null)
                DestroyNow(ownMaterial);
        }

        void OnEvent(JObject @event)
        {
            if ((string)@event["type"] == "game_asset_updated" &&
                (string)@event["publisher_pubkey"] == publisherPubkey &&
                (string)@event["application_id"] == applicationId &&
                (string)@event["asset_id"] == assetId &&
                ((long?)@event["revision"] ?? 0) > AppliedRevision)
                _ = LoadLatestAsync();
        }

        /// 最新の検証済みの版を得て、適用中より新しければ差し替える。読み込み中に呼ばれたら、終わった後にもう一度読む。
        public async Task LoadLatestAsync()
        {
            if (loading)
            {
                again = true;
                return;
            }
            loading = true;
            try
            {
                do
                {
                    again = false;
                    await LoadOnceAsync();
                } while (again);
            }
            finally
            {
                loading = false;
            }
        }

        async Task LoadOnceAsync()
        {
            var payload = new JObject
            {
                ["topic"] = topic,
                ["publisher_pubkey"] = publisherPubkey,
                ["application_id"] = applicationId,
                ["asset_id"] = assetId,
            };
            // 大きなアセットの取得は長くかかりうる（上限 5 分）。
            var response = await Client.SendAsync("fetch_game_asset", payload, timeoutMilliseconds: 300_000);
            if (this == null)
                return;
            if (!response.Ok)
            {
                Failed?.Invoke((string)response.Raw["error"]?["details"]?["reason"] ?? response.ErrorCode);
                return;
            }
            var revision = (long)response.Data["revision"];
            if (revision <= AppliedRevision)
                return;
            var path = (string)response.Data["path"];
            var format = (JObject)response.Data["format"];
            string reason;
            try
            {
                reason = await ApplyAsync(path, format);
            }
            catch (Exception error)
            {
                // file を開けない（新しい版の適用で前の file が消えた等）・読み込みの例外。今のスキンはそのまま。
                Debug.LogWarning($"kukuri: the skin could not be loaded: {error.Message}");
                reason = "invalid_asset";
            }
            if (this == null)
                return;
            if (reason != null)
            {
                Failed?.Invoke(reason);
                return;
            }
            AppliedRevision = revision;
            Applied?.Invoke(revision);
        }

        /// 新しい版を読んでから入れ替える。読めなければ理由を返し、今のスキンを変えない。
        async Task<string> ApplyAsync(string path, JObject format)
        {
            var root = parent != null ? parent : transform;
            switch ((string)format["kind"])
            {
                case "glb":
                {
                    // .glb の外の URI（buffer・image）は読まない。署名と hash で確かめた file の中だけを使う。
                    var import = new GltfImport(new NoDownloads(), GetComponent<TimeBudgetPerFrameDeferAgent>());
                    var holder = new GameObject("skin");
                    holder.SetActive(false);
                    holder.transform.SetParent(root, false);
                    try
                    {
                        if (!await import.LoadFile(path) || !await import.InstantiateMainSceneAsync(holder.transform) ||
                            this == null)
                        {
                            Release(holder, import, null, null);
                            return "invalid_asset";
                        }
                    }
                    catch
                    {
                        Release(holder, import, null, null);
                        throw;
                    }
                    Swap(holder, import, null, null);
                    return null;
                }
                case "png":
                case "jpeg":
                {
                    if (textureTarget == null)
                        return "no_texture_target";
                    var bytes = await File.ReadAllBytesAsync(path);
                    if (this == null)
                        return "destroyed";
                    var texture = new Texture2D(2, 2);
                    if (!texture.LoadImage(bytes))
                    {
                        Release(null, null, texture, null);
                        return "invalid_asset";
                    }
                    if (ownMaterial == null)
                    {
                        // 共有の material を書き換えないよう、1 度だけ複製して renderer に持たせる。
                        ownMaterial = new Material(textureTarget.sharedMaterial);
                        textureTarget.sharedMaterial = ownMaterial;
                    }
                    ownMaterial.mainTexture = texture;
                    Swap(null, null, texture, null);
                    return null;
                }
                case "asset_bundle":
                {
                    // AssetBundle は作った Unity の版と build の対象に縛られるので、合わなければ読まない（2026-10-30 の判断）。
                    if ((string)format["unity_version"] != Application.unityVersion ||
                        (string)format["build_target"] != BuildTarget)
                        return "incompatible_asset_bundle";
                    // 同じ名前で作り直した bundle は、前の bundle を開いたままでは読めない。前の bundle を閉じてから読む
                    // （置いた GameObject と読み出した asset は残るので、今のスキンは表示されたまま）。
                    if (currentBundle != null)
                    {
                        currentBundle.Unload(false);
                        currentBundle = null;
                    }
                    var request = AssetBundle.LoadFromFileAsync(path);
                    await Until(request);
                    var bundle = request.assetBundle;
                    if (bundle == null)
                        return "invalid_asset";
                    var assets = bundle.LoadAllAssetsAsync<GameObject>();
                    await Until(assets);
                    if (assets.asset is not GameObject prefab || this == null)
                    {
                        bundle.Unload(true);
                        return "invalid_asset";
                    }
                    var instance = Instantiate(prefab, root, false);
                    Swap(instance, null, null, bundle);
                    // 閉じた前の bundle から読み出していた asset を解放する。
                    _ = Resources.UnloadUnusedAssets();
                    return null;
                }
                default:
                    return "unsupported_format";
            }
        }

        /// 新しい版を表示し、前の版の GameObject・glTF の資源・texture・AssetBundle を解放する。
        void Swap(GameObject instance, GltfImport import, Texture2D texture, AssetBundle bundle)
        {
            if (instance != null)
            {
                instance.SetActive(true);
                Release(Instance, currentImport, null, currentBundle);
                (Instance, currentImport, currentBundle) = (instance, import, bundle);
            }
            if (texture != null)
            {
                Release(null, null, currentTexture, null);
                currentTexture = texture;
            }
        }

        static void Release(GameObject instance, GltfImport import, Texture2D texture, AssetBundle bundle)
        {
            if (instance != null)
                DestroyNow(instance);
            import?.Dispose();
            if (texture != null)
                DestroyNow(texture);
            if (bundle != null)
                bundle.Unload(true);
        }

        static void DestroyNow(UnityEngine.Object target)
        {
            if (Application.isPlaying)
                Destroy(target);
            else
                DestroyImmediate(target);
        }

        /// glTF の外の URI を読まない download provider。GLB の BIN chunk と data URI だけが使われる。
        sealed class NoDownloads : IDownloadProvider
        {
            public Task<IDownload> Request(Uri url) => Task.FromResult<IDownload>(new Refused());

            public Task<ITextureDownload> RequestTexture(Uri url, bool nonReadable) =>
                Task.FromResult<ITextureDownload>(new Refused());

            sealed class Refused : ITextureDownload
            {
                public bool Success => false;
                public string Error => "kukuri skins do not load data from outside the signed file";
                public byte[] Data => null;
                public string Text => null;
                public bool? IsBinary => null;
                public Texture2D Texture => null;
                public void Dispose() { }
            }
        }

        static async Task Until(AsyncOperation operation)
        {
            while (!operation.isDone)
                await Task.Yield();
        }

        /// manifest の build の対象の名前（Unity の `BuildTarget` の名前）。
        static string BuildTarget => Application.platform switch
        {
            RuntimePlatform.WindowsPlayer or RuntimePlatform.WindowsEditor => "StandaloneWindows64",
            RuntimePlatform.OSXPlayer or RuntimePlatform.OSXEditor => "StandaloneOSX",
            RuntimePlatform.LinuxPlayer or RuntimePlatform.LinuxEditor => "StandaloneLinux64",
            RuntimePlatform.Android => "Android",
            RuntimePlatform.IPhonePlayer => "iOS",
            _ => Application.platform.ToString(),
        };
    }
}
