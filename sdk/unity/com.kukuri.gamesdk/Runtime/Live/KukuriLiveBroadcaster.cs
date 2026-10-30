using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using Newtonsoft.Json.Linq;
using UnityEngine;
using UnityEngine.Rendering;

namespace Kukuri.GameSdk
{
    /// camera の映像とゲームの音を、ゲームが作った live session へ配信する（ADR 0070 §8.3、#1982）。`StartBroadcast` を
    /// 呼ぶまで何も取り込まず、送らない。camera は配信の RenderTexture に描く（ゲームの画面へは `Target` を出す）。
    /// 音は `AudioListener` の出力を取り、`microphone` が on なら mic の音を混ぜて 1 つの音声にする（mic が off でも
    /// ゲームの音は送る）。
    public sealed class KukuriLiveBroadcaster : MonoBehaviour
    {
        [SerializeField] Camera sourceCamera;
        [SerializeField] int width = 1280;
        [SerializeField] int height = 720;
        [SerializeField] bool microphone;

        RenderTexture target;
        RenderTexture previousTarget;
        bool reading;
        float lastCapture = float.NegativeInfinity;
        KukuriLiveAudio audioTap;

        public KukuriClient Client { get; set; }
        public string Topic { get; set; }
        public Camera SourceCamera { get => sourceCamera; set => sourceCamera = value; }
        public bool Microphone { get => microphone; set => microphone = value; }
        public string SessionId { get; private set; }
        public bool Broadcasting { get; private set; }
        /// 配信している画面（camera が描く先）。
        public RenderTexture Target => target;
        /// SDK が受け付けた映像の frame の数。
        public int FramesSent { get; private set; }

        /// `sessionId`（この profile で作った live session）の配信を始める。
        public async Task<bool> StartBroadcast(string sessionId)
        {
            if (Broadcasting)
                return true;
            var response = await Client.SendAsync("set_live_broadcast", new JObject
            {
                ["topic"] = Topic,
                ["session_id"] = sessionId,
                ["source"] = "camera",
                // mic の音は Unity が混ぜるので、runtime の mic は開かない。
                ["microphone"] = false,
            });
            if (!response.Ok || this == null)
                return false;
            SessionId = sessionId;
            target = new RenderTexture(Math.Min(width, 1280), Math.Min(height, 720), 24, RenderTextureFormat.ARGB32);
            previousTarget = sourceCamera.targetTexture;
            sourceCamera.targetTexture = target;
            audioTap = KukuriLiveAudio.Attach(sourceCamera, Client, sessionId, microphone);
            Broadcasting = true;
            return true;
        }

        /// 配信を止め、取り込み・mic・RenderTexture を解放する。
        public async Task StopBroadcast()
        {
            if (!Broadcasting)
                return;
            var session = SessionId;
            Release();
            await Client.SendAsync("stop_live_broadcast", new JObject { ["topic"] = Topic, ["session_id"] = session });
        }

        void Update() => Capture();

        /// 30 fps までで camera の描画を読む。読み出しは 1 つずつで、前の読み出しが終わる前の frame は送らない。
        internal void Capture()
        {
            if (!Broadcasting || reading || Time.realtimeSinceStartup - lastCapture < 1f / 30)
                return;
            lastCapture = Time.realtimeSinceStartup;
            reading = true;
            AsyncGPUReadback.Request(target, 0, TextureFormat.RGBA32, OnReadback);
        }

        void OnReadback(AsyncGPUReadbackRequest request)
        {
            reading = false;
            if (this == null || !Broadcasting || request.hasError)
                return;
            if (Client.SendLiveVideo(SessionId, request.width, request.height,
                    request.GetData<byte>().AsReadOnlySpan()) == KukuriSendStatus.Sent)
                FramesSent++;
        }

        void OnDestroy()
        {
            if (Broadcasting)
                _ = StopBroadcast();
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
            Broadcasting = false;
            if (audioTap != null)
                DestroyNow(audioTap);
            audioTap = null;
            if (sourceCamera != null && sourceCamera.targetTexture == target)
                sourceCamera.targetTexture = previousTarget;
            if (target != null)
            {
                target.Release();
                DestroyNow(target);
            }
            target = null;
        }
    }

    /// `AudioListener` と同じ GameObject に置き、ゲームの出力音（と mic の音）を配信へ渡す。音を出す側（speaker）の音は
    /// 変えない。mic の音は main thread で読んで送り待ちに置き、audio thread で混ぜる。
    [AddComponentMenu("")]
    sealed class KukuriLiveAudio : MonoBehaviour
    {
        const float QueueSeconds = 0.2f;

        KukuriClient client;
        string sessionId;
        int sampleRate;
        AudioClip mic;
        int micPosition;
        float[] micRead = Array.Empty<float>();
        float[] mixed = Array.Empty<float>();
        readonly Queue<float> micSamples = new Queue<float>();

        /// 配信する camera の `AudioListener`、無ければ scene の `AudioListener` に置く。
        internal static KukuriLiveAudio Attach(Camera camera, KukuriClient client, string sessionId, bool microphone)
        {
            var listener = camera.GetComponent<AudioListener>();
            if (listener == null)
                listener = FindAnyObjectByType<AudioListener>();
            if (listener == null)
                return null;
            var tap = listener.gameObject.AddComponent<KukuriLiveAudio>();
            tap.client = client;
            tap.sessionId = sessionId;
            tap.sampleRate = AudioSettings.outputSampleRate;
            if (microphone && UnityEngine.Microphone.devices.Length > 0)
                tap.mic = UnityEngine.Microphone.Start(null, true, 1, tap.sampleRate);
            return tap;
        }

        void Update()
        {
            if (mic == null)
                return;
            var position = UnityEngine.Microphone.GetPosition(null);
            var count = (position - micPosition + mic.samples) % mic.samples;
            if (count == 0)
                return;
            if (micRead.Length != count)
                micRead = new float[count];
            mic.GetData(micRead, micPosition);
            micPosition = position;
            lock (micSamples)
            {
                foreach (var sample in micRead)
                    micSamples.Enqueue(sample);
                while (micSamples.Count > sampleRate * QueueSeconds)
                    micSamples.Dequeue();
            }
        }

        /// audio thread。speaker へ出す `data` は変えず、写しに mic の音を足して送る。
        internal void OnAudioFilterRead(float[] data, int channels)
        {
            if (mixed.Length != data.Length)
                mixed = new float[data.Length];
            Array.Copy(data, mixed, data.Length);
            if (mic != null)
                lock (micSamples)
                    for (var frame = 0; frame < data.Length / channels && micSamples.Count > 0; frame++)
                    {
                        var sample = micSamples.Dequeue();
                        for (var channel = 0; channel < channels; channel++)
                            mixed[frame * channels + channel] += sample;
                    }
            client.SendLiveAudio(sessionId, sampleRate, channels, mixed);
        }

        void OnDestroy()
        {
            if (mic != null)
                UnityEngine.Microphone.End(null);
        }
    }
}
