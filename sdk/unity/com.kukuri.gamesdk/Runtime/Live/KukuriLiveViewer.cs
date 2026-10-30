using System;
using System.Threading.Tasks;
using Newtonsoft.Json.Linq;
using UnityEngine;

namespace Kukuri.GameSdk
{
    /// live session を視聴し、復号した映像を `Output` の RenderTexture に描く（ADR 0070 §8.3、#1982）。音声は SDK が OS の
    /// 出力で鳴らす（音量は `set_live_volume`）。視聴は見るだけで、投稿・反応・自分の取り込みや配信を始めない。`screen` を
    /// 指定すれば、その Renderer の material の texture を `Output` にする。満員・映像なし・終了になったら、SDK が 5 秒
    /// おいて視聴をやり直す（`State` が変わる）。この live の開始 post への返信とリアクションだけを `ReplyReceived`・
    /// `ReactionReceived` で知らせる（同じスレッドの別の live のものは渡さない。返信とリアクションは開始 post に属する）。
    public sealed class KukuriLiveViewer : MonoBehaviour
    {
        [SerializeField] Renderer screen;

        KukuriClient client;
        Texture2D luma;
        Texture2D cb;
        Texture2D cr;
        Material convert;
        Material screenMaterial;

        public KukuriClient Client
        {
            get => client;
            set
            {
                if (client != null)
                {
                    client.LiveFrameReceived -= OnFrame;
                    client.EventReceived -= OnEvent;
                }
                client = value;
                if (client != null)
                {
                    client.LiveFrameReceived += OnFrame;
                    client.EventReceived += OnEvent;
                }
            }
        }

        public string Topic { get; set; }
        public string SessionId { get; private set; }
        /// この live の開始 post の ID（無ければ null）。
        public string StartPostId { get; private set; }
        public RenderTexture Output { get; private set; }
        /// 視聴の状態（`watching`・`full`・`no_media`・`ended`）。視聴の前は null。
        public string State { get; private set; }
        public event Action<string> StateChanged;
        /// 開始 post への返信（`post_received` の `post`）とリアクション（`reaction_received` の `reaction`）。
        public event Action<JObject> ReplyReceived;
        public event Action<JObject> ReactionReceived;
        /// 描いた frame の数と、最後に描いた frame の timestamp・音声の再生時刻（μs）。
        public int FramesShown { get; private set; }
        public long LastTimestampUs { get; private set; }
        public long LastAudioClockUs { get; private set; }
        /// 最後に描いた frame の、配信者からの段数（1 は直接、2 以上は視聴者の転送）。
        public int Hops { get; private set; }

        /// `sessionId` に参加して視聴を始める。
        public async Task<bool> StartWatching(string sessionId)
        {
            var session = new JObject { ["topic"] = Topic, ["session_id"] = sessionId };
            var joined = await client.SendAsync("join_live_session", session);
            if (!joined.Ok)
                return false;
            if (this == null)
            {
                _ = client.SendAsync("leave_live_session", session);
                return false;
            }
            SessionId = sessionId;
            var live = await client.SendAsync("get_live_session", session);
            StartPostId = live.Ok && live.Data is JObject view ? (string)view["start_post_id"] : null;
            var watched = await client.SendAsync("watch_live_media", session);
            return watched.Ok;
        }

        /// 視聴を止め、texture を解放する。
        public async Task StopWatching()
        {
            if (SessionId == null)
                return;
            var session = new JObject { ["topic"] = Topic, ["session_id"] = SessionId };
            SessionId = null;
            StartPostId = null;
            Release();
            await client.SendAsync("stop_watching_live_media", session);
            await client.SendAsync("leave_live_session", session);
        }

        void OnEvent(JObject parsed)
        {
            switch ((string)parsed["type"])
            {
                case "live_watch_status" when (string)parsed["session_id"] == SessionId:
                    State = (string)parsed["state"];
                    StateChanged?.Invoke(State);
                    break;
                case "reaction_received" when StartPostId != null &&
                                              (string)parsed["reaction"]?["target_object_id"] == StartPostId:
                    ReactionReceived?.Invoke((JObject)parsed["reaction"]);
                    break;
                case "post_received" when StartPostId != null &&
                                          ((string)parsed["post"]?["reply_to"] == StartPostId ||
                                           (string)parsed["post"]?["root_id"] == StartPostId):
                    ReplyReceived?.Invoke((JObject)parsed["post"]);
                    break;
            }
        }

        void OnFrame(KukuriLiveFrame frame)
        {
            if (frame.SessionId != SessionId || this == null)
                return;
            int width = frame.Width, height = frame.Height, half = (width + 1) / 2, halfHeight = (height + 1) / 2;
            if (luma == null || luma.width != width || luma.height != height)
            {
                Release();
                luma = new Texture2D(width, height, TextureFormat.R8, false, true);
                cb = new Texture2D(half, halfHeight, TextureFormat.R8, false, true);
                cr = new Texture2D(half, halfHeight, TextureFormat.R8, false, true);
                Output = new RenderTexture(width, height, 0, RenderTextureFormat.ARGB32);
                convert = new Material(UnityEngine.Resources.Load<Shader>("KukuriI420"));
                convert.SetTexture("_UTex", cb);
                convert.SetTexture("_VTex", cr);
                if (screen != null)
                {
                    screenMaterial = screen.material;
                    screenMaterial.mainTexture = Output;
                }
            }
            luma.SetPixelData(frame.I420, 0, 0);
            cb.SetPixelData(frame.I420, 0, width * height);
            cr.SetPixelData(frame.I420, 0, width * height + half * halfHeight);
            luma.Apply(false);
            cb.Apply(false);
            cr.Apply(false);
            Graphics.Blit(luma, Output, convert);
            FramesShown++;
            LastTimestampUs = frame.TimestampUs;
            LastAudioClockUs = frame.AudioClockUs;
            Hops = frame.Hops;
        }

        void OnDestroy()
        {
            if (SessionId != null && client != null)
                _ = StopWatching();
            Release();
            Client = null;
        }

        /// play mode の外（EditMode の試験）では即座に破棄する。
        static void DestroyNow(UnityEngine.Object owned)
        {
            if (UnityEngine.Application.isPlaying)
                Destroy(owned);
            else
                DestroyImmediate(owned);
        }

        void Release()
        {
            foreach (var owned in new UnityEngine.Object[] { luma, cb, cr, convert, screenMaterial })
                if (owned != null)
                    DestroyNow(owned);
            if (Output != null)
            {
                Output.Release();
                DestroyNow(Output);
            }
            luma = cb = cr = null;
            convert = screenMaterial = null;
            Output = null;
        }
    }
}
