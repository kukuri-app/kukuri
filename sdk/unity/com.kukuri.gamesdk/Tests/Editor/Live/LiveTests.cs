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
    /// 取り込まず送らない。止めたら RenderTexture・texture を解放する。1280×720 の配信・視聴の間の frame は契約の予算（ADR 0070
    /// §8.4 の frame time p95 ≤ 16.7 ms・p99 ≤ 25 ms）に収まり、描く frame の時刻 − 音声の再生時刻は p95 で −200〜+125 ms に
    /// 入る（2026-10-31 のユーザー判断）。test-graphics.ps1 が graphics を使い、keyring を切って loopback だけで動かす。
    public sealed class LiveTests
    {
        const int Width = 1280;
        const int Height = 720;

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
        IEnumerator Frames(Func<bool> condition, string what, Action each = null, Func<string> detail = null,
            double forSeconds = 0)
        {
            var clock = Stopwatch.StartNew();
            var frame = Stopwatch.StartNew();
            while (!condition())
            {
                if (forSeconds > 0 && clock.Elapsed.TotalSeconds >= forSeconds)
                    yield break;
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

        /// 視聴の音量を 0 にする（試験で speaker から音を出さない。復号・出力・再生時刻は動く）。
        IEnumerator Mute(KukuriClient client, string session) =>
            Call(client, "set_live_volume", new JObject { ["session_id"] = session, ["volume"] = 0.0 });

        static string NewTopic() => "kukuri:topic:test-live-" + Guid.NewGuid().ToString("N").Substring(0, 8);

        /// ゲーム（FFI）と desktop（開発用の Sidecar）の profile を開き、同意して topic を購読し、互いにつなぐ。
        IEnumerator Connect(string topic)
        {
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
        }

        IEnumerator CreateLive(KukuriClient client, string topic, Action<string> session, JObject channel = null)
        {
            var payload = new JObject
                { ["topic"] = topic, ["title"] = "test: live", ["description"] = "", ["start_post"] = "test: live" };
            if (channel != null)
                payload["channel_ref"] = channel;
            yield return Call(client, "create_live_session", payload, data => session((string)data));
        }

        /// 1 色で塗った下の行からの RGBA。
        static byte[] Solid(Color32 color)
        {
            var rgba = new byte[Width * Height * 4];
            for (var at = 0; at < rgba.Length; at += 4)
            {
                rgba[at] = color.r;
                rgba[at + 1] = color.g;
                rgba[at + 2] = color.b;
                rgba[at + 3] = 255;
            }
            return rgba;
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

        /// 描く frame の時刻 − 音声の再生時刻が −200〜+125 ms に入る frame が 95% 以上。
        static string AssertInSync(List<KukuriLiveFrame> frames, string what)
        {
            var played = frames.Where(frame => frame.AudioClockUs > 0).ToList();
            Assert.IsNotEmpty(played, what + ": no frame came with an audio clock");
            var inSync = played.Count(frame => frame.TimestampUs - frame.AudioClockUs is >= -200_000 and <= 125_000);
            Assert.GreaterOrEqual(inSync * 100, played.Count * 95, $"{what}: {inSync} of {played.Count} frames in sync " +
                string.Join(",", played.Select(frame => (frame.TimestampUs - frame.AudioClockUs) / 1000)));
            var offsets = played.Select(frame => (frame.TimestampUs - frame.AudioClockUs) / 1000).ToList();
            return $"{offsets.Min()}..{offsets.Max()}";
        }

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
            var topic = NewTopic();
            yield return Connect(topic);

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
            yield return Mute(desktop, gameLive);
            frameMilliseconds.Clear();
            // EditMode では camera は player loop で描かないので、frame ごとに描いて読む。音は Unity の audio thread が
            // `AudioListener` の出力（ここでは無音）を `OnAudioFilterRead` へ渡す。
            yield return Frames(() => received.Count(frame => frame.AudioClockUs > 0) >= 30,
                "the desktop did not receive the game's frames with audio", () =>
                {
                    camera.Render();
                    broadcaster.Capture();
                }, () => $"received={received.Count} with_audio={received.Count(frame => frame.AudioClockUs > 0)} " +
                    $"sent={broadcaster.FramesSent} rate={AudioSettings.outputSampleRate}");
            var last = received.Last();
            var (_, _, v) = Sample(last, last.Width / 2, last.Height / 2);
            Assert.Greater(v, 200, "the desktop sees the red background");
            var gameAv = AssertInSync(received, "game to desktop");
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
            // 最初の keyframe より前に見に来る。配信者に断られて `no_media` になり、SDK が間をおいて視聴をやり直す。
            var viewerObject = new GameObject("live viewer");
            objects.Add(viewerObject);
            var viewer = viewerObject.AddComponent<KukuriLiveViewer>();
            viewer.Client = game;
            viewer.Topic = topic;
            var states = new List<string>();
            viewer.StateChanged += state => states.Add(state);
            var shown = new List<KukuriLiveFrame>();
            game.LiveFrameReceived += frame => shown.Add(frame);
            var watching = viewer.StartWatching(desktopLive);
            yield return Wait(watching, "the watch did not start");
            Assert.IsTrue(watching.Result);
            yield return Mute(game, desktopLive);
            // 配信者に断られる（`no_media`）まで送らない。その後に送り始め、SDK のやり直しで見えるようになる。
            yield return Frames(() => states.Contains("no_media"), "the first watch was not refused");
            var desktopSample = 0;
            var desktopClock = Stopwatch.StartNew();
            frameMilliseconds.Clear();
            yield return Frames(() => viewer.FramesShown >= 30 && viewer.LastAudioClockUs > 0,
                "the game did not show the desktop's frames", () =>
                {
                    desktop.SendLiveVideo(desktopLive, Width, Height, frames);
                    while (desktopSample < desktopClock.Elapsed.TotalSeconds * 48000)
                    {
                        desktop.SendLiveAudio(desktopLive, 48000, 2, Tone(48000, desktopSample));
                        desktopSample += 960;
                    }
                }, () => $"shown={viewer.FramesShown} audio={viewer.LastAudioClockUs} state={viewer.State}");
            Assert.AreEqual("watching", viewer.State);
            Assert.AreEqual("watching", states.Last(), "the watch is retried after the refusal");
            // 視聴する側は自分の mic を開かず、配信もしない。
            Assert.IsFalse(Microphone.IsRecording(null));
            Assert.AreEqual(KukuriSendStatus.Closed, game.SendLiveVideo(desktopLive, Width, Height, frames));
            Assert.AreEqual(Width, viewer.Output.width);
            var top = Read(viewer.Output, Width / 2, Height * 3 / 4);
            var bottom = Read(viewer.Output, Width / 2, Height / 4);
            Assert.Greater(top.r, 0.7f, $"the top is red ({top})");
            Assert.Greater(bottom.b, 0.7f, $"the bottom is blue ({bottom})");
            var desktopAv = AssertInSync(shown, "desktop to game");
            Save(viewer.Output, "1-game-watches-desktop");
            Save(broadcaster.Target, "2-game-broadcast");
            var viewP95 = Percentile(frameMilliseconds, 0.95);
            var viewP99 = Percentile(frameMilliseconds, 0.99);
            Debug.Log($"KGS_LIVE desktop_received={received.Count} game_shown={viewer.FramesShown} " +
                $"av_ms game_to_desktop={gameAv} desktop_to_game={desktopAv} " +
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

        /// 同じ公開スレッドに 2 つの live（#1982 AC-2）。ゲームは両方を見て、映像と、開始 post への返信・リアクションをそれぞれの
        /// 視聴に混ぜずに受ける。片方の視聴をやめると、その購読だけが止まり、もう片方は続く。
        [UnityTest]
        public IEnumerator TwoLivesInOneThreadKeepTheirVideoAndReactionsApart()
        {
            if (SystemInfo.graphicsDeviceType == UnityEngine.Rendering.GraphicsDeviceType.Null)
                Assert.Ignore("the live view needs graphics (run test-graphics.ps1)");
            var topic = NewTopic();
            yield return Connect(topic);
            string thread = null;
            yield return Call(desktop, "create_public_channel",
                new JObject { ["topic"] = topic, ["title"] = "test: thread", ["first_post"] = "test: thread" },
                data => thread = (string)data["channel_id"]);
            // ゲームはスレッドの投稿・リアクションの知らせを購読する（desktop・Web の開いている列にあたる）。
            yield return Call(game, "set_public_channel_subscription",
                new JObject { ["topic"] = topic, ["channel_id"] = thread, ["enabled"] = true });
            var inThread = new JObject { ["kind"] = "public_channel", ["channel_id"] = thread };
            string first = null, second = null;
            yield return CreateLive(desktop, topic, value => first = value, inThread);
            yield return CreateLive(desktop, topic, value => second = value, inThread);
            foreach (var live in new[] { first, second })
                yield return Call(desktop, "set_live_broadcast", new JObject
                {
                    ["topic"] = topic, ["session_id"] = live, ["source"] = "camera", ["microphone"] = false,
                });
            var red = Solid(new Color32(230, 20, 20, 255));
            var blue = Solid(new Color32(20, 20, 230, 255));
            Action broadcast = () =>
            {
                desktop.SendLiveVideo(first, Width, Height, red);
                desktop.SendLiveVideo(second, Width, Height, blue);
            };
            yield return Frames(() => false, "", broadcast, null, 0.5);

            var viewers = new List<KukuriLiveViewer>();
            var replies = new[] { new List<JObject>(), new List<JObject>() };
            var reactions = new[] { new List<JObject>(), new List<JObject>() };
            foreach (var live in new[] { first, second })
            {
                var viewerObject = new GameObject("viewer " + viewers.Count);
                objects.Add(viewerObject);
                var viewer = viewerObject.AddComponent<KukuriLiveViewer>();
                var index = viewers.Count;
                viewer.ReplyReceived += post => replies[index].Add(post);
                viewer.ReactionReceived += reaction => reactions[index].Add(reaction);
                viewer.Client = game;
                viewer.Topic = topic;
                var watching = viewer.StartWatching(live);
                yield return Wait(watching, "the watch did not start");
                Assert.IsTrue(watching.Result);
                yield return Mute(game, live);
                Assert.IsNotNull(viewer.StartPostId, "the live has its own start post");
                viewers.Add(viewer);
            }
            Assert.AreNotEqual(viewers[0].StartPostId, viewers[1].StartPostId);
            yield return Frames(() => viewers.All(viewer => viewer.FramesShown >= 10), "the game did not show both lives",
                broadcast, () => string.Join(",", viewers.Select(viewer => $"{viewer.FramesShown}/{viewer.State}")));
            Assert.Greater(Read(viewers[0].Output, Width / 2, Height / 2).r, 0.7f, "the first live is red");
            Assert.Greater(Read(viewers[1].Output, Width / 2, Height / 2).b, 0.7f, "the second live is blue");
            Save(viewers[0].Output, "3-thread-first-live");
            Save(viewers[1].Output, "4-thread-second-live");

            // desktop が 1 つ目の開始 post にリアクションし、2 つ目の開始 post に返信する。
            yield return Call(desktop, "toggle_reaction", new JObject
            {
                ["target_topic_id"] = topic, ["target_object_id"] = viewers[0].StartPostId,
                ["reaction_key"] = new JObject { ["kind"] = "emoji", ["emoji"] = "👍" }, ["channel_ref"] = inThread,
            });
            yield return Call(desktop, "create_post", new JObject
            {
                ["topic"] = topic, ["content"] = "test: reply to the second live", ["reply_to"] = viewers[1].StartPostId,
                ["channel_ref"] = inThread,
            });
            yield return Frames(() => reactions[0].Count > 0 && replies[1].Count > 0,
                "the reaction and the reply did not reach their lives", broadcast,
                () => $"reactions={reactions[0].Count}/{reactions[1].Count} replies={replies[0].Count}/{replies[1].Count}");
            yield return Frames(() => false, "", broadcast, null, 2);
            Assert.AreEqual(viewers[0].StartPostId, (string)reactions[0][0]["target_object_id"]);
            Assert.IsEmpty(reactions[1], "the second live does not get the first live's reaction");
            Assert.AreEqual(viewers[1].StartPostId, (string)replies[1][0]["reply_to"]);
            Assert.IsEmpty(replies[0], "the first live does not get the second live's reply");

            // 1 つ目の視聴をやめると、その映像の購読だけが止まり（SDK は止めた session の frame を渡さない）、2 つ目は続く。
            var delivered = new Dictionary<string, int> { [first] = 0, [second] = 0 };
            game.LiveFrameReceived += frame => delivered[frame.SessionId]++;
            var stop = viewers[0].StopWatching();
            yield return Wait(stop, "the watch did not stop");
            yield return Frames(() => false, "", broadcast, null, 1);
            delivered[first] = 0;
            delivered[second] = 0;
            yield return Frames(() => false, "", broadcast, null, 2);
            Assert.IsNull(viewers[0].Output);
            Assert.AreEqual(0, delivered[first], "the stopped watch delivers no frame");
            Assert.Greater(delivered[second], 0, "the other live keeps playing");
        }
    }
}
