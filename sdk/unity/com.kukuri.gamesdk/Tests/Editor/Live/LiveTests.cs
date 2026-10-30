using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Kukuri.GameSdk.Editor;
using Newtonsoft.Json.Linq;
using NUnit.Framework;
using UnityEngine;
using UnityEngine.TestTools;
using Debug = UnityEngine.Debug;
using Object = UnityEngine.Object;

namespace Kukuri.GameSdk.Live.Tests
{
    /// Unity の Live の配信・視聴（#1982 AC-1）。ゲーム（FFI）と desktop（開発用の Sidecar。desktop の app と同じ runtime）の
    /// 2 つの profile で、ゲームの camera の配信を desktop が受け、desktop の配信をゲームの texture で見る。配信を始める前は
    /// 取り込まず送らない。止めたら RenderTexture・texture を解放する。視聴の間の frame は契約の予算（ADR 0070 §8.4 の frame
    /// time p95 ≤ 16.7 ms・p99 ≤ 25 ms）に収まる。test-graphics.ps1 が graphics を使い、keyring を切って loopback だけで動かす。
    public sealed class LiveTests
    {
        const int Width = 320;
        const int Height = 240;

        KukuriClient game;
        KukuriClient desktop;
        readonly List<GameObject> objects = new List<GameObject>();
        readonly List<double> frameMilliseconds = new List<double>();

        [TearDown]
        public void TearDown()
        {
            foreach (var owned in objects)
                if (owned != null)
                    Object.DestroyImmediate(owned);
            game?.Dispose();
            desktop?.Dispose();
            SidecarLauncher.StopAll();
        }

        static string NewDirectory() =>
            Path.Combine(Path.GetTempPath(), "kgs-" + Guid.NewGuid().ToString("N").Substring(0, 12));

        /// frame を進める（両方の client の pump と、`each` の処理）。frame の間隔を記録する。
        IEnumerator Frames(Func<bool> condition, string what, Action each = null, Func<string> detail = null)
        {
            var clock = Stopwatch.StartNew();
            var frame = Stopwatch.StartNew();
            while (!condition())
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 120, what + " " + detail?.Invoke());
                each?.Invoke();
                game?.Pump();
                desktop?.Pump();
                yield return null;
                frameMilliseconds.Add(frame.Elapsed.TotalMilliseconds);
                frame.Restart();
            }
        }

        IEnumerator Wait(Task task, string what)
        {
            yield return Frames(() => task.IsCompleted, what);
            if (task.IsFaulted)
                throw task.Exception;
        }

        IEnumerator Call(KukuriClient client, string command, JObject payload, Action<JToken> result = null)
        {
            var request = client.SendAsync(command, payload, timeoutMilliseconds: 120_000);
            yield return Wait(request, $"{command} did not answer");
            Assert.IsTrue(request.Result.Ok, $"{command}: {request.Result.ErrorCode} {request.Result.ErrorMessage}");
            result?.Invoke(request.Result.Data);
        }

        IEnumerator Accept(KukuriClient client)
        {
            JToken status = null;
            yield return Call(client, "get_desktop_startup_status", null, data => status = data);
            if ((string)status["status"] != "consent_required")
                yield break;
            JToken consent = null;
            yield return Call(client, "get_app_consent_status", null, data => consent = data);
            var documents = new JArray(((JArray)consent["documents"]).Select(document =>
                new JObject { ["slug"] = document["slug"], ["version"] = document["currentVersion"] }));
            yield return Call(client, "accept_app_consents",
                new JObject { ["documents"] = documents, ["language"] = "ja", ["age_attested"] = true });
        }

        IEnumerator CreateLive(KukuriClient client, string topic, Action<string> session)
        {
            yield return Call(client, "create_live_session",
                new JObject { ["topic"] = topic, ["title"] = "test: live", ["description"] = "" },
                data => session((string)data));
        }

        /// 上の行から並んだ I420 の点の Y・U・V。
        static (int y, int u, int v) Sample(KukuriLiveFrame frame, int x, int row)
        {
            int width = frame.Width, half = (width + 1) / 2, quarter = half * ((frame.Height + 1) / 2);
            var chroma = row / 2 * half + x / 2;
            return (frame.I420[row * width + x], frame.I420[width * frame.Height + chroma],
                frame.I420[width * frame.Height + quarter + chroma]);
        }

        static Color Read(RenderTexture texture, int x, int y)
        {
            var image = new Texture2D(texture.width, texture.height, TextureFormat.RGBA32, false);
            RenderTexture.active = texture;
            image.ReadPixels(new Rect(0, 0, texture.width, texture.height), 0, 0);
            image.Apply();
            RenderTexture.active = null;
            var color = image.GetPixel(x, y);
            Object.DestroyImmediate(image);
            return color;
        }

        static void Save(RenderTexture texture, string name)
        {
            var image = new Texture2D(texture.width, texture.height, TextureFormat.RGB24, false);
            RenderTexture.active = texture;
            image.ReadPixels(new Rect(0, 0, texture.width, texture.height), 0, 0);
            image.Apply();
            RenderTexture.active = null;
            var path = Path.Combine(Path.GetTempPath(), $"kgs-live-{name}.png");
            File.WriteAllBytes(path, image.EncodeToPNG());
            Debug.Log("KGS_CAPTURE " + path);
            Object.DestroyImmediate(image);
        }

        /// 下の行から並んだ RGBA。上半分は赤、下半分は青（上下が正しく届くかを見る）。
        static byte[] RedOverBlue()
        {
            var rgba = new byte[Width * Height * 4];
            for (var row = 0; row < Height; row++)
            for (var x = 0; x < Width; x++)
            {
                var top = row >= Height / 2;
                var at = (row * Width + x) * 4;
                rgba[at] = (byte)(top ? 230 : 20);
                rgba[at + 1] = 20;
                rgba[at + 2] = (byte)(top ? 20 : 230);
                rgba[at + 3] = 255;
            }
            return rgba;
        }

        /// 20 ms 分の stereo の正弦波（`OnAudioFilterRead` が受ける形）。
        static float[] Tone(int sampleRate, int start) =>
            Enumerable.Range(start, sampleRate / 50).SelectMany(index =>
            {
                var value = Mathf.Sin(2 * Mathf.PI * 440 * index / sampleRate) * 0.2f;
                return new[] { value, value };
            }).ToArray();

        static double Percentile(List<double> values, double rank)
        {
            var sorted = values.OrderBy(value => value).ToList();
            return sorted[(int)Math.Min(sorted.Count - 1, sorted.Count * rank)];
        }

        [UnityTest]
        public IEnumerator GameAndDesktopBroadcastAndWatchEachOther()
        {
            if (SystemInfo.graphicsDeviceType == UnityEngine.Rendering.GraphicsDeviceType.Null)
                Assert.Ignore("the live capture needs graphics (run test-graphics.ps1)");
            var topic = "kukuri:topic:test-live-" + Guid.NewGuid().ToString("N").Substring(0, 8);
            game = KukuriClient.Open(KukuriMode.Ffi, "game", NewDirectory());
            desktop = KukuriClient.Open(KukuriMode.Sidecar, "desktop", NewDirectory());
            foreach (var client in new[] { game, desktop })
            {
                yield return Accept(client);
                yield return Call(client, "set_topic_gossip_enabled", new JObject { ["topic"] = topic, ["enabled"] = true });
            }
            string ticketA = null, ticketB = null;
            yield return Call(game, "get_local_peer_ticket", null, data => ticketA = (string)data);
            yield return Call(desktop, "get_local_peer_ticket", null, data => ticketB = (string)data);
            yield return Call(desktop, "import_peer_ticket", new JObject { ["ticket"] = ticketA });
            yield return Call(game, "import_peer_ticket", new JObject { ["ticket"] = ticketB });

            // ゲーム → desktop: 赤い背景の camera を配信する。
            var cameraObject = new GameObject("live camera");
            objects.Add(cameraObject);
            var camera = cameraObject.AddComponent<Camera>();
            camera.clearFlags = CameraClearFlags.SolidColor;
            camera.backgroundColor = Color.red;
            cameraObject.AddComponent<AudioListener>();
            var broadcaster = cameraObject.AddComponent<KukuriLiveBroadcaster>();
            broadcaster.SourceCamera = camera;
            broadcaster.Client = game;
            broadcaster.Topic = topic;
            string gameLive = null;
            yield return CreateLive(game, topic, value => gameLive = value);
            // 明示的に始める前は、取り込みの要求を出さず、映像も音も受け付けない。
            broadcaster.Capture();
            Assert.AreEqual(0, broadcaster.FramesSent);
            Assert.IsNull(broadcaster.Target);
            Assert.AreEqual(KukuriSendStatus.Closed, game.SendLiveVideo(gameLive, Width, Height, RedOverBlue()));
            Assert.AreEqual(KukuriSendStatus.Closed, game.SendLiveAudio(gameLive, 48000, 2, Tone(48000, 0)));
            var started = broadcaster.StartBroadcast(gameLive);
            yield return Wait(started, "the broadcast did not start");
            Assert.IsTrue(started.Result);
            var tap = cameraObject.GetComponent<KukuriLiveAudio>();
            Assert.IsNotNull(tap, "the game audio is taken from the AudioListener");

            // 配信者は最初の keyframe を置くまで視聴を断る（`no_media`）ので、配信中の live を見に来る順にする。
            yield return Frames(() => broadcaster.FramesSent >= 5, "the game did not capture", () =>
            {
                camera.Render();
                broadcaster.Capture();
            });
            var received = new List<KukuriLiveFrame>();
            desktop.LiveFrameReceived += frame => received.Add(frame);
            yield return Call(desktop, "join_live_session", new JObject { ["topic"] = topic, ["session_id"] = gameLive });
            yield return Call(desktop, "watch_live_media", new JObject { ["topic"] = topic, ["session_id"] = gameLive });
            var sample = 0;
            frameMilliseconds.Clear();
            // EditMode では camera も audio も player loop で動かないので、frame ごとに描いて読み、音の callback を呼ぶ。
            yield return Frames(() => received.Count(frame => frame.AudioClockUs > 0) >= 30,
                "the desktop did not receive the game's frames with audio", () =>
                {
                    camera.Render();
                    broadcaster.Capture();
                    tap.OnAudioFilterRead(Tone(AudioSettings.outputSampleRate, sample), 2);
                    sample += AudioSettings.outputSampleRate / 50;
                }, () => $"received={received.Count} with_audio={received.Count(frame => frame.AudioClockUs > 0)} " +
                    $"sent={broadcaster.FramesSent} rate={AudioSettings.outputSampleRate}");
            var last = received.Last();
            var (_, _, v) = Sample(last, last.Width / 2, last.Height / 2);
            Assert.Greater(v, 200, "the desktop sees the red background");
            Assert.LessOrEqual(last.TimestampUs, last.AudioClockUs + 125_000, "video does not run ahead of audio");
            var gameP95 = Percentile(frameMilliseconds, 0.95);
            var gameP99 = Percentile(frameMilliseconds, 0.99);

            // desktop → ゲーム: desktop の配信をゲームの texture で見る。ゲームは見るだけで自分の配信を始めない。
            string desktopLive = null;
            yield return CreateLive(desktop, topic, value => desktopLive = value);
            yield return Call(desktop, "set_live_broadcast", new JObject
            {
                ["topic"] = topic, ["session_id"] = desktopLive, ["source"] = "camera", ["microphone"] = false,
            });
            var frames = RedOverBlue();
            for (var index = 0; index < 5; index++)
                Assert.AreEqual(KukuriSendStatus.Sent, desktop.SendLiveVideo(desktopLive, Width, Height, frames));
            var viewerObject = new GameObject("live viewer");
            objects.Add(viewerObject);
            var viewer = viewerObject.AddComponent<KukuriLiveViewer>();
            viewer.Client = game;
            viewer.Topic = topic;
            var watching = viewer.StartWatching(desktopLive);
            yield return Wait(watching, "the watch did not start");
            Assert.IsTrue(watching.Result);
            var desktopSample = 0;
            frameMilliseconds.Clear();
            yield return Frames(() => viewer.FramesShown >= 30 && viewer.LastAudioClockUs > 0,
                "the game did not show the desktop's frames", () =>
                {
                    desktop.SendLiveVideo(desktopLive, Width, Height, frames);
                    desktop.SendLiveAudio(desktopLive, 48000, 2, Tone(48000, desktopSample));
                    desktopSample += 960;
                }, () => $"shown={viewer.FramesShown} audio={viewer.LastAudioClockUs} state={viewer.State}");
            Assert.AreEqual("watching", viewer.State);
            Assert.AreEqual(Width, viewer.Output.width);
            var top = Read(viewer.Output, Width / 2, Height * 3 / 4);
            var bottom = Read(viewer.Output, Width / 2, Height / 4);
            Assert.Greater(top.r, 0.7f, $"the top is red ({top})");
            Assert.Greater(bottom.b, 0.7f, $"the bottom is blue ({bottom})");
            Assert.LessOrEqual(viewer.LastTimestampUs, viewer.LastAudioClockUs + 125_000);
            Save(viewer.Output, "1-game-watches-desktop");
            Save(broadcaster.Target, "2-game-broadcast");
            var viewP95 = Percentile(frameMilliseconds, 0.95);
            var viewP99 = Percentile(frameMilliseconds, 0.99);
            Debug.Log($"KGS_LIVE desktop_received={received.Count} game_shown={viewer.FramesShown} " +
                $"broadcast_frame_p95_ms={gameP95:F1} p99_ms={gameP99:F1} view_frame_p95_ms={viewP95:F1} p99_ms={viewP99:F1}");
            Assert.LessOrEqual(Math.Max(gameP95, viewP95), 16.7, "frame time p95");
            Assert.LessOrEqual(Math.Max(gameP99, viewP99), 25, "frame time p99");

            // 止めたら、取り込み・texture を解放し、映像も音も受け付けない。
            var textures = Resources.FindObjectsOfTypeAll<Texture2D>().Length;
            var stopWatching = viewer.StopWatching();
            yield return Wait(stopWatching, "the watch did not stop");
            Assert.IsNull(viewer.Output);
            Assert.AreEqual(textures - 3, Resources.FindObjectsOfTypeAll<Texture2D>().Length, "the I420 planes are released");
            var stopped = broadcaster.StopBroadcast();
            yield return Wait(stopped, "the broadcast did not stop");
            Assert.IsNull(broadcaster.Target);
            Assert.IsNull(cameraObject.GetComponent<KukuriLiveAudio>(), "the audio tap is removed");
            Assert.IsNull(camera.targetTexture);
            Assert.AreEqual(KukuriSendStatus.Closed, game.SendLiveVideo(gameLive, Width, Height, RedOverBlue()));
            Assert.AreEqual(KukuriSendStatus.Closed, game.SendLiveAudio(gameLive, 48000, 2, Tone(48000, 0)));
        }
    }
}
