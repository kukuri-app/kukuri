using System;
using System.Collections.Generic;
using System.Text;
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
        /// この profile が参加中。
        public bool IsJoined { get; internal set; }
    }

    /// 1 つのゲーム（application ID）の room を topic（`channelId` を指定すれば private channel）から一覧し、作成・参加・
    /// 退出・終了する uGUI のロビー。一覧は更新の新しい順に 20 件ずつで、続きは「さらに表示」で読む（#1978）。
    /// `runner` の client が開くまでは何もしない。
    public sealed class KukuriLobby : MonoBehaviour
    {
        [SerializeField] KukuriRunner runner;
        [SerializeField] string applicationId = "com.example.game";
        [SerializeField] string topic = "kukuri:topic:game";
        [Tooltip("private channel の room を扱うときの channel ID。空なら topic の公開の room。")]
        [SerializeField] string channelId = "";
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
        [Tooltip("招待の書き出し（private channel の所有者）と読み込み（招待された人）に使う欄。")]
        [SerializeField] InputField inviteInput;
        [SerializeField] Button exportInviteButton;
        [SerializeField] Button importInviteButton;

        readonly List<LobbyRoom> rooms = new List<LobbyRoom>();
        readonly List<KukuriLobbyItem> items = new List<KukuriLobbyItem>();
        KukuriClient client;
        KukuriClient subscribed;
        string ownPubkey;
        string syncedTopic;
        string nextCursor;
        int pages;
        bool busy;
        float nextRefreshAt;

        public IReadOnlyList<LobbyRoom> Rooms => rooms;
        public bool HasMore => nextCursor != null;
        /// 参加中の session と room（参加していなければ null）。
        public string JoinedSessionId { get; private set; }
        public string JoinedRoomId { get; private set; }

        /// session が閉じたときの理由の code（`left`・`room_ended`・`access_revoked`・`host_unavailable`）。
        public event Action<string> SessionClosed;

        /// 変えると、読んだ一覧を捨てる（次の読み込みは先頭の頁から）。
        public string ApplicationId
        {
            get => applicationId;
            set
            {
                applicationId = value;
                Clear();
            }
        }

        /// 変えると、読んだ一覧を捨て、次の読み込みの前に新しい topic の同期を有効にする。
        public string Topic
        {
            get => topic;
            set
            {
                topic = value;
                Clear();
            }
        }

        /// 変えると、読んだ一覧を捨てる。空なら topic の公開の room。
        public string ChannelId
        {
            get => channelId;
            set
            {
                channelId = value ?? "";
                Clear();
            }
        }

        /// runner を使わずに client を渡す（試験と、client を自分で開くゲーム用）。
        public KukuriClient Client
        {
            get => client ?? (runner != null ? runner.Client : null);
            set => client = value;
        }

        void Awake()
        {
            Listen(createButton, () => CreateRoomAsync(titleInput != null ? titleInput.text : ""));
            Listen(refreshButton, RefreshAsync);
            Listen(moreButton, LoadMoreAsync);
            Listen(exportInviteButton, async () =>
            {
                var invite = await ExportInviteAsync();
                if (inviteInput != null)
                    inviteInput.text = invite;
            });
            Listen(importInviteButton, () => ImportInviteAsync(inviteInput != null ? inviteInput.text : ""));
            if (itemTemplate != null)
                itemTemplate.gameObject.SetActive(false);
            Render();
        }

        void Listen(Button button, Func<Task> operation)
        {
            if (button != null)
                button.onClick.AddListener(() => _ = Run(operation));
        }

        void Update()
        {
            if (refreshSeconds <= 0 || pages > 1 || Client == null || busy || Time.unscaledTime < nextRefreshAt)
                return;
            nextRefreshAt = Time.unscaledTime + refreshSeconds;
            _ = Run(RefreshAsync);
        }

        void OnDestroy()
        {
            if (subscribed != null)
                subscribed.EventReceived -= OnEvent;
        }

        JObject Scope() => channelId.Length == 0
            ? new JObject { ["kind"] = "public" }
            : new JObject { ["kind"] = "channel", ["channel_id"] = channelId };

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
                ["channel_ref"] = channelId.Length == 0
                    ? new JObject { ["kind"] = "public" }
                    : new JObject { ["kind"] = "private_channel", ["channel_id"] = channelId },
                ["title"] = title,
                ["description"] = "",
                ["participants"] = new JArray(),
                ["action"] = new JObject { ["application_id"] = applicationId, ["max_players"] = maxPlayers },
            };
            var roomId = (string)Require(await Client.SendAsync("create_game_room", payload));
            await RefreshAsync();
            return roomId;
        }

        /// 自分の room を終える。一覧には「終了」と表示されて残り、参加中の人の session は閉じる。
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

        /// room の host へ参加する。断られたら理由の code の `KukuriException`。
        public async Task<string> JoinRoomAsync(string roomId)
        {
            await EnsureTopicAsync();
            var payload = new JObject { ["topic"] = topic, ["scope"] = Scope(), ["room_id"] = roomId };
            var sessionId = (string)Require(await Client.SendAsync("join_game_session", payload))["session_id"];
            JoinedSessionId = sessionId;
            JoinedRoomId = roomId;
            Render();
            return sessionId;
        }

        /// 参加中の session から退出する（閉じた知らせは `SessionClosed` に `left` で届く）。
        public async Task LeaveRoomAsync()
        {
            if (JoinedSessionId == null)
                return;
            Require(await Client.SendAsync("leave_game_session", new JObject { ["session_id"] = JoinedSessionId }));
        }

        /// 今の private channel の招待を書き出す（所有者）。招待は秘密として受け取り、保存しない。
        public async Task<string> ExportInviteAsync()
        {
            var response = await Client.SendAsync("export_private_channel_invite",
                new JObject { ["topic"] = topic, ["channel_id"] = channelId }, acceptsSecretOutput: true);
            Require(response);
            return response.Secret != null ? Encoding.UTF8.GetString(response.Secret) : "";
        }

        /// 招待を読み込み、その private channel の room に切り替える。
        public async Task ImportInviteAsync(string invite)
        {
            var preview = Require(await Client.SendAsync("import_private_channel_invite", new JObject(),
                Encoding.UTF8.GetBytes(invite.Trim())));
            Topic = (string)preview["topic_id"];
            ChannelId = (string)preview["channel_id"];
            await RefreshAsync();
        }

        async Task<JToken> ListAsync(string cursor)
        {
            await EnsureTopicAsync();
            var payload = new JObject { ["topic"] = topic, ["scope"] = Scope(), ["application_id"] = applicationId };
            if (cursor != null)
                payload["cursor"] = cursor;
            return Require(await Client.SendAsync("list_action_rooms", payload));
        }

        /// topic の同期を有効にし、自分の公開鍵を一度だけ読み、session の閉じた知らせを受ける。
        async Task EnsureTopicAsync()
        {
            var client = Client ?? throw new KukuriException("not_ready", "the kukuri client is not open");
            if (subscribed != client)
            {
                if (subscribed != null)
                    subscribed.EventReceived -= OnEvent;
                client.EventReceived += OnEvent;
                subscribed = client;
            }
            if (syncedTopic != topic)
            {
                var requested = topic;
                Require(await client.SendAsync("set_topic_gossip_enabled", new JObject { ["topic"] = requested, ["enabled"] = true }));
                syncedTopic = requested;
            }
            ownPubkey ??= (string)Require(await client.SendAsync("get_sync_status"))["local_author_pubkey"];
        }

        void OnEvent(JObject @event)
        {
            if ((string)@event["type"] != "game_session_closed" || (string)@event["session_id"] != JoinedSessionId)
                return;
            var reason = (string)@event["reason"];
            JoinedSessionId = null;
            JoinedRoomId = null;
            SetStatus(ReasonLabel(reason));
            Render();
            SessionClosed?.Invoke(reason);
        }

        void Clear()
        {
            rooms.Clear();
            nextCursor = null;
            pages = 0;
            Render();
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

        /// 失敗の応答を、参加を断った理由の code（あれば）つきの `KukuriException` にする。
        static JToken Require(KukuriResponse response)
        {
            if (!response.Ok)
                throw new KukuriException((string)response.Raw?["error"]?["details"]?["reason"] ?? response.ErrorCode,
                    response.ErrorMessage);
            return response.Data;
        }

        static string ReasonLabel(string code) => code switch
        {
            "left" => "退出しました",
            "room_ended" => "部屋が終了しました",
            "access_revoked" => "参加の資格がなくなりました",
            "host_unavailable" => "部屋を作った人の端末に接続できなくなりました",
            "access_denied" => "この部屋には参加できません",
            "room_full" => "満員です",
            "room_not_found" => "部屋が見つかりません",
            "host_unreachable" => "部屋を作った人の端末に接続できません",
            "session_limit" => "これ以上の部屋には同時に参加できません",
            _ => $"失敗しました（{code}）",
        };

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
                SetStatus(ReasonLabel(error.Code));
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
            if (exportInviteButton != null)
                exportInviteButton.interactable = channelId.Length > 0;
            foreach (var room in rooms)
                room.IsJoined = room.RoomId == JoinedRoomId;
            if (listContent == null || itemTemplate == null)
                return;
            while (items.Count < rooms.Count)
                items.Add(Instantiate(itemTemplate, listContent));
            for (var index = 0; index < items.Count; index++)
            {
                var visible = index < rooms.Count;
                items[index].gameObject.SetActive(visible);
                if (visible)
                    items[index].Show(rooms[index],
                        roomId => _ = Run(() => JoinRoomAsync(roomId)),
                        () => _ = Run(LeaveRoomAsync),
                        roomId => _ = Run(() => EndRoomAsync(roomId)));
            }
        }
    }
}
