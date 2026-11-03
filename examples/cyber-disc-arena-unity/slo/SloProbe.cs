using System;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading.Tasks;
using CyberDiscArena;
using Kukuri.GameSdk;
using Kukuri.GameSdk.Lobby;
using Newtonsoft.Json.Linq;
using UnityEngine;
using UnityEngine.SceneManagement;
using UnityEngine.UI;

/// 性能の計測（ADR 0070 §8.4、#1984 AC-1）の Player の probe。`-slo-role host|client|viewer1〜5` のときだけ動く。
/// 測った値は確保済みの配列にため（計測が GC を増やさないように frame ごとの確保をしない）、終わるときに
/// `<work>/<role>.samples` へ `<種類> <QPC> <値...>` で書き、slo.ps1 が集計する。
/// f: frame の間隔（ticks）、g: GC の回数が増えた frame、p: 前の frame の native の poll と写しの時間（ms。poll しなかった
/// frame は無い）、m: SDK の memory（bytes）、i: キャッチの操作から確定まで
/// （ticks）、s: 配信の frame に埋めた番号、v: 視聴者が受けた番号と段数、u: スキンの発行を送った時点と版、a: スキンの適用と
/// 版、w: 発行の応答が返った時点と版、h: 新しい版の知らせ（`game_asset_updated`）の到着と版、k: 対戦の packet の受信、id: endpoint ID、pid: 作者の
/// Sidecar の endpoint ID。時刻はすべて QueryPerformanceCounter の値（proxy と同じ時計。Unity の
/// `Stopwatch.GetTimestamp()` は process の起動からの時間で、process をまたいで比べられない）。
[DefaultExecutionOrder(-1000)]
public sealed class SloProbe : MonoBehaviour
{
    /// 配信の frame に埋める番号の bit 数（左上の 1 行に、番号と、その反転を 24 px 角で並べる）。
    const int Bits = 24;
    const int Block = 24;
    /// 計測で発行するスキン（arena のロッカーに登録済みの ID）。
    const string Skin = "holo";

    [SerializeField] KukuriRunner runner;
    [SerializeField] KukuriLobby lobby;
    [SerializeField] ArenaSession session;
    [SerializeField] KukuriAccountScreen account;
    [SerializeField] ArenaSkins skins;
    [SerializeField] ArenaBroadcast broadcast;
    [SerializeField] Camera feed;

    string role;
    string work;
    StreamWriter samples;
    long lastFrame;
    long nextMemory;
    long pressed;
    bool pressing;
    bool keepPlaying;
    float heldSince = -1;
    KukuriClient hooked;
    KukuriClient publisher;
    long lastRevision = -1;
    Texture2D stamp;
    Color32[] stampPixels;
    int counter;
    string mutedSession;

    static string Arg(string name)
    {
        var args = Environment.GetCommandLineArgs();
        var index = Array.IndexOf(args, name);
        return index >= 0 && index + 1 < args.Length ? args[index + 1] : null;
    }

    [DllImport("kernel32.dll")]
    static extern bool QueryPerformanceCounter(out long value);

    [DllImport("kernel32.dll")]
    static extern bool QueryPerformanceFrequency(out long value);

    static long Now => QueryPerformanceCounter(out var value) ? value : 0;
    static readonly long Frequency = QueryPerformanceFrequency(out var value) ? value : 1;
    static bool Elsewhere() => SceneManager.GetActiveScene().name == "Boot";
    string Exchange(string name) => Path.Combine(work, name);

    /// 他の process が書き終えた直後の file は、ウイルス対策などがまだ開いていて読めないことがあるので、少しの間やり直す。
    async Task<string> Read(string name)
    {
        for (var attempt = 0; ; attempt++)
        {
            try
            {
                return File.ReadAllText(Exchange(name));
            }
            catch (IOException) when (attempt < 50)
            {
                await Task.Delay(100);
            }
        }
    }
    const int Capacity = 1 << 20;
    readonly char[] kinds = new char[Capacity];
    readonly long[] times = new long[Capacity];
    readonly long[] values = new long[Capacity];
    readonly long[] extras = new long[Capacity];
    int recorded;
    int collections;

    void Sample(string line) => samples?.WriteLine(line);

    void Record(char kind, long time, long value, long extra = long.MinValue)
    {
        if (recorded == Capacity) return;
        kinds[recorded] = kind;
        times[recorded] = time;
        values[recorded] = value;
        extras[recorded++] = extra;
    }

    void Flush()
    {
        if (samples == null) return;
        for (var index = 0; index < recorded; index++)
        {
            var value = kinds[index] == 'p' ? (values[index] / 1000.0).ToString("F3") : values[index].ToString();
            samples.WriteLine(extras[index] == long.MinValue
                ? $"{kinds[index]} {times[index]} {value}"
                : $"{kinds[index]} {times[index]} {value} {extras[index]}");
        }
        recorded = 0;
        samples.Flush();
    }

    void Awake()
    {
        if (Elsewhere())
        {
            SceneManager.LoadScene(Arg("-slo-scene") ?? "SloArena");
            return;
        }
        role = Arg("-slo-role");
        if (role == null) return;
        work = Arg("-slo-work");
        samples = new StreamWriter(Exchange(role + ".samples"), false, new UTF8Encoding(false), 1 << 20);
        runner.Settings = KukuriSettings.Create(KukuriMode.Ffi, role, Arg("-slo-profile-dir"));
        // 試験では音を出さない（Live の音は視聴を始めたらすぐ音量を 0 にする）。
        AudioListener.volume = 0;
        // 7 つの Player が CPU と GPU を取り合わないよう、描く速さを 120 frame/秒までにする（ADR 0070 §8.4、2026-11-03 の判断）。
        Application.targetFrameRate = 120;
        runner.enabled = true;
        if (session == null) return;
        session.InputSource = Bot;
        // キャッチの操作から、host がその受け止めを確定した出来事を受けるまで。受け止め損ねた（当たった・外れて相手が
        // 拾った）操作は、確定が無いので数えない。
        session.Confirmed += @event =>
        {
            if (pressed == 0) return;
            if (@event.Kind == ArenaEventKind.Catch && @event.Target == session.Local)
            {
                var now = Now;
                Record('i', now, now - pressed);
            }
            pressed = 0;
        };
    }

    async void Start()
    {
        if (role == null || Elsewhere()) return;
        try
        {
            await (role == "host" ? Host() : role == "client" ? Client() : Viewer());
            await Until(() => File.Exists(Exchange("done")), 1800, "the host did not finish");
            Flush();
            Application.Quit(0);
        }
        catch (Exception error)
        {
            Debug.Log("KGS_FAILED " + error);
            Flush();
            Application.Quit(1);
        }
    }

    /// 相手へ向き、左右に動きながら、持てば少し待って投げる。ディスクが向かってきて受け止められる距離に入った frame で
    /// キャッチを押す（早く押すと、ディスクが届くまでの待ちを測ってしまう）。
    ArenaInput Bot(int local)
    {
        var t = Time.time;
        var input = new ArenaInput { Yaw = ArenaControls.FacingYaw(session), Move = new Vector2(Mathf.Sin(t * (local == 0 ? 1.3f : 1.7f)), 0) };
        if (session.Phase != ArenaPhase.Playing) return input;
        if (session.Holder == local)
        {
            pressing = false;
            if (heldSince < 0) heldSince = t;
            if (t - heldSince > 0.6f)
            {
                heldSince = -1;
                session.Act();
            }
            return input;
        }
        heldSince = -1;
        var me = session.Players[local].Position;
        var disc = session.Disc;
        var coming = disc.Mode == DiscMode.Flying && Vector3.Dot(disc.Velocity, me - disc.Position) > 0;
        var distance = Vector2.Distance(new Vector2(me.x, me.z), new Vector2(disc.Position.x, disc.Position.z));
        if (coming && distance < ArenaRules.CatchRange && !pressing)
        {
            pressing = true;
            pressed = Now;
            session.Act();
        }
        if (!coming) pressing = false;
        return input;
    }

    static async Task Until(Func<bool> condition, double seconds, string what)
    {
        var deadline = Time.realtimeSinceStartup + seconds;
        while (!condition())
        {
            if (Time.realtimeSinceStartup > deadline) throw new TimeoutException(what);
            await Task.Yield();
        }
    }

    static Task UntilTicks(long ticks) => Until(() => Now >= ticks, (ticks - Now) / (double)Frequency + 60, "the clock");

    static async Task<JToken> Send(KukuriClient client, string command, JObject payload = null)
    {
        var response = await client.SendAsync(command, payload);
        if (!response.Ok) throw new Exception(command + ": " + response.ErrorCode);
        return response.Data;
    }

    /// 同意して「はじめる」。自分の ticket を書き、`peers` の ticket を取り込む。
    async Task Begin(params string[] peers)
    {
        if (runner.Client == null)
            await Until(() => runner.Client != null || runner.OpenError != null, 60, "the SDK did not open");
        if (runner.Client == null) throw new Exception("the SDK did not open: " + runner.OpenError?.Code);
        await Until(() => account.Screen == "consent", 60, "the consent screen did not open");
        await account.Accept();
        await Until(() => account.Screen == "start", 60, "the start screen did not open");
        await account.Begin();
        await Until(() => account.Screen == "playing", 60, "the game did not start");
        // 先に試験の topic を購読し（ロビー・ロッカー・モニター）、購読が始まってから ticket を書いて相手を取り込む。取り込んだ
        // 相手は、その時点で購読している topic にだけ加わる。順番が揃わないと、topic の gossip が 2 つに分かれ、片方に
        // 送った部屋や配信の知らせが届かない。
        var topic = Arg("-slo-topic");
        if (lobby != null) lobby.Topic = topic;
        if (skins != null) skins.Topic = topic;
        broadcast.Topic = topic;
        // 視聴者の scene には lobby が無いので、lobby と同じ購読をする（ロビーにいる人と同じ）。
        if (lobby == null)
            await Send(runner.Client, "set_topic_gossip_enabled", new JObject { ["topic"] = topic, ["enabled"] = true });
        var deadline = Time.realtimeSinceStartup + 60;
        while (!((JArray)(await Send(runner.Client, "get_sync_status"))["topic_diagnostics"]).Any(entry => (string)entry["topic"] == topic))
        {
            if (Time.realtimeSinceStartup > deadline) throw new TimeoutException("the topic was not subscribed");
            await Task.Delay(500);
        }
        var ticket = (string)await Send(runner.Client, "get_local_peer_ticket");
        Sample($"id {Now} {ticket.Split('@')[0]}");
        File.WriteAllText(Exchange(role + ".ticket.tmp"), ticket);
        File.Move(Exchange(role + ".ticket.tmp"), Exchange(role + ".ticket"));
        foreach (var peer in peers)
        {
            await Until(() => File.Exists(Exchange(peer + ".ticket")), 180, "no ticket from " + peer);
            await Send(runner.Client, "import_peer_ticket", new JObject { ["ticket"] = await Read(peer + ".ticket") });
        }
    }

    /// 作者のスキンを選び、着るまで待つ（作者の ticket と公開鍵は host が書く）。
    async Task Wear()
    {
        await Until(() => File.Exists(Exchange("publisher.info")), 180, "the publisher did not start");
        var lines = (await Read("publisher.info")).Split('\n');
        await Send(runner.Client, "import_peer_ticket", new JObject { ["ticket"] = lines[0] });
        skins.PublisherPubkey = lines[1];
        await Until(() => skins.Published.Contains(Skin), 180, "the locker did not show the skin");
        skins.Select(Skin);
        await Until(() => skins.Loader(0).AppliedRevision >= 1, 180, "the skin was not worn");
    }

    async Task Host()
    {
        var viewers = Enumerable.Range(1, 5).Select(index => "viewer" + index).ToArray();
        await Begin(viewers.Prepend("client").ToArray());
        publisher = KukuriClient.Open(KukuriMode.Sidecar, "publisher", Arg("-slo-publisher-dir"));
        await Send(publisher, "accept_app_consents", new JObject
        {
            ["documents"] = new JArray(((JArray)(await Send(publisher, "get_app_consent_status"))["documents"]).Select(document =>
                new JObject { ["slug"] = document["slug"], ["version"] = document["currentVersion"] })),
            ["language"] = "ja",
            ["age_attested"] = true,
        });
        foreach (var peer in new[] { "host", "client" })
            await Send(publisher, "import_peer_ticket", new JObject { ["ticket"] = await Read(peer + ".ticket") });
        var ticket = (string)await Send(publisher, "get_local_peer_ticket");
        Sample($"pid {Now} {ticket.Split('@')[0]}");
        await Send(runner.Client, "import_peer_ticket", new JObject { ["ticket"] = ticket });
        var pubkey = (string)(await Send(publisher, "get_sync_status"))["local_author_pubkey"];
        await Publish(WriteGlb(1));
        File.WriteAllText(Exchange("publisher.tmp"), ticket + "\n" + pubkey);
        File.Move(Exchange("publisher.tmp"), Exchange("publisher.info"));
        await Wear();
        var update = WriteGlb(2);
        CreateStamp();
        await Until(() => viewers.Append("client").All(peer => File.Exists(Exchange(peer + ".ready"))), 600, "the players did not gather");
        broadcast.BroadcastNext = true;
        await lobby.CreateRoomAsync("slo");
        await Until(() => session.Phase == ArenaPhase.Playing, 180, "the match did not start");
        keepPlaying = true;
        await Until(() => broadcast.LiveSession != null, 60, "the live did not start");
        await Until(() => viewers.All(viewer => File.Exists(Exchange(viewer + ".watching"))), 300, "the viewers did not watch");
        // 準備の 10 秒の後に測る。
        var start = Now + 10 * Frequency;
        var end = start + int.Parse(Arg("-slo-seconds")) * Frequency;
        File.WriteAllText(Exchange("window"), $"{start} {end}");
        await UntilTicks(start + 60 * Frequency);
        var sent = Now;
        var revision = await Publish(update);
        Record('u', sent, revision);
        Record('w', Now, revision);
        await UntilTicks(end);
        if (Arg("-slo-blackout") == "1")
        {
            File.WriteAllText(Exchange("blackout-request"), "now");
            await Until(() => File.Exists(Exchange("blackout")) && File.ReadAllText(Exchange("blackout")).Trim().Length > 0, 60, "the proxy did not black out");
            var restored = long.Parse(File.ReadAllText(Exchange("blackout")).Trim().Split(' ')[1]);
            await UntilTicks(restored + 25 * Frequency);
        }
        keepPlaying = false;
        File.WriteAllText(Exchange("done"), "done");
    }

    async Task Client()
    {
        await Begin("host");
        await Wear();
        await Gather();
        await Until(() => lobby.Rooms.Any(room => room.Title == "slo"), 180, "the room did not arrive");
        await lobby.JoinRoomAsync(lobby.Rooms.First(room => room.Title == "slo").RoomId);
        await Until(() => session.Phase == ArenaPhase.Playing, 180, "the match did not start");
    }

    /// 試合の前にロビーへそろう。部屋と配信の開始の知らせを受けられるよう、topic の近隣とつながってから host に知らせる
    /// （つながる前に送られた知らせは届かない）。
    async Task Gather()
    {
        var topic = Arg("-slo-topic");
        var deadline = Time.realtimeSinceStartup + 600;
        while (!((JArray)(await Send(runner.Client, "get_sync_status"))["topic_diagnostics"]).Any(entry =>
                   (string)entry["topic"] == topic && (int)entry["peer_count"] > 0))
        {
            if (Time.realtimeSinceStartup > deadline) throw new TimeoutException("no neighbor on the topic");
            await Task.Delay(1000);
        }
        File.WriteAllText(Exchange(role + ".ready"), "ready");
    }

    async Task Viewer()
    {
        await Begin("host");
        await Gather();
        try
        {
            await Until(() => broadcast.Viewer.SessionId != null && broadcast.Viewer.FramesShown > 30, 300, "the monitors did not show the live");
        }
        catch (TimeoutException)
        {
            // 見つからなかった理由を残す（配信の一覧と、topic の近隣・最後に受けた時刻）。
            Debug.Log("KGS_LIVES " + (await runner.Client.SendAsync("list_live_sessions", new JObject { ["topic"] = Arg("-slo-topic") })).Data);
            Debug.Log("KGS_SYNC " + ((JObject)(await Send(runner.Client, "get_sync_status")))["topic_diagnostics"]);
            Debug.Log($"KGS_VIEWER {broadcast.Viewer.SessionId} {broadcast.Viewer.State} {broadcast.Viewer.FramesShown}");
            throw;
        }
        File.WriteAllText(Exchange(role + ".watching"), "watching");
    }

    async Task<long> Publish(string path)
    {
        var release = await Send(publisher, "publish_game_asset", new JObject
        {
            ["topic"] = Arg("-slo-topic"),
            ["application_id"] = "com.kukuri.cyber-disc-arena",
            ["asset_id"] = Skin,
            ["format"] = new JObject { ["kind"] = "glb" },
            ["path"] = path,
        });
        return (long)release["revision"];
    }

    /// 5 MiB の glb（体の前に置く 300×300 の格子の mesh と、640×640 の雑音の PNG の texture）。版ごとに形と雑音を変える。
    string WriteGlb(int variant)
    {
        const int n = 300;
        var random = new System.Random(variant);
        var texture = new Texture2D(640, 640, TextureFormat.RGB24, false);
        var noise = new byte[640 * 640 * 3];
        random.NextBytes(noise);
        texture.SetPixelData(noise, 0);
        texture.Apply();
        var png = texture.EncodeToPNG();
        Destroy(texture);
        using var bin = new MemoryStream();
        using var writer = new BinaryWriter(bin);
        var min = new[] { float.MaxValue, float.MaxValue, float.MaxValue };
        var max = new[] { float.MinValue, float.MinValue, float.MinValue };
        for (var j = 0; j < n; j++)
            for (var i = 0; i < n; i++)
            {
                var p = new[] { (i / (n - 1f) - 0.5f) * 0.4f, 1.2f + (j / (n - 1f) - 0.5f) * 0.4f, 0.35f + 0.02f * Mathf.Sin(i * 0.1f * variant + j * 0.07f) };
                for (var k = 0; k < 3; k++)
                {
                    writer.Write(p[k]);
                    min[k] = Mathf.Min(min[k], p[k]);
                    max[k] = Mathf.Max(max[k], p[k]);
                }
            }
        var uvOffset = bin.Position;
        for (var j = 0; j < n; j++)
            for (var i = 0; i < n; i++)
            {
                writer.Write(i / (n - 1f));
                writer.Write(j / (n - 1f));
            }
        var indexOffset = bin.Position;
        for (var j = 0; j < n - 1; j++)
            for (var i = 0; i < n - 1; i++)
            {
                var a = (uint)(j * n + i);
                foreach (var index in new[] { a, a + n, a + 1, a + 1, a + n, a + n + 1 }) writer.Write(index);
            }
        var imageOffset = bin.Position;
        writer.Write(png);
        writer.Flush();
        var used = bin.Position;
        string Json(long bufferLength) => new JObject
        {
            ["asset"] = new JObject { ["version"] = "2.0" },
            ["scene"] = 0,
            ["scenes"] = new JArray(new JObject { ["nodes"] = new JArray(0) }),
            ["nodes"] = new JArray(new JObject { ["mesh"] = 0 }),
            ["meshes"] = new JArray(new JObject
            {
                ["primitives"] = new JArray(new JObject
                {
                    ["attributes"] = new JObject { ["POSITION"] = 0, ["TEXCOORD_0"] = 1 },
                    ["indices"] = 2,
                    ["material"] = 0,
                }),
            }),
            ["materials"] = new JArray(new JObject { ["pbrMetallicRoughness"] = new JObject { ["baseColorTexture"] = new JObject { ["index"] = 0 } } }),
            ["textures"] = new JArray(new JObject { ["source"] = 0 }),
            ["images"] = new JArray(new JObject { ["bufferView"] = 3, ["mimeType"] = "image/png" }),
            ["accessors"] = new JArray(
                new JObject { ["bufferView"] = 0, ["componentType"] = 5126, ["count"] = n * n, ["type"] = "VEC3", ["min"] = new JArray(min), ["max"] = new JArray(max) },
                new JObject { ["bufferView"] = 1, ["componentType"] = 5126, ["count"] = n * n, ["type"] = "VEC2" },
                new JObject { ["bufferView"] = 2, ["componentType"] = 5125, ["count"] = (n - 1) * (n - 1) * 6, ["type"] = "SCALAR" }),
            ["bufferViews"] = new JArray(
                new JObject { ["buffer"] = 0, ["byteOffset"] = 0, ["byteLength"] = uvOffset },
                new JObject { ["buffer"] = 0, ["byteOffset"] = uvOffset, ["byteLength"] = indexOffset - uvOffset },
                new JObject { ["buffer"] = 0, ["byteOffset"] = indexOffset, ["byteLength"] = imageOffset - indexOffset },
                new JObject { ["buffer"] = 0, ["byteOffset"] = imageOffset, ["byteLength"] = png.Length }),
            ["buffers"] = new JArray(new JObject { ["byteLength"] = bufferLength }),
        }.ToString(Newtonsoft.Json.Formatting.None);
        // 全体をちょうど 5 MiB にする（足りない分は buffer の末尾の、どこからも参照しない bytes）。
        const long total = 5 << 20;
        var json = Encoding.UTF8.GetBytes(Json(0).PadRight(Json(0).Length + 16));
        var jsonLength = (json.Length + 3) & ~3;
        var binLength = total - 12 - 8 - jsonLength - 8;
        if (binLength < used) throw new Exception("the glb is larger than 5 MiB");
        json = Encoding.UTF8.GetBytes(Json(binLength));
        var padded = new byte[jsonLength];
        for (var i = 0; i < padded.Length; i++) padded[i] = (byte)' ';
        Array.Copy(json, padded, json.Length);
        var path = Exchange($"skin-{variant}.glb");
        using (var file = new BinaryWriter(File.Create(path)))
        {
            file.Write(0x46546C67u);
            file.Write(2u);
            file.Write((uint)total);
            file.Write((uint)jsonLength);
            file.Write(0x4E4F534Au);
            file.Write(padded);
            file.Write((uint)binLength);
            file.Write(0x004E4942u);
            file.Write(bin.GetBuffer(), 0, (int)used);
            file.Write(new byte[binLength - used]);
        }
        return path;
    }

    /// 配信の camera の左上に、frame ごとの番号を白黒の四角で描く UI を置く（配信の RenderTexture に入る）。
    void CreateStamp()
    {
        stamp = new Texture2D(Bits * 2, 1, TextureFormat.RGBA32, false) { filterMode = FilterMode.Point };
        stampPixels = new Color32[Bits * 2];
        var layer = Enumerable.Range(0, 32).First(index => (feed.cullingMask & (1 << index)) != 0);
        var canvas = new GameObject("slo-stamp", typeof(Canvas)) { layer = layer }.GetComponent<Canvas>();
        canvas.renderMode = RenderMode.ScreenSpaceCamera;
        canvas.worldCamera = feed;
        canvas.planeDistance = feed.nearClipPlane + 0.01f;
        var image = new GameObject("bits", typeof(RawImage)) { layer = layer }.GetComponent<RawImage>();
        image.transform.SetParent(canvas.transform, false);
        image.texture = stamp;
        var rect = image.rectTransform;
        rect.anchorMin = rect.anchorMax = rect.pivot = new Vector2(0, 1);
        rect.anchoredPosition = Vector2.zero;
        rect.sizeDelta = new Vector2(Bits * 2 * Block, Block);
    }

    void DrawStamp(int value)
    {
        for (var bit = 0; bit < Bits; bit++)
        {
            var on = ((value >> bit) & 1) == 1;
            stampPixels[bit] = on ? new Color32(255, 255, 255, 255) : new Color32(0, 0, 0, 255);
            stampPixels[Bits + bit] = on ? new Color32(0, 0, 0, 255) : new Color32(255, 255, 255, 255);
        }
        stamp.SetPixels32(stampPixels);
        stamp.Apply(false);
    }

    /// 受けた frame の Y 面から番号を読む（上端と下端の行を試し、番号と反転が合う方を使う）。
    void OnFrame(KukuriLiveFrame frame)
    {
        var now = Now;
        if (frame.Width < Bits * 2 * Block) return;
        foreach (var row in new[] { Block / 2, frame.Height - Block / 2 - 1 })
        {
            var value = Decode(frame.I420, frame.Width, row);
            if (value < 0) continue;
            Record('v', now, value, frame.Hops);
            return;
        }
    }

    static int Decode(byte[] luma, int width, int row)
    {
        var value = 0;
        for (var index = 0; index < Bits * 2; index++)
        {
            var center = index * Block + Block / 2;
            var sum = 0;
            for (var dy = -3; dy <= 3; dy++)
                for (var dx = -3; dx <= 3; dx++)
                    sum += luma[(row + dy) * width + center + dx];
            var on = sum / 49 > 128;
            if (index < Bits)
            {
                if (on) value |= 1 << index;
            }
            else if (on == (((value >> (index - Bits)) & 1) == 1))
                return -1;
        }
        return value;
    }

    /// 測る間は試合を終わらせない（host の裁定の残り時間と得点を戻す。負荷は変えない）。
    void KeepPlaying()
    {
        var rules = typeof(ArenaSession).GetField("rules", BindingFlags.NonPublic | BindingFlags.Instance)?.GetValue(session);
        if (rules == null) return;
        var left = rules.GetType().GetProperty("TimeLeft");
        if ((float)left.GetValue(rules) < 60) left.SetValue(rules, 120f);
        var scores = (byte[])rules.GetType().GetField("Scores").GetValue(rules);
        for (var player = 0; player < 2; player++)
            if (scores[player] >= ArenaRules.WinningScore - 1) scores[player] = 0;
    }

    void Update()
    {
        if (role == null || Elsewhere()) return;
        var now = Now;
        if (session != null && lastFrame != 0) Record('f', now, now - lastFrame);
        if (GC.CollectionCount(0) != collections)
        {
            collections = GC.CollectionCount(0);
            Record('g', now, collections);
        }
        lastFrame = now;
        var client = runner.Client;
        if (client != null)
        {
            if (client.LastPollMilliseconds is double poll)
                Record('p', now, (long)(poll * 1000));
            if (client != hooked)
            {
                hooked = client;
                client.LiveFrameReceived += OnFrame;
                client.EventReceived += @event =>
                {
                    if ((string)@event["type"] == "game_asset_updated")
                        Record('h', Now, (long)@event["revision"]);
                };
                if (session != null) client.PacketReceived += _ => Record('k', Now, 0);
            }
        }
        if (now >= nextMemory)
        {
            Record('m', now, KukuriClient.AllocatedBytes);
            nextMemory = now + Frequency;
        }
        if (skins != null && skins.Loader(0).AppliedRevision != lastRevision)
        {
            lastRevision = skins.Loader(0).AppliedRevision;
            Record('a', now, lastRevision);
        }
        if (stamp != null)
        {
            DrawStamp(++counter);
            if (broadcast.LiveSession != null) Record('s', now, counter);
        }
        if (keepPlaying) KeepPlaying();
        publisher?.Pump();
        var watching = broadcast != null ? broadcast.Viewer.SessionId : null;
        if (watching != null && watching != mutedSession && client != null)
        {
            mutedSession = watching;
            _ = client.SendAsync("set_live_volume", new JObject { ["session_id"] = watching, ["volume"] = 0.0 });
        }
        // 部屋の一覧は、待っている部屋が来るまで短い間隔で読み直す。
        if (role == "client" && client != null && !session.InMatch && Time.frameCount % 30 == 0)
            _ = lobby.RefreshAsync();
    }

    void OnApplicationQuit() => Flush();
}
