using System.Linq;
using System.Threading.Tasks;
using Kukuri.GameSdk;
using Kukuri.GameSdk.Lobby;
using Newtonsoft.Json.Linq;
using UnityEngine;

namespace CyberDiscArena
{
    /// 試合の配信と 4 面モニターとゲーム外からの応援（ADR 0070 §5.5）。
    /// - 部屋を作る前に「配信する」を選んでおくと、host は試合の開始で ring を見下ろす camera の映像を公開の Live にし、
    ///   試合が終わると止める（既定は配信しない。2026-11-01 のユーザー判断）。
    /// - 4 面モニターは、対戦中は自分の試合（配信していれば配信の画面）、ロビーにいる間は配信中の他の試合を Live で受けて映す。
    /// - 配信の開始 post へのリアクション（desktop・Web の SNS から、またはロビーで観戦中の人の R 長押しの応援）が届くと、
    ///   ring の上に応援の演出を出す。
    [DefaultExecutionOrder(100)]
    public sealed class ArenaBroadcast : MonoBehaviour
    {
        /// R 長押しの応援（リアクションの絵文字と、演出に出す文字）。
        static readonly (string emoji, string label)[] Cheers = { ("👏", "CLAP!"), ("🔥", "FIRE!"), ("🎉", "GG!") };

        [SerializeField] KukuriRunner runner;
        [SerializeField] KukuriAccountScreen account;
        [SerializeField] KukuriLobby lobby;
        [SerializeField] ArenaSession session;
        [SerializeField] ArenaCheer cheer;
        [SerializeField] KukuriLiveBroadcaster broadcaster;
        [SerializeField] KukuriLiveViewer viewer;
        [SerializeField] Camera feed;
        [SerializeField] Renderer[] monitors = new Renderer[4];

        MaterialPropertyBlock block;
        RenderTexture local;
        KukuriClient subscribed;
        bool busy;
        float nextTry;

        /// 次に作る部屋の試合を配信するか（部屋を作る前に選ぶ）。
        public bool BroadcastNext { get; set; }
        /// 配信中の自分の live と、その開始 post。
        public string LiveSession { get; private set; }
        public string LivePost { get; private set; }
        public KukuriLiveViewer Viewer => viewer;

        void Awake() => viewer.ReactionReceived += reaction => cheer.Spawn(Label(reaction));

        void Update()
        {
            var client = runner.Client;
            if (client != subscribed)
            {
                if (subscribed != null)
                    subscribed.EventReceived -= OnEvent;
                subscribed = client;
                if (client != null)
                    client.EventReceived += OnEvent;
            }
            var playing = session.InMatch && (session.Phase == ArenaPhase.Countdown || session.Phase == ArenaPhase.Playing);
            if (!busy && client != null && Time.unscaledTime >= nextTry)
            {
                if (session.IsHost && BroadcastNext && LiveSession == null && playing)
                    _ = Step(() => StartLive(client));
                else if (LiveSession != null && !playing)
                    _ = Step(() => StopLive(client));
                else if (viewer.SessionId != null && (session.InMatch || viewer.State == "ended"))
                    _ = Step(viewer.StopWatching);
                else if (!session.InMatch && account.Screen == "playing" && viewer.SessionId == null)
                    _ = Step(() => WatchOther(client));
            }
            // 対戦中は ring を見下ろす camera を映す（配信していなければ手元の texture へ）。
            feed.enabled = session.InMatch;
            if (session.InMatch && !broadcaster.Broadcasting)
                feed.targetTexture = local ??= new RenderTexture(640, 360, 24);
            Texture shown = broadcaster.Broadcasting ? broadcaster.Target : session.InMatch ? local : viewer.Output;
            block ??= new MaterialPropertyBlock();
            block.SetTexture("_BaseMap", shown != null ? shown : Texture2D.blackTexture);
            foreach (var monitor in monitors)
                monitor.SetPropertyBlock(block);
        }

        // ロビーで観戦している間は、北のモニターを正面から見る（ArenaView の camera の後に動かす）。
        void LateUpdate()
        {
            if (!session.InMatch && viewer.SessionId != null && Camera.main != null)
                Camera.main.transform.SetPositionAndRotation(new Vector3(0, 4.5f, 2f), Quaternion.identity);
        }

        /// 1 つずつ実行し、失敗したら 5 秒おいてやり直す。
        async Task Step(System.Func<Task> operation)
        {
            busy = true;
            try
            {
                await operation();
            }
            catch (KukuriException)
            {
                nextTry = Time.unscaledTime + 5;
            }
            finally
            {
                busy = false;
            }
        }

        static JToken Require(KukuriResponse response) =>
            response.Ok ? response.Data : throw new KukuriException(response.ErrorCode, response.ErrorMessage);

        async Task StartLive(KukuriClient client)
        {
            var id = (string)Require(await client.SendAsync("create_live_session", new JObject
            {
                ["topic"] = lobby.Topic,
                ["channel_ref"] = new JObject { ["kind"] = "public" },
                ["title"] = "Cyber Disc Arena の試合",
                ["description"] = "",
            }));
            var live = Require(await client.SendAsync("get_live_session", new JObject { ["topic"] = lobby.Topic, ["session_id"] = id }));
            broadcaster.Client = client;
            broadcaster.Topic = lobby.Topic;
            if (!await broadcaster.StartBroadcast(id))
            {
                await client.SendAsync("end_live_session", new JObject { ["topic"] = lobby.Topic, ["session_id"] = id });
                throw new KukuriException("broadcast_failed", "the broadcast did not start");
            }
            (LiveSession, LivePost) = (id, (string)live["start_post_id"]);
        }

        async Task StopLive(KukuriClient client)
        {
            var id = LiveSession;
            (LiveSession, LivePost) = (null, null);
            await broadcaster.StopBroadcast();
            await client.SendAsync("end_live_session", new JObject { ["topic"] = lobby.Topic, ["session_id"] = id });
        }

        /// ロビーにいる間、配信中の他の試合を 1 つ選んで見る（10 秒ごとに探す）。
        async Task WatchOther(KukuriClient client)
        {
            nextTry = Time.unscaledTime + 10;
            var lives = Require(await client.SendAsync("list_live_sessions", new JObject { ["topic"] = lobby.Topic })) as JArray;
            var other = lives?.FirstOrDefault(live => (string)live["status"] == "Live" && (string)live["host_pubkey"] != account.PublicKey);
            if (other == null || session.InMatch)
                return;
            viewer.Client = client;
            viewer.Topic = lobby.Topic;
            await viewer.StartWatching((string)other["session_id"]);
        }

        /// 応援する（R 長押しの menu）。自分の配信か、観戦中の配信の開始 post へリアクションを付ける。
        public async Task React(string emoji)
        {
            var target = LivePost ?? viewer.StartPostId;
            if (target == null || runner.Client == null)
                return;
            await runner.Client.SendAsync("toggle_reaction", new JObject
            {
                ["target_topic_id"] = lobby.Topic,
                ["target_object_id"] = target,
                ["reaction_key"] = new JObject { ["kind"] = "emoji", ["emoji"] = emoji },
                ["channel_ref"] = new JObject { ["kind"] = "public" },
            });
        }

        void OnEvent(JObject @event)
        {
            if ((string)@event["type"] == "reaction_received" && LivePost != null && @event["reaction"] is JObject reaction &&
                (string)reaction["target_object_id"] == LivePost)
                cheer.Spawn(Label(reaction));
        }

        static string Label(JObject reaction)
        {
            var emoji = (string)reaction["reaction"]?["emoji"];
            return Cheers.FirstOrDefault(cheer => cheer.emoji == emoji).label ?? "CHEER!";
        }

        void OnGUI()
        {
            if (account.Screen == "playing" && !session.InMatch)
                BroadcastNext = GUI.Toggle(new Rect(1000, 330, 260, 30), BroadcastNext, " 作る部屋の試合を配信する");
            if (session.InMatch && LiveSession != null)
                GUI.Label(new Rect(20, 88, 400, 30), "● 配信中（公開）");
            if (!session.InMatch && viewer.SessionId != null)
                GUI.Label(new Rect(1000, 360, 260, 30), "モニター: 配信中の試合を観戦中");
            // R を押している間、応援の menu を出す。
            var target = LivePost ?? viewer.StartPostId;
            if (target == null || !Input.GetKey(KeyCode.R) || ArenaControls.Typing())
                return;
            GUILayout.BeginArea(new Rect(Screen.width / 2 - 150, Screen.height - 140, 300, 60), GUI.skin.box);
            GUILayout.BeginHorizontal();
            foreach (var (emoji, label) in Cheers)
                if (GUILayout.Button(label, GUILayout.Height(40)))
                    _ = React(emoji);
            GUILayout.EndHorizontal();
            GUILayout.EndArea();
        }
    }
}
