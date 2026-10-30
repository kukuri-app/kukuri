using System;
using System.Threading.Tasks;
using Newtonsoft.Json.Linq;
using UnityEngine;

namespace Kukuri.GameSdk
{
    /// live session を視聴し、復号した映像を `Output` の RenderTexture に描く（ADR 0070 §8.3、#1982）。音声は SDK が OS の
    /// 出力で鳴らす（音量は `set_live_volume`）。視聴は見るだけで、投稿・反応・自分の取り込みや配信を始めない。`screen` を
    /// 指定すれば、その Renderer の material の texture を `Output` にする。
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
        public RenderTexture Output { get; private set; }
        /// 視聴の状態（`watching`・`full`・`no_media`・`ended`）。視聴の前は null。
        public string State { get; private set; }
        public event Action<string> StateChanged;
        /// 描いた frame の数と、最後に描いた frame の timestamp・音声の再生時刻（μs）。
        public int FramesShown { get; private set; }
        public long LastTimestampUs { get; private set; }
        public long LastAudioClockUs { get; private set; }

        /// `sessionId` に参加して視聴を始める。
        public async Task<bool> StartWatching(string sessionId)
        {
            var session = new JObject { ["topic"] = Topic, ["session_id"] = sessionId };
            var joined = await client.SendAsync("join_live_session", session);
            if (!joined.Ok || this == null)
                return false;
            SessionId = sessionId;
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
            Release();
            await client.SendAsync("stop_watching_live_media", session);
            await client.SendAsync("leave_live_session", session);
        }

        void OnEvent(JObject parsed)
        {
            if ((string)parsed["type"] != "live_watch_status" || (string)parsed["session_id"] != SessionId)
                return;
            State = (string)parsed["state"];
            StateChanged?.Invoke(State);
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
