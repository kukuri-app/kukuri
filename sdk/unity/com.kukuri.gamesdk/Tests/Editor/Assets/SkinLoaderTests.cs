using System;
using System.Collections;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using GLTFast.Export;
using Kukuri.GameSdk.Editor;
using Newtonsoft.Json.Linq;
using NUnit.Framework;
using UnityEngine;
using UnityEngine.TestTools;
using Debug = UnityEngine.Debug;
using Object = UnityEngine.Object;

namespace Kukuri.GameSdk.Assets.Tests
{
    /// スキンの動的な差し替え（#1980 AC-2）。発行者（開発用の Sidecar）と Player（FFI）の 2 つの profile で、自分で作った
    /// .glb を発行 → 更新の知らせ → P2P の取得 → 検証 → 再起動なしの差し替えを確かめる。壊れた版では今のスキンを保ち、
    /// 差し替えを繰り返しても資源が増えず、適用中の frame は契約の予算（ADR 0070 §8.4 の 33 ms）に収まる。
    /// test-graphics.ps1 が glTFast 6.20.0 を入れた project で、keyring を切って loopback だけで動かす。
    public sealed class SkinLoaderTests
    {
        const string Application = "com.kukuri.test.skins";

        KukuriClient publisher;
        KukuriClient player;
        GameObject loaderObject;
        double slowestFrameMilliseconds;

        [TearDown]
        public void TearDown()
        {
            if (loaderObject != null)
                Object.DestroyImmediate(loaderObject);
            publisher?.Dispose();
            player?.Dispose();
            SidecarLauncher.StopAll();
        }

        static string NewDirectory() =>
            Path.Combine(Path.GetTempPath(), "kgs-" + Guid.NewGuid().ToString("N").Substring(0, 12));

        /// frame を進める。frame の間隔（両方の client の pump と、loader の非同期の続きを含む）の最大を測る。
        IEnumerator Frames(Func<bool> condition, string what)
        {
            var clock = Stopwatch.StartNew();
            var frame = Stopwatch.StartNew();
            while (!condition())
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 120, what);
                publisher?.Pump();
                player?.Pump();
                yield return null;
                slowestFrameMilliseconds = Math.Max(slowestFrameMilliseconds, frame.Elapsed.TotalMilliseconds);
                frame.Restart();
            }
        }

        IEnumerator Call(KukuriClient client, string command, JObject payload, Action<JToken> result = null)
        {
            var request = client.SendAsync(command, payload, timeoutMilliseconds: 120_000);
            yield return Frames(() => request.IsCompleted, $"{command} did not answer");
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

        /// primitive を 1 つ置いた .glb を書く（自己生成の fixture）。
        static string Export(PrimitiveType shape)
        {
            var source = GameObject.CreatePrimitive(shape);
            var export = new GameObjectExport(new ExportSettings { Format = GltfFormat.Binary });
            export.AddScene(new[] { source });
            var path = Path.Combine(Path.GetTempPath(), $"kgs-skin-{Guid.NewGuid():N}.glb");
            Assert.IsTrue(export.SaveToFileAndDispose(path, true).Result, "export the fixture");
            Object.DestroyImmediate(source);
            return path;
        }

        IEnumerator Publish(string topic, string path, Action<long> revision, string asset = "skin", string kind = "glb",
            string buildTarget = null)
        {
            var format = new JObject { ["kind"] = kind };
            if (kind == "asset_bundle")
            {
                format["unity_version"] = UnityEngine.Application.unityVersion;
                format["build_target"] = buildTarget;
            }
            yield return Call(publisher, "publish_game_asset", new JObject
            {
                ["topic"] = topic,
                ["application_id"] = Application,
                ["asset_id"] = asset,
                ["format"] = format,
                ["path"] = path,
            }, data => revision((long)data["revision"]));
        }

        /// primitive の prefab だけを入れた AssetBundle を、同じ bundle 名で作る（版ごとに作り直す）。
        static string Bundle(PrimitiveType shape)
        {
            const string prefab = "Assets/KgsSkinBundle.prefab";
            var source = GameObject.CreatePrimitive(shape);
            UnityEditor.PrefabUtility.SaveAsPrefabAsset(source, prefab);
            Object.DestroyImmediate(source);
            var output = Path.Combine(Path.GetTempPath(), "kgs-bundle-" + Guid.NewGuid().ToString("N").Substring(0, 8));
            Directory.CreateDirectory(output);
            UnityEditor.BuildPipeline.BuildAssetBundles(output,
                new[] { new UnityEditor.AssetBundleBuild { assetBundleName = "kgs-skin", assetNames = new[] { prefab } } },
                UnityEditor.BuildAssetBundleOptions.None, UnityEditor.BuildTarget.StandaloneWindows64);
            UnityEditor.AssetDatabase.DeleteAsset(prefab);
            return Path.Combine(output, "kgs-skin");
        }

        /// 1 色で塗った PNG の texture を書く（自己生成の fixture）。
        static string Png(Color color)
        {
            var texture = new Texture2D(4, 4);
            texture.SetPixels(Enumerable.Repeat(color, 16).ToArray());
            var path = Path.Combine(Path.GetTempPath(), $"kgs-banner-{Guid.NewGuid():N}.png");
            File.WriteAllBytes(path, texture.EncodeToPNG());
            Object.DestroyImmediate(texture);
            return path;
        }

        static Color Shown(Renderer renderer) => ((Texture2D)renderer.sharedMaterial.mainTexture).GetPixel(0, 0);

        static int Vertices(KukuriSkinLoader loader) =>
            loader.Instance.GetComponentsInChildren<MeshFilter>(true).Sum(filter => filter.sharedMesh.vertexCount);

        /// graphics があれば、表示中のスキンを camera で描いて PNG に書く（描画の証拠。-nographics では書かない）。
        static void Capture(string name)
        {
            if (SystemInfo.graphicsDeviceType == UnityEngine.Rendering.GraphicsDeviceType.Null)
                return;
            var camera = new GameObject("capture").AddComponent<Camera>();
            camera.transform.position = new Vector3(0, 1.2f, -3);
            camera.transform.LookAt(Vector3.zero);
            camera.clearFlags = CameraClearFlags.SolidColor;
            camera.backgroundColor = new Color(0.2f, 0.3f, 0.4f);
            var light = new GameObject("light").AddComponent<Light>();
            light.type = LightType.Directional;
            light.transform.rotation = Quaternion.Euler(50, -30, 0);
            var target = new RenderTexture(480, 320, 24);
            camera.targetTexture = target;
            camera.Render();
            RenderTexture.active = target;
            var image = new Texture2D(480, 320, TextureFormat.RGB24, false);
            image.ReadPixels(new Rect(0, 0, 480, 320), 0, 0);
            image.Apply();
            RenderTexture.active = null;
            var path = Path.Combine(Path.GetTempPath(), $"kgs-skin-{name}.png");
            File.WriteAllBytes(path, image.EncodeToPNG());
            Debug.Log("KGS_CAPTURE " + path);
            camera.targetTexture = null;
            Object.DestroyImmediate(image);
            Object.DestroyImmediate(target);
            Object.DestroyImmediate(camera.gameObject);
            Object.DestroyImmediate(light.gameObject);
        }

        static int[] Resources() => new[]
        {
            UnityEngine.Resources.FindObjectsOfTypeAll<Mesh>().Length,
            UnityEngine.Resources.FindObjectsOfTypeAll<Material>().Length,
            UnityEngine.Resources.FindObjectsOfTypeAll<Texture2D>().Length,
        };

        [UnityTest]
        public IEnumerator ASignedSkinIsFetchedVerifiedAndSwappedWithoutARestart()
        {
            var topic = "kukuri:topic:test-skins-" + Guid.NewGuid().ToString("N").Substring(0, 8);
            // 発行は開発用の Sidecar だけに許される（ゲームの Player は FFI）。
            publisher = KukuriClient.Open(KukuriMode.Sidecar, "publisher", NewDirectory());
            player = KukuriClient.Open(KukuriMode.Ffi, "player", NewDirectory());
            foreach (var client in new[] { publisher, player })
            {
                yield return Accept(client);
                yield return Call(client, "set_topic_gossip_enabled", new JObject { ["topic"] = topic, ["enabled"] = true });
            }
            string ticketA = null, ticketB = null, publisherKey = null;
            yield return Call(publisher, "get_local_peer_ticket", null, data => ticketA = (string)data);
            yield return Call(player, "get_local_peer_ticket", null, data => ticketB = (string)data);
            yield return Call(player, "import_peer_ticket", new JObject { ["ticket"] = ticketA });
            yield return Call(publisher, "import_peer_ticket", new JObject { ["ticket"] = ticketB });
            yield return Call(publisher, "get_sync_status", null, data => publisherKey = (string)data["local_author_pubkey"]);

            long revision = 0;
            yield return Publish(topic, Export(PrimitiveType.Cube), value => revision = value);
            Assert.AreEqual(1, revision);
            loaderObject = new GameObject("loader");
            var loader = loaderObject.AddComponent<KukuriSkinLoader>();
            string failure = null;
            loader.Failed += reason => failure = reason;
            loader.Topic = topic;
            loader.PublisherPubkey = publisherKey;
            loader.ApplicationId = Application;
            loader.AssetId = "skin";
            loader.Client = player;
            yield return Frames(() => loader.AppliedRevision == 1, "the first skin was not applied");
            var cube = Vertices(loader);
            Assert.Greater(cube, 0);
            Capture("1-cube");

            // 新しい版を発行すると、更新の知らせで取得・検証して、再起動せずに差し替える。
            var first = loader.Instance;
            yield return Publish(topic, Export(PrimitiveType.Sphere), value => revision = value);
            yield return Frames(() => loader.AppliedRevision == revision, "the new skin was not applied");
            Assert.IsTrue(first == null, "the previous skin was released");
            Assert.AreNotEqual(cube, Vertices(loader));
            var sphere = Vertices(loader);
            Capture("2-sphere");

            // 読めない版（署名と hash は正しいが glTF として壊れている）では、今のスキンを保つ。
            var broken = Path.Combine(Path.GetTempPath(), $"kgs-broken-{Guid.NewGuid():N}.glb");
            File.WriteAllBytes(broken, new byte[] { (byte)'g', (byte)'l', (byte)'T', (byte)'F', 2, 0, 0, 0, 1, 2, 3, 4 });
            LogAssert.ignoreFailingMessages = true;
            yield return Publish(topic, broken, value => revision = value);
            yield return Frames(() => failure != null, "the broken skin was not reported");
            LogAssert.ignoreFailingMessages = false;
            Assert.AreEqual("invalid_asset", failure);
            Assert.AreEqual(revision - 1, loader.AppliedRevision, "the current skin is kept");
            Assert.AreEqual(sphere, Vertices(loader));
            Capture("3-kept-after-broken");

            // 差し替えを繰り返しても、mesh・material・texture は増えない。
            var before = Resources();
            slowestFrameMilliseconds = 0;
            foreach (var shape in new[] { PrimitiveType.Cube, PrimitiveType.Sphere, PrimitiveType.Cube, PrimitiveType.Sphere })
            {
                yield return Publish(topic, Export(shape), value => revision = value);
                yield return Frames(() => loader.AppliedRevision == revision, "a repeated skin was not applied");
            }
            var after = Resources();
            Debug.Log($"KGS_SKIN resources before={string.Join(",", before)} after={string.Join(",", after)} slowest_frame_ms={slowestFrameMilliseconds:F1}");
            for (var index = 0; index < before.Length; index++)
                Assert.LessOrEqual(after[index], before[index], "repeated swaps release the previous skins");
            Assert.LessOrEqual(slowestFrameMilliseconds, 33, "a frame while applying stays within the budget");

            // texture の fixture（PNG）でも、差し替え・壊れた版での維持・資源の回収が同じく働く。
            var quad = GameObject.CreatePrimitive(PrimitiveType.Quad);
            quad.transform.SetParent(loaderObject.transform, false);
            var bannerObject = new GameObject("banner");
            bannerObject.transform.SetParent(loaderObject.transform, false);
            var banner = bannerObject.AddComponent<KukuriSkinLoader>();
            string bannerFailure = null;
            banner.Failed += reason => bannerFailure = reason;
            typeof(KukuriSkinLoader).GetField("textureTarget", System.Reflection.BindingFlags.NonPublic |
                System.Reflection.BindingFlags.Instance).SetValue(banner, quad.GetComponent<Renderer>());
            banner.Topic = topic;
            banner.PublisherPubkey = publisherKey;
            banner.ApplicationId = Application;
            banner.AssetId = "banner";
            yield return Publish(topic, Png(Color.red), value => revision = value, "banner", "png");
            banner.Client = player;
            yield return Frames(() => banner.AppliedRevision == revision, "the first banner was not applied");
            Assert.AreEqual(Color.red, Shown(quad.GetComponent<Renderer>()));
            yield return Publish(topic, Png(Color.blue), value => revision = value, "banner", "png");
            yield return Frames(() => banner.AppliedRevision == revision, "the new banner was not applied");
            Assert.AreEqual(Color.blue, Shown(quad.GetComponent<Renderer>()));
            var brokenPng = Path.Combine(Path.GetTempPath(), $"kgs-broken-{Guid.NewGuid():N}.png");
            File.WriteAllBytes(brokenPng, new byte[] { 0x89, (byte)'P', (byte)'N', (byte)'G', 13, 10, 26, 10, 0, 1, 2, 3 });
            yield return Publish(topic, brokenPng, value => revision = value, "banner", "png");
            yield return Frames(() => bannerFailure != null, "the broken banner was not reported");
            Assert.AreEqual("invalid_asset", bannerFailure);
            Assert.AreEqual(Color.blue, Shown(quad.GetComponent<Renderer>()), "the current banner is kept");
            var textures = UnityEngine.Resources.FindObjectsOfTypeAll<Texture2D>().Length;
            foreach (var color in new[] { Color.green, Color.yellow, Color.cyan })
            {
                yield return Publish(topic, Png(color), value => revision = value, "banner", "png");
                yield return Frames(() => banner.AppliedRevision == revision, "a repeated banner was not applied");
                Assert.AreEqual(color, Shown(quad.GetComponent<Renderer>()));
            }
            Assert.LessOrEqual(UnityEngine.Resources.FindObjectsOfTypeAll<Texture2D>().Length, textures,
                "repeated banners release the previous textures");
            Capture("4-banner");

            // AssetBundle: 同じ bundle 名で作り直した新しい版にも差し替わる。build の対象が合わない版は読まない。
            var bundleObject = new GameObject("bundle");
            bundleObject.transform.SetParent(loaderObject.transform, false);
            bundleObject.transform.localPosition = new Vector3(5, 0, 0);
            var bundled = bundleObject.AddComponent<KukuriSkinLoader>();
            string bundleFailure = null;
            bundled.Failed += reason => bundleFailure = reason;
            bundled.Topic = topic;
            bundled.PublisherPubkey = publisherKey;
            bundled.ApplicationId = Application;
            bundled.AssetId = "bundle";
            yield return Publish(topic, Bundle(PrimitiveType.Cube), value => revision = value, "bundle", "asset_bundle",
                "StandaloneWindows64");
            bundled.Client = player;
            yield return Frames(() => bundled.AppliedRevision == revision, "the first bundle was not applied");
            var bundledCube = Vertices(bundled);
            yield return Publish(topic, Bundle(PrimitiveType.Sphere), value => revision = value, "bundle", "asset_bundle",
                "StandaloneWindows64");
            yield return Frames(() => bundled.AppliedRevision == revision, "the rebuilt bundle was not applied");
            Assert.AreNotEqual(bundledCube, Vertices(bundled), "the bundle with the same name was swapped");
            yield return Publish(topic, Bundle(PrimitiveType.Cube), value => revision = value, "bundle", "asset_bundle",
                "Android");
            yield return Frames(() => bundleFailure != null, "the incompatible bundle was not reported");
            Assert.AreEqual("incompatible_asset_bundle", bundleFailure);
            Assert.AreEqual(revision - 1, bundled.AppliedRevision, "the current bundle is kept");
        }
    }
}
