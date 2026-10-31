using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using Newtonsoft.Json.Linq;
using UnityEngine;
using UnityEngine.UI;

namespace Kukuri.GameSdk.Social
{
    /// 掲示板の 1 件の投稿（既存の投稿・返信。ADR 0070 §5.4）。
    public sealed class BoardPost
    {
        public string ObjectId { get; internal set; }
        /// 返信の先の thread の root（root の投稿なら自分の ID）。
        public string ThreadId { get; internal set; }
        public string ReplyTo { get; internal set; }
        public string AuthorPubkey { get; internal set; }
        public string AuthorName { get; internal set; }
        public string Content { get; internal set; }
        /// 公開範囲の表示名（既存の投稿の `audience_label`）。
        public string Audience { get; internal set; }
        public long CreatedAt { get; internal set; }
    }

    /// 1 つの topic（`channelId` を指定すれば private channel）の投稿を新しい順に 20 件ずつ表示し、投稿・返信する uGUI の
    /// 掲示板（#1981）。行を押すとその thread を開き、thread では返信を書く。続きは「さらに表示」で読む。新着は
    /// `post_received` で知り、表示中の先頭の頁だけを読み直す。開いている間だけ topic を購読し、閉じる（無効にする・scene を
    /// 変える）と、自分で始めた購読を外す。`runner` の client が開くまでは何もしない。
    public sealed class KukuriBoard : MonoBehaviour
    {
        const int PageSize = 20;

        [SerializeField] KukuriRunner runner;
        [SerializeField] string topic = "kukuri:topic:game";
        [Tooltip("private channel の掲示板にするときの channel ID。空なら topic の公開の投稿。")]
        [SerializeField] string channelId = "";
        [SerializeField] Text destinationText;
        [SerializeField] InputField postInput;
        [SerializeField] Button postButton;
        [SerializeField] Button backButton;
        [SerializeField] Button moreButton;
        [SerializeField] RectTransform listContent;
        [SerializeField] KukuriBoardItem itemTemplate;
        [SerializeField] Text statusText;

        readonly List<BoardPost> posts = new List<BoardPost>();
        readonly List<KukuriBoardItem> items = new List<KukuriBoardItem>();
        KukuriClient client;
        KukuriClient subscribed;
        string subscribedTopic;
        bool ownsSubscription;
        /// 続きの位置（timeline・thread の `next_cursor`。形は command が決める）。
        JToken nextCursor;
        bool busy;
        bool stale;
        float retryAt;

        public IReadOnlyList<BoardPost> Posts => posts;
        public bool HasMore => nextCursor != null;
        /// 開いている thread の root の ID（一覧なら null）。
        public string ThreadId { get; private set; }
        /// 表示中の一覧・thread に新着の知らせが届いて、まだ読んでいない。
        public bool HasNew => stale;
        /// 開いている間、topic を購読しているか。
        public bool Subscribed => subscribed != null;

        /// 変えると、読んだ投稿を捨てて一覧へ戻る。
        public string Topic
        {
            get => topic;
            set
            {
                topic = value;
                Clear();
            }
        }

        /// 変えると、読んだ投稿を捨てて一覧へ戻る。空なら topic の公開の投稿。
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
            Listen(postButton, () => PostAsync(postInput != null ? postInput.text : ""));
            Listen(backButton, BackAsync);
            Listen(moreButton, LoadMoreAsync);
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
            if (Client == null || busy || Time.unscaledTime < retryAt)
                return;
            if (subscribed == null || stale)
                _ = Run(posts.Count == 0 ? RefreshAsync : ReadNewAsync);
        }

        void OnDisable() => _ = ReleaseAsync();

        JObject Scope() => channelId.Length == 0
            ? new JObject { ["kind"] = "public" }
            : new JObject { ["kind"] = "channel", ["channel_id"] = channelId };

        /// 先頭の頁を読み直す（続きを読んでいた分は捨てる）。
        public async Task RefreshAsync()
        {
            var page = await ListAsync(null);
            stale = false;
            posts.Clear();
            Append(page);
        }

        /// 前の頁の続きを読む。
        public async Task LoadMoreAsync()
        {
            if (nextCursor != null)
                Append(await ListAsync(nextCursor));
        }

        /// 投稿の thread を開く（root と返信を古い順に読む）。
        public Task OpenThreadAsync(string threadId)
        {
            ThreadId = threadId;
            return RefreshAsync();
        }

        /// thread から一覧へ戻る。
        public Task BackAsync()
        {
            ThreadId = null;
            return RefreshAsync();
        }

        /// 一覧では topic（または private channel）へ投稿し、thread では root へ返信する。投稿の ID を返す。
        public async Task<string> PostAsync(string content)
        {
            if (content.Trim().Length == 0)
                return null;
            await EnsureSubscribedAsync();
            var payload = new JObject
            {
                ["topic"] = topic,
                ["content"] = content,
                ["channel_ref"] = channelId.Length == 0
                    ? new JObject { ["kind"] = "public" }
                    : new JObject { ["kind"] = "private_channel", ["channel_id"] = channelId },
            };
            if (ThreadId != null)
                payload["reply_to"] = ThreadId;
            var objectId = (string)Require(await Client.SendAsync("create_post", payload));
            if (postInput != null)
                postInput.text = "";
            await ReadNewAsync();
            return objectId;
        }

        /// 閉じる。自分で始めた購読を外し、新着を受けるのをやめる（`OnDisable` でも呼ぶ）。
        public async Task ReleaseAsync()
        {
            var releasing = subscribed;
            if (releasing == null)
                return;
            releasing.EventReceived -= OnEvent;
            subscribed = null;
            if (ownsSubscription)
            {
                ownsSubscription = false;
                await releasing.SendAsync("unsubscribe_topic", new JObject { ["topic"] = subscribedTopic });
            }
        }

        /// 新着の分だけ先頭の頁を読み直し、読んでいない投稿を足す（読んだ続きの頁は保つ）。`Update` が新着の知らせで呼ぶ。
        public async Task ReadNewAsync()
        {
            stale = false;
            var page = await ListAsync(null);
            if (posts.Count == 0)
            {
                Append(page);
                return;
            }
            var known = new HashSet<string>();
            foreach (var post in posts)
                known.Add(post.ObjectId);
            var fresh = new List<BoardPost>();
            foreach (var item in (JArray)page["items"])
                if (!known.Contains((string)item["object_id"]))
                    fresh.Add(ToPost(item));
            // 一覧は新しい順、thread は古い順に並ぶ。
            if (ThreadId == null)
                posts.InsertRange(0, fresh);
            else
                posts.AddRange(fresh);
            Render();
        }

        async Task<JToken> ListAsync(JToken cursor)
        {
            await EnsureSubscribedAsync();
            var payload = ThreadId == null
                ? new JObject { ["topic"] = topic, ["scope"] = Scope(), ["limit"] = PageSize }
                : new JObject { ["topic"] = topic, ["thread_id"] = ThreadId, ["limit"] = PageSize };
            if (cursor != null)
                payload["cursor"] = cursor;
            return Require(await Client.SendAsync(ThreadId == null ? "list_timeline" : "list_thread", payload));
        }

        /// 開いている間、topic を購読し、新着を受ける。すでに購読している topic（ゲームの他の部分が使っている）は閉じても外さない。
        async Task EnsureSubscribedAsync()
        {
            var current = Client ?? throw new KukuriException("not_ready", "the kukuri client is not open");
            if (subscribed == current && subscribedTopic == topic)
                return;
            await ReleaseAsync();
            var status = Require(await current.SendAsync("get_sync_status"));
            var already = false;
            foreach (var subscribedName in status["subscribed_topics"] as JArray ?? new JArray())
                already |= (string)subscribedName == topic;
            if (!already)
                Require(await current.SendAsync("set_topic_gossip_enabled", new JObject { ["topic"] = topic, ["enabled"] = true }));
            ownsSubscription = !already;
            subscribedTopic = topic;
            current.EventReceived += OnEvent;
            subscribed = current;
        }

        void OnEvent(JObject @event)
        {
            if ((string)@event["type"] != "post_received")
                return;
            // 溢れたときは何の投稿か分からないので読み直す。
            var post = @event["post"] as JObject;
            if (post == null)
            {
                stale = true;
                return;
            }
            var channel = (string)post["channel_id"] ?? "";
            var thread = (string)post["root_id"] ?? (string)post["object_id"];
            if ((string)post["topic_id"] == topic && channel == channelId && (ThreadId == null || thread == ThreadId))
                stale = true;
        }

        void Clear()
        {
            posts.Clear();
            nextCursor = null;
            ThreadId = null;
            stale = true;
            Render();
        }

        void Append(JToken page)
        {
            foreach (var item in (JArray)page["items"])
                posts.Add(ToPost(item));
            nextCursor = page["next_cursor"] is JObject cursor ? cursor : null;
            Render();
        }

        static BoardPost ToPost(JToken item) => new BoardPost
        {
            ObjectId = (string)item["object_id"],
            ThreadId = (string)item["root_id"] ?? (string)item["object_id"],
            ReplyTo = (string)item["reply_to"],
            AuthorPubkey = (string)item["author_pubkey"],
            AuthorName = (string)item["author_display_name"] ?? (string)item["author_name"],
            Content = (string)item["content"],
            Audience = (string)item["audience_label"],
            CreatedAt = (long?)item["created_at"] ?? 0,
        };

        static JToken Require(KukuriResponse response)
        {
            if (!response.Ok)
                throw new KukuriException(response.ErrorCode, response.ErrorMessage);
            return response.Data;
        }

        /// 画面の操作と新着の読み直しを 1 つずつ実行し、失敗は状態の欄に出す。
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
                // 開く・読み直すの失敗を毎 frame 繰り返さない。
                retryAt = Time.unscaledTime + 5f;
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
            if (destinationText != null)
                destinationText.text = (ThreadId == null ? "投稿先: " : "返信先の thread: ") + topic +
                    (channelId.Length == 0 ? "（公開。誰でも読めます）" : "（private channel。参加者だけが読めます）");
            if (backButton != null)
                backButton.gameObject.SetActive(ThreadId != null);
            if (moreButton != null)
                moreButton.interactable = nextCursor != null;
            if (listContent == null || itemTemplate == null)
                return;
            while (items.Count < posts.Count)
                items.Add(Instantiate(itemTemplate, listContent));
            for (var index = 0; index < items.Count; index++)
            {
                var visible = index < posts.Count;
                items[index].gameObject.SetActive(visible);
                if (visible)
                    items[index].Show(posts[index], ThreadId == null,
                        threadId => _ = Run(() => OpenThreadAsync(threadId)));
            }
        }
    }
}
