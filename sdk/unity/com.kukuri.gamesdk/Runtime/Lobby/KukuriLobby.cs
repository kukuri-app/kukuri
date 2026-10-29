using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using Newtonsoft.Json.Linq;
using UnityEngine;
using UnityEngine.UI;

namespace Kukuri.GameSdk.Lobby
{
    /// 一覧に出る 1 つの room（action session。ADR 0070 §5.2）。
    public sealed class LobbyRoom
    {
        public string RoomId { get; internal set; }
        public string Title { get; internal set; }
        /// `Waiting`・`Running`・`Paused`・`Ended`。
        public string Status { get; internal set; }
        public string HostPubkey { get; internal set; }
        public string HostEndpointId { get; internal set; }
        public int MaxPlayers { get; internal set; }
        /// この profile のアカウントが作った room。
        public bool IsMine { get; internal set; }
    }

    /// 1 つのゲーム（application ID）の room を topic から一覧し、作成・終了する uGUI のロビー。一覧は更新の新しい順に
    /// 20 件ずつで、続きは「さらに表示」で読む（#1978）。`runner` の client が開くまでは何もしない。
    public sealed class KukuriLobby : MonoBehaviour
    {
        [SerializeField] KukuriRunner runner;
        [SerializeField] string applicationId = "com.example.game";
        [SerializeField] string topic = "kukuri:topic:game";
        [SerializeField] int maxPlayers = 4;
        [Tooltip("一覧の先頭の頁を読み直す間隔（秒）。0 なら自動では読み直さない。続きを読んだ間は読み直さない。")]
        [SerializeField] float refreshSeconds = 5f;
        [SerializeField] InputField titleInput;
        [SerializeField] Button createButton;
        [SerializeField] Button refreshButton;
        [SerializeField] Button moreButton;
        [SerializeField] RectTransform listContent;
        [SerializeField] KukuriLobbyItem itemTemplate;
        [SerializeField] Text statusText;

        readonly List<LobbyRoom> rooms = new List<LobbyRoom>();
        readonly List<KukuriLobbyItem> items = new List<KukuriLobbyItem>();
        KukuriClient client;
        string ownPubkey;
        string nextCursor;
        int pages;
        bool busy;
        float nextRefreshAt;

        public IReadOnlyList<LobbyRoom> Rooms => rooms;
        public bool HasMore => nextCursor != null;

        public string ApplicationId
        {
            get => applicationId;
            set => applicationId = value;
        }

        public string Topic
        {
            get => topic;
            set => topic = value;
        }

        /// runner を使わずに client を渡す（試験と、client を自分で開くゲーム用）。
        public KukuriClient Client
        {
            get => client ?? (runner != null ? runner.Client : null);
            set => client = value;
        }

        void Awake()
        {
            if (createButton != null)
                createButton.onClick.AddListener(() => _ = Run(() => CreateRoomAsync(titleInput != null ? titleInput.text : "")));
            if (refreshButton != null)
                refreshButton.onClick.AddListener(() => _ = Run(RefreshAsync));
            if (moreButton != null)
                moreButton.onClick.AddListener(() => _ = Run(LoadMoreAsync));
            if (itemTemplate != null)
                itemTemplate.gameObject.SetActive(false);
            Render();
        }

        void Update()
        {
            if (refreshSeconds <= 0 || pages > 1 || Client == null || busy || Time.unscaledTime < nextRefreshAt)
                return;
            nextRefreshAt = Time.unscaledTime + refreshSeconds;
            _ = Run(RefreshAsync);
        }

        /// 先頭の頁を読み直す（続きを読んでいた分は捨てる）。
        public async Task RefreshAsync()
        {
            var page = await ListAsync(null);
            rooms.Clear();
            pages = 0;
            Append(page);
        }

        /// 前の頁の続きを読む。
        public async Task LoadMoreAsync()
        {
            if (nextCursor == null)
                return;
            Append(await ListAsync(nextCursor));
        }

        /// room を作る。作った room の ID を返す。
        public async Task<string> CreateRoomAsync(string title)
        {
            await EnsureTopicAsync();
            var payload = new JObject
            {
                ["topic"] = topic,
                ["title"] = title,
                ["description"] = "",
                ["participants"] = new JArray(),
                ["action"] = new JObject { ["application_id"] = applicationId, ["max_players"] = maxPlayers },
            };
            var roomId = (string)Require(await Client.SendAsync("create_game_room", payload));
            await RefreshAsync();
            return roomId;
        }

        /// 自分の room を終える。一覧には「終了」と表示されて残る。
        public async Task EndRoomAsync(string roomId)
        {
            var payload = new JObject
            {
                ["topic"] = topic,
                ["room_id"] = roomId,
                ["status"] = "Ended",
                ["scores"] = new JArray(),
            };
            Require(await Client.SendAsync("update_game_room", payload));
            await RefreshAsync();
        }

        async Task<JToken> ListAsync(string cursor)
        {
            await EnsureTopicAsync();
            var payload = new JObject { ["topic"] = topic, ["application_id"] = applicationId };
            if (cursor != null)
                payload["cursor"] = cursor;
            return Require(await Client.SendAsync("list_action_rooms", payload));
        }

        /// topic の同期を有効にし、自分の公開鍵を一度だけ読む。
        async Task EnsureTopicAsync()
        {
            if (ownPubkey != null)
                return;
            var client = Client ?? throw new KukuriException("not_ready", "the kukuri client is not open");
            Require(await client.SendAsync("set_topic_gossip_enabled", new JObject { ["topic"] = topic, ["enabled"] = true }));
            ownPubkey = (string)Require(await client.SendAsync("get_sync_status"))["local_author_pubkey"];
        }

        void Append(JToken page)
        {
            foreach (var item in (JArray)page["items"])
            {
                var action = item["action"];
                rooms.Add(new LobbyRoom
                {
                    RoomId = (string)item["room_id"],
                    Title = (string)item["title"],
                    Status = (string)item["status"],
                    HostPubkey = (string)item["host_pubkey"],
                    HostEndpointId = (string)action?["host_endpoint_id"],
                    MaxPlayers = (int?)action?["max_players"] ?? 0,
                    IsMine = (string)item["host_pubkey"] == ownPubkey,
                });
            }
            nextCursor = (string)page["next_cursor"];
            pages++;
            Render();
        }

        static JToken Require(KukuriResponse response)
        {
            if (!response.Ok)
                throw new KukuriException(response.ErrorCode, response.ErrorMessage);
            return response.Data;
        }

        /// 画面の操作から 1 つずつ実行し、失敗は状態の欄に出す。
        async Task Run(Func<Task> operation)
        {
            if (busy || Client == null)
                return;
            busy = true;
            try
            {
                await operation();
                SetStatus("");
            }
            catch (KukuriException error)
            {
                SetStatus($"失敗しました（{error.Code}）");
            }
            finally
            {
                busy = false;
            }
        }

        void SetStatus(string text)
        {
            if (statusText != null)
                statusText.text = text;
        }

        void Render()
        {
            if (moreButton != null)
                moreButton.interactable = nextCursor != null;
            if (listContent == null || itemTemplate == null)
                return;
            while (items.Count < rooms.Count)
                items.Add(Instantiate(itemTemplate, listContent));
            for (var index = 0; index < items.Count; index++)
            {
                var visible = index < rooms.Count;
                items[index].gameObject.SetActive(visible);
                if (visible)
                    items[index].Show(rooms[index], roomId => _ = Run(() => EndRoomAsync(roomId)));
            }
        }
    }
}
