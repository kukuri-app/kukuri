using System;
using System.Collections.Generic;
using System.Threading;
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
        /// 本文。成人向けの申告があり、表示設定で成人向けを出さないときは伏せた旨。
        public string Content { get; internal set; }
        /// 公開範囲の表示名（既存の投稿の `audience_label`）。
        public string Audience { get; internal set; }
        public long CreatedAt { get; internal set; }
    }

    /// 1 つの topic（`channelId` を指定すれば private channel）の投稿を新しい順に 20 件ずつ表示し、投稿・返信する uGUI の
    /// 掲示板（#1981）。行を押すとその thread を開き、thread では返信を書く。続きは「さらに表示」で読む。新着は
    /// `post_received` と、開いている間の一定の間隔の読み直し（遅れて届いた投稿には知らせが来ない）で知り、表示中の頁の
    /// 新しい側だけを読む。開いている間だけ topic を購読し、閉じる（無効にする・scene を変える・`ReleaseAsync`）と、自分で
    /// 始めた購読を外す。手元に持つ投稿は 100 件までで、超えたら足した側と反対の端を捨てる（古い側は「さらに表示」、
    /// 新しい側は「先頭へ戻る」で読み直す）。操作は 1 つずつ順に実行する。`runner` の client が開くまでは何もしない。
    public sealed class KukuriBoard : MonoBehaviour
    {
        const int PageSize = 20;
        const int MaxPosts = 100;
        const string AdultHidden = "成人向けの内容を含むため表示していません（設定で表示できます）";

        [SerializeField] KukuriRunner runner;
        [SerializeField] string topic = "kukuri:topic:game";
        [Tooltip("private channel の掲示板にするときの channel ID。空なら topic の公開の投稿。")]
        [SerializeField] string channelId = "";
        [Tooltip("開いている間、表示中の頁の新しい側を読み直す間隔（秒）。0 なら新着の知らせのときだけ読む。")]
        [SerializeField] float refreshSeconds = 10f;
        [SerializeField] Text destinationText;
        [SerializeField] InputField postInput;
        [SerializeField] Button postButton;
        [SerializeField] Button backButton;
        [SerializeField] Button moreButton;
        [SerializeField] RectTransform listContent;
        [SerializeField] KukuriBoardItem itemTemplate;
        [SerializeField] Text statusText;

        readonly List<BoardPost> posts = new List<BoardPost>();
        readonly HashSet<string> known = new HashSet<string>();
        readonly List<KukuriBoardItem> items = new List<KukuriBoardItem>();
        readonly SemaphoreSlim gate = new SemaphoreSlim(1, 1);
        KukuriClient client;
        KukuriClient subscribed;
        string subscribedTopic;
        bool ownsSubscription;
        bool showAdult;
        /// 閉じるたびに進める（閉じた後に返った購読の応答を取り消す）。
        int generation;
        bool closed;
        JToken nextCursor;
        /// 上限を超えて新しい側（一覧の先頭・thread の root の側）を捨てた。
        bool headCut;
        bool stale;
        float retryAt;
        float refreshAt;

        /// 閉じた後に返った応答。画面には出さない。
        sealed class ClosedException : Exception { }

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

        void OnEnable() => closed = false;

        void OnDisable() => _ = ReleaseAsync();

        void Update()
        {
            if (closed || Client == null || gate.CurrentCount == 0 || Time.unscaledTime < retryAt)
                return;
            if (subscribed == null)
                _ = Run(RefreshAsync);
            else if (stale || (refreshSeconds > 0 && Time.unscaledTime >= refreshAt))
                _ = Run(ReadNewAsync);
        }

        JObject Scope() => channelId.Length == 0
            ? new JObject { ["kind"] = "public" }
            : new JObject { ["kind"] = "channel", ["channel_id"] = channelId };

        /// 先頭の頁を読み直す（続きを読んでいた分は捨てる）。
        public Task RefreshAsync() => Serial(LoadHeadAsync);

        /// 前の頁の続きを読む。
        public Task LoadMoreAsync() => Serial(async () =>
        {
            if (nextCursor == null)
                return;
            var view = View();
            var page = await ListAsync(nextCursor);
            if (view == View())
                Append(page, true);
        });

        /// 投稿の thread を開く（root と返信を古い順に読む）。読めなければ元の表示のまま。
        public Task OpenThreadAsync(string threadId) => Switch(threadId);

        /// thread から一覧へ戻る。一覧で上限を超えて先頭を捨てていれば、先頭の頁を読み直す。
        public Task BackAsync() => Switch(null);

        /// 一覧では topic（または private channel）へ投稿し、thread では root へ返信する。投稿の ID を返す。
        public Task<string> PostAsync(string content) => Serial(async () =>
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
            // 投稿はできている。表示の読み直しの失敗は、次の読み直しに回す（送り直しを促さない）。
            try
            {
                await LoadNewAsync();
            }
            catch (KukuriException)
            {
                stale = true;
            }
            return objectId;
        });

        /// 表示中の頁の新しい側を読み、読んでいない投稿を足す（読んだ続きの頁は保つ）。一覧は先頭の頁、thread は末尾まで
        /// 読んでいれば最後の投稿の後ろを読む（続きを残していれば「さらに表示」に任せる）。
        public Task ReadNewAsync() => Serial(LoadNewAsync);

        /// 閉じる。自分で始めた購読を外し、新着を受けるのをやめる（`OnDisable` でも呼ぶ）。次に開く操作をするまで開き直さない。
        public async Task ReleaseAsync()
        {
            closed = true;
            generation++;
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

        Task Serial(Func<Task> body) => Serial(async () =>
        {
            await body();
            return true;
        });

        /// 操作を 1 つずつ順に実行する（開く操作は、閉じた後でも開き直す）。
        async Task<T> Serial<T>(Func<Task<T>> body)
        {
            closed = false;
            await gate.WaitAsync();
            try
            {
                return await body();
            }
            finally
            {
                gate.Release();
            }
        }

        Task Switch(string threadId) => Serial(async () =>
        {
            var previous = (ThreadId, posts: new List<BoardPost>(posts), nextCursor);
            ThreadId = threadId;
            try
            {
                await LoadHeadAsync();
            }
            catch
            {
                (ThreadId, nextCursor) = (previous.ThreadId, previous.nextCursor);
                Replace(previous.posts);
                throw;
            }
        });

        /// 読み始めた時の表示（topic・channel・thread）。読む間に変わったら結果を捨てる。
        (string, string, string) View() => (topic, channelId, ThreadId);

        async Task LoadHeadAsync()
        {
            stale = false;
            refreshAt = Time.unscaledTime + refreshSeconds;
            var view = View();
            JToken page;
            try
            {
                page = await ListAsync(null);
            }
            catch
            {
                stale = true;
                throw;
            }
            if (view != View())
            {
                stale = true;
                return;
            }
            nextCursor = null;
            Replace(new List<BoardPost>());
            Append(page, true);
        }

        async Task LoadNewAsync()
        {
            stale = false;
            refreshAt = Time.unscaledTime + refreshSeconds;
            // 一覧の先頭を捨てていれば先頭の頁は続かない。thread の続きが残っていれば「さらに表示」に任せる。
            if (ThreadId == null ? headCut : nextCursor != null)
                return;
            var view = View();
            JToken cursor = null;
            if (ThreadId != null && posts.Count > 0)
            {
                var last = posts[posts.Count - 1];
                cursor = new JObject { ["created_at"] = last.CreatedAt, ["object_id"] = last.ObjectId };
            }
            JToken page;
            try
            {
                page = await ListAsync(cursor);
            }
            catch
            {
                stale = true;
                throw;
            }
            if (view != View())
            {
                stale = true;
                return;
            }
            if (posts.Count == 0 || ThreadId != null)
            {
                Append(page, cursor == null);
                return;
            }
            // 一覧は新しい順に並ぶ。先頭の頁の読んでいない投稿を、先頭に足す（足した行だけを描く）。
            var fresh = new List<BoardPost>();
            foreach (var item in (JArray)page["items"])
            {
                var post = ToPost(item);
                if (known.Add(post.ObjectId))
                    fresh.Add(post);
            }
            for (var index = fresh.Count - 1; index >= 0; index--)
            {
                posts.Insert(0, fresh[index]);
                if (itemTemplate == null || listContent == null)
                    continue;
                var row = Instantiate(itemTemplate, listContent);
                row.transform.SetSiblingIndex(itemTemplate.transform.GetSiblingIndex() + 1);
                items.Insert(0, row);
                ShowRow(0);
            }
            if (posts.Count > MaxPosts)
                Trim(false);
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
            if (subscribed != null)
            {
                await ReleaseAsync();
                closed = false;
            }
            var opened = generation;
            var requested = topic;
            var status = Require(await current.SendAsync("get_sync_status"));
            var already = false;
            foreach (var name in status["subscribed_topics"] as JArray ?? new JArray())
                already |= (string)name == requested;
            if (!already)
                Require(await current.SendAsync("set_topic_gossip_enabled", new JObject { ["topic"] = requested, ["enabled"] = true }));
            showAdult = (bool?)Require(await current.SendAsync("get_content_display_settings"))["adult_content_enabled"] ?? false;
            // 待つ間に閉じたら、始めた購読を取り消して戻る。
            if (opened != generation)
            {
                if (!already)
                    await current.SendAsync("unsubscribe_topic", new JObject { ["topic"] = requested });
                throw new ClosedException();
            }
            ownsSubscription = !already;
            subscribedTopic = requested;
            current.EventReceived += OnEvent;
            subscribed = current;
        }

        void OnEvent(JObject @event)
        {
            var type = (string)@event["type"];
            // 知らせが溢れたときは、何の投稿か分からないので読み直す。
            if (type == "events_lost")
            {
                stale = true;
                return;
            }
            if (type != "post_received")
                return;
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
            nextCursor = null;
            ThreadId = null;
            Replace(new List<BoardPost>());
            stale = true;
        }

        /// 表示を `next` に置き換える（行は使い回す）。
        void Replace(List<BoardPost> next)
        {
            headCut = false;
            posts.Clear();
            known.Clear();
            foreach (var post in next)
            {
                posts.Add(post);
                known.Add(post.ObjectId);
            }
            Render();
        }

        /// 頁を後ろに足す（読んでいない投稿だけ）。`withCursor` なら続きの位置を頁のものにする。
        void Append(JToken page, bool withCursor)
        {
            var from = posts.Count;
            foreach (var item in (JArray)page["items"])
            {
                var post = ToPost(item);
                if (known.Add(post.ObjectId))
                    posts.Add(post);
            }
            if (withCursor)
                nextCursor = page["next_cursor"] is JObject cursor ? cursor : null;
            if (posts.Count > MaxPosts)
            {
                Trim(true);
                return;
            }
            RenderHeader();
            if (listContent == null || itemTemplate == null)
                return;
            for (var index = from; index < posts.Count; index++)
            {
                if (index == items.Count)
                    items.Add(Instantiate(itemTemplate, listContent));
                ShowRow(index);
            }
        }

        /// 上限を超えた分を、先頭（`fromHead`）か末尾から捨てる。末尾を捨てたら、続きの位置を残った最後の投稿にする。
        void Trim(bool fromHead)
        {
            var over = posts.Count - MaxPosts;
            var start = fromHead ? 0 : MaxPosts;
            for (var index = start; index < start + over; index++)
                known.Remove(posts[index].ObjectId);
            posts.RemoveRange(start, over);
            if (fromHead)
                headCut = true;
            else
            {
                var last = posts[posts.Count - 1];
                nextCursor = new JObject { ["created_at"] = last.CreatedAt, ["object_id"] = last.ObjectId };
            }
            Render();
        }

        BoardPost ToPost(JToken item)
        {
            var adult = false;
            foreach (var label in item["content_labels"] as JArray ?? new JArray())
                adult |= (string)label == "adult";
            return new BoardPost
            {
                ObjectId = (string)item["object_id"],
                ThreadId = (string)item["root_id"] ?? (string)item["object_id"],
                ReplyTo = (string)item["reply_to"],
                AuthorPubkey = (string)item["author_pubkey"] ?? "",
                AuthorName = (string)item["author_display_name"] ?? (string)item["author_name"],
                Content = adult && !showAdult ? AdultHidden : (string)item["content"],
                Audience = (string)item["audience_label"],
                CreatedAt = (long?)item["created_at"] ?? 0,
            };
        }

        static JToken Require(KukuriResponse response)
        {
            if (!response.Ok)
                throw new KukuriException(response.ErrorCode, response.ErrorMessage);
            return response.Data;
        }

        /// 画面の操作と読み直しを実行し、失敗は状態の欄に出す（5 秒は自動では繰り返さない）。
        async Task Run(Func<Task> operation)
        {
            if (Client == null)
                return;
            try
            {
                await operation();
                SetStatus("");
            }
            catch (ClosedException)
            {
            }
            catch (Exception error)
            {
                SetStatus(error is KukuriException failure ? $"失敗しました（{failure.Code}）" : "失敗しました");
                retryAt = Time.unscaledTime + 5f;
            }
        }

        void SetStatus(string text)
        {
            if (statusText != null)
                statusText.text = text;
        }

        void RenderHeader()
        {
            if (destinationText != null)
                destinationText.text = (ThreadId == null ? "投稿先: " : "返信先の thread: ") + topic +
                    (channelId.Length == 0 ? "（公開。誰でも読めます）" : "（private channel。参加者だけが読めます）");
            if (backButton != null)
            {
                // thread からは一覧へ、先頭を捨てた一覧からは先頭の頁へ戻る（どちらも先頭の頁を読み直す）。
                backButton.gameObject.SetActive(ThreadId != null || headCut);
                var label = backButton.GetComponentInChildren<Text>(true);
                if (label != null)
                    label.text = ThreadId != null ? "一覧へ戻る" : "先頭へ戻る";
            }
            if (moreButton != null)
                moreButton.interactable = nextCursor != null;
        }

        void ShowRow(int index)
        {
            items[index].gameObject.SetActive(true);
            items[index].Show(posts[index], ThreadId == null, threadId => _ = Run(() => OpenThreadAsync(threadId)));
        }

        /// 全部の行を描き直す（表示を置き換えたときだけ）。
        void Render()
        {
            RenderHeader();
            if (listContent == null || itemTemplate == null)
                return;
            while (items.Count < posts.Count)
                items.Add(Instantiate(itemTemplate, listContent));
            for (var index = 0; index < items.Count; index++)
            {
                if (index < posts.Count)
                    ShowRow(index);
                else
                    items[index].gameObject.SetActive(false);
            }
        }
    }
}
