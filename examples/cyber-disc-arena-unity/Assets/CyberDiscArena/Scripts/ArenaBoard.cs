using System.IO;
using System.Threading.Tasks;
using Kukuri.GameSdk;
using Kukuri.GameSdk.Social;
using Newtonsoft.Json.Linq;
using UnityEngine;

namespace CyberDiscArena
{
    /// ゲーム内の掲示板（SDK の `KukuriBoard`）。Enter で開け閉めし、文字を入力している間は対戦の操作に使わない（ArenaControls）。
    /// 試合の結果は「結果を投稿」で、得点の文と画面の画像を掲示板の topic へ公開で投稿する（2026-11-01 のユーザー判断。
    /// 押したときだけ）。投稿はゲームを終えた後も SNS（desktop・Web）から返信でき、次に掲示板でその thread を開くと読める。
    [DefaultExecutionOrder(200)]
    public sealed class ArenaBoard : MonoBehaviour
    {
        [SerializeField] KukuriRunner runner;
        [SerializeField] KukuriAccountScreen account;
        [SerializeField] KukuriBoard board;

        public KukuriBoard Board => board;
        /// 結果の投稿の知らせ（次の試合で消す）。
        public string Notice { get; set; } = "";

        /// 掲示板を開いているか。開いている間は、arena の他の IMGUI（開始の画面・ロッカー・配信の切替・退出・応援の menu）を
        /// 出さない（IMGUI は uGUI の上に描かれ、押すと両方が反応する。2026-11-01 のユーザー判断）。
        public static bool Open { get; private set; }

        void Awake() => board.gameObject.SetActive(false);

        void OnDisable() => Open = false;

        // 開いている間は開始の画面を隠す（ArenaView の後に動く）。
        void LateUpdate()
        {
            if (Open)
                account.Visible = false;
        }

        void Update()
        {
            if (account.Screen == "playing" && !ArenaControls.Typing() &&
                (Input.GetKeyDown(KeyCode.Return) || Input.GetKeyDown(KeyCode.KeypadEnter)))
            {
                board.gameObject.SetActive(!board.gameObject.activeSelf);
                // 閉じたら開始の画面を戻す（対戦中は ArenaView が同じ frame でまた隠す）。
                if (!board.gameObject.activeSelf)
                    account.Visible = true;
            }
            Open = board.gameObject.activeSelf;
        }

        /// 結果の文と、今の画面の画像を投稿する（画像は一時 file に書き、BLAKE3 の hash で添付する）。
        public async Task<string> PostResult(string content)
        {
            await Awaitable.EndOfFrameAsync();
            var shot = ScreenCapture.CaptureScreenshotAsTexture();
            var png = shot.EncodeToPNG();
            Destroy(shot);
            Notice = "結果を投稿しています";
            var path = Path.Combine(Application.temporaryCachePath, $"cyber-disc-arena-result-{System.Guid.NewGuid():N}.png");
            try
            {
                File.WriteAllBytes(path, png);
                var response = await runner.Client.SendAsync("create_post", new JObject
                {
                    ["topic"] = board.Topic,
                    ["content"] = content,
                    ["channel_ref"] = new JObject { ["kind"] = "public" },
                    ["attachments"] = new JArray(new JObject
                    {
                        ["path"] = path,
                        ["hash"] = KukuriClient.Blake3Hex(png),
                        ["byte_size"] = png.Length,
                        ["mime"] = "image/png",
                        ["file_name"] = "result.png",
                    }),
                });
                Notice = response.Ok ? "結果を投稿しました（掲示板は Enter）" : $"投稿できませんでした（{response.ErrorCode}）";
                return response.Ok ? (string)response.Data : null;
            }
            catch (System.Exception error) when (error is KukuriException || error is IOException)
            {
                Notice = "投稿できませんでした";
                return null;
            }
            finally
            {
                File.Delete(path);
            }
        }
    }
}
