using System.Collections.Generic;
using System.Linq;
using Kukuri.GameSdk;
using Kukuri.GameSdk.Assets;
using Newtonsoft.Json.Linq;
using UnityEngine;

namespace CyberDiscArena
{
    /// スキンのロッカー。作者が発行するスキン（PNG。ID はここに登録したもの）の中から、発行済みのものを選べる。選んだ
    /// スキンは game player ごとに保存し（`ArenaProfile`）、自分の体に貼る。対戦では相手が選んだスキンを相手の体に貼る
    /// （スキンの ID は対戦の開始で互いに送り、本体は SDK が P2P で取る）。作者が新しいスキンや新しい版を発行すると、ロッカー
    /// と着ている体は再起動せずに変わる（ADR 0070 §5.3・§7）。
    public sealed class ArenaSkins : MonoBehaviour
    {
        [SerializeField] KukuriRunner runner;
        [SerializeField] KukuriAccountScreen account;
        [SerializeField] ArenaProfile profile;
        [SerializeField] ArenaSession session;
        /// 2 人の体に貼る loader（player 0・1 の順）。
        [SerializeField] KukuriSkinLoader[] loaders = new KukuriSkinLoader[2];
        [SerializeField] string publisherPubkey = "";
        [SerializeField] string applicationId = "com.kukuri.cyber-disc-arena";
        [SerializeField] string topic = "kukuri:topic:cyber-disc-arena";
        /// 発行するスキンの ID（作者が Editor の発行で登録する。32 件まで）。
        [SerializeField] string[] skins = { "neon-red", "gold", "holo" };
        [SerializeField] Rect area = new Rect(1000, 16, 260, 300);

        readonly HashSet<string> published = new HashSet<string>();
        KukuriClient subscribed;
        bool listing;
        bool stale = true;
        float listedAt;

        /// player の体に貼る loader（試験・演出用）。
        public KukuriSkinLoader Loader(int player) => loaders[player];

        /// 発行済みで選べるスキン。
        public IReadOnlyCollection<string> Published => published;
        public string PublisherPubkey { get => publisherPubkey; set => publisherPubkey = value ?? ""; }
        public string Topic { get => topic; set => topic = value ?? ""; }

        /// スキンを選ぶ（`ArenaProfile.StandardSkin` か発行済みのもの）。
        public void Select(string skin)
        {
            if (skin == ArenaProfile.StandardSkin || published.Contains(skin))
                profile.Skin = skin;
        }

        void Awake() => session.LocalSkin = () => profile.Skin;

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
            var local = session.InMatch ? session.Local : 0;
            for (var index = 0; index < loaders.Length; index++)
            {
                var loader = loaders[index];
                loader.Topic = topic;
                loader.PublisherPubkey = publisherPubkey;
                loader.ApplicationId = applicationId;
                var skin = index == local ? profile.Skin : session.InMatch ? session.RemoteSkin : ArenaProfile.StandardSkin;
                loader.AssetId = skin == ArenaProfile.StandardSkin ? "" : skin;
            }
            // 新しく発行されたスキンの知らせが来ないこともあるので、ロビーにいる間は 10 秒ごとにも読み直す。
            if (!session.InMatch && Time.unscaledTime - listedAt > 10)
                stale = true;
            if (stale && !listing && client != null && account.Screen == "playing" && publisherPubkey.Length > 0)
                _ = List(client);
        }

        void OnEvent(JObject @event)
        {
            if ((string)@event["type"] == "game_asset_updated" && (string)@event["publisher_pubkey"] == publisherPubkey &&
                (string)@event["application_id"] == applicationId)
                stale = true;
        }

        /// 登録した ID のうち発行済みのもの（1 回の読込みは登録した件数まで）。
        async System.Threading.Tasks.Task List(KukuriClient client)
        {
            listing = true;
            stale = false;
            listedAt = Time.unscaledTime;
            var response = await client.SendAsync("list_game_assets", new JObject
            {
                ["topic"] = topic,
                ["publisher_pubkey"] = publisherPubkey,
                ["application_id"] = applicationId,
                ["asset_ids"] = new JArray(skins.Take(32)),
            });
            listing = false;
            if (!response.Ok)
                return;
            published.Clear();
            foreach (var release in response.Data as JArray ?? new JArray())
                published.Add((string)release["asset_id"]);
        }

        void OnGUI()
        {
            if (account.Screen != "playing" || session.InMatch)
                return;
            GUILayout.BeginArea(area, GUI.skin.box);
            GUILayout.Label("ロッカー（スキン）");
            foreach (var skin in new[] { ArenaProfile.StandardSkin }.Concat(skins))
            {
                var available = skin == ArenaProfile.StandardSkin || published.Contains(skin);
                var label = skin == profile.Skin ? $"● {skin}（着ている）" : available ? skin : $"{skin}（未公開）";
                GUI.enabled = available && skin != profile.Skin;
                if (GUILayout.Button(label))
                    Select(skin);
                GUI.enabled = true;
            }
            GUILayout.EndArea();
        }
    }
}
