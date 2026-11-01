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
    public sealed class ArenaBoard : MonoBehaviour
    {
        [SerializeField] KukuriRunner runner;
        [SerializeField] KukuriAccountScreen account;
        [SerializeField] KukuriBoard board;

        public KukuriBoard Board => board;
        /// 最後に投稿した結果（投稿 ID と、画面に出す知らせ）。
        public string ResultPost { get; private set; }
        public string Notice { get; private set; } = "";

        void Awake() => board.gameObject.SetActive(false);

        void Update()
        {
            if (account.Screen != "playing" || ArenaControls.Typing())
                return;
            if (Input.GetKeyDown(KeyCode.Return) || Input.GetKeyDown(KeyCode.KeypadEnter))
                board.gameObject.SetActive(!board.gameObject.activeSelf);
        }

        /// 結果の文と、今の画面の画像を投稿する（画像は一時 file に書き、BLAKE3 の hash で添付する）。
        public async Task<string> PostResult(string content)
        {
            Notice = "結果を投稿しています";
            await Awaitable.EndOfFrameAsync();
            var shot = ScreenCapture.CaptureScreenshotAsTexture();
            var png = shot.EncodeToPNG();
            Destroy(shot);
            var path = Path.Combine(Application.temporaryCachePath, $"cyber-disc-arena-result-{System.Guid.NewGuid():N}.png");
            File.WriteAllBytes(path, png);
            try
            {
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
                return ResultPost = response.Ok ? (string)response.Data : null;
            }
            finally
            {
                File.Delete(path);
            }
        }
    }
}
