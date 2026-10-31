using System;
using System.Collections;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Kukuri.GameSdk.Social;
using Newtonsoft.Json.Linq;
using NUnit.Framework;
using UnityEditor;
using UnityEngine;
using UnityEngine.TestTools;
using Object = UnityEngine.Object;

namespace Kukuri.GameSdk.Tests
{
    /// 掲示板の prefab だけを入れた project で、2 つの独立した profile が投稿・新着・thread・返信・続きの頁・購読の回収を
    /// 確かめる（#1981 AC-2）。アカウントは同意で作られたものをそのまま使う。test.ps1 が keyring を切り、loopback だけで動かす。
    public sealed class BoardTests
    {
        const string PrefabPath = "Packages/com.kukuri.gamesdk/Runtime/Social/KukuriBoard.prefab";

        KukuriClient a;
        KukuriClient b;
        GameObject boardA;
        GameObject boardB;

        [TearDown]
        public void TearDown()
        {
            a?.Dispose();
            b?.Dispose();
            if (boardA != null)
                Object.DestroyImmediate(boardA);
            if (boardB != null)
                Object.DestroyImmediate(boardB);
        }

        static string NewProfileDirectory() =>
            Path.Combine(Path.GetTempPath(), "kgs-" + Guid.NewGuid().ToString("N").Substring(0, 12));

        static string NewTopic() => "kukuri:topic:test-board-" + Guid.NewGuid().ToString("N").Substring(0, 8);

        IEnumerator Wait(Task task)
        {
            var clock = Stopwatch.StartNew();
            while (!task.IsCompleted)
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 60, "no response");
                a?.Pump();
                b?.Pump();
                yield return null;
            }
            if (task.IsFaulted)
                throw task.Exception.InnerException;
        }

        IEnumerator Call(KukuriClient client, string command, JObject payload, Action<JToken> result = null)
        {
            var request = client.SendAsync(command, payload);
            yield return Wait(request);
            Assert.IsTrue(request.Result.Ok, $"{command}: {request.Result.ErrorCode} {request.Result.ErrorMessage}");
            result?.Invoke(request.Result.Data);
        }

        IEnumerator Accept(KukuriClient client)
        {
            JToken consent = null;
            yield return Call(client, "get_app_consent_status", null, data => consent = data);
            var documents = new JArray(((JArray)consent["documents"]).Select(document =>
                new JObject { ["slug"] = document["slug"], ["version"] = document["currentVersion"] }));
            yield return Call(client, "accept_app_consents",
                new JObject { ["documents"] = documents, ["language"] = "ja", ["age_attested"] = true });
        }

        static KukuriBoard Board(KukuriClient client, string topic, out GameObject instance)
        {
            instance = Object.Instantiate(AssetDatabase.LoadAssetAtPath<GameObject>(PrefabPath));
            var board = instance.GetComponent<KukuriBoard>();
            board.Client = client;
            board.Topic = topic;
            return board;
        }

        static KukuriBoardItem[] Shown(GameObject board) => board.GetComponentsInChildren<KukuriBoardItem>(true)
            .Where(item => item.name.EndsWith("(Clone)") && item.gameObject.activeSelf)
            .ToArray();

        IEnumerator Until(Func<bool> condition, string what, Func<Task> retry = null)
        {
            var clock = Stopwatch.StartNew();
            while (!condition())
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 120, what);
                if (retry != null)
                    yield return Wait(retry());
                var pause = Stopwatch.StartNew();
                while (pause.Elapsed.TotalSeconds < 0.5)
                {
                    a?.Pump();
                    b?.Pump();
                    yield return null;
                }
            }
        }

        IEnumerator Subscribed(KukuriClient client, string topic, Action<bool> result)
        {
            yield return Call(client, "get_sync_status", null,
                data => result(((JArray)data["subscribed_topics"]).Any(name => (string)name == topic)));
        }

        [UnityTest]
        public IEnumerator PostsRepliesAndNewArrivalsAreSharedBetweenTwoProfiles()
        {
            var topic = NewTopic();
            a = KukuriClient.Open(KukuriMode.Ffi, "a", NewProfileDirectory());
            b = KukuriClient.Open(KukuriMode.Ffi, "b", NewProfileDirectory());
            yield return Accept(a);
            yield return Accept(b);
            string ticketA = null, ticketB = null;
            yield return Call(a, "get_local_peer_ticket", null, data => ticketA = (string)data);
            yield return Call(b, "get_local_peer_ticket", null, data => ticketB = (string)data);
            yield return Call(b, "import_peer_ticket", new JObject { ["ticket"] = ticketA });
            yield return Call(a, "import_peer_ticket", new JObject { ["ticket"] = ticketB });
            var mine = Board(a, topic, out boardA);
            var theirs = Board(b, topic, out boardB);
            yield return Wait(mine.RefreshAsync());
            yield return Wait(theirs.RefreshAsync());
            StringAssert.Contains("公開", boardA.GetComponentsInChildren<UnityEngine.UI.Text>()
                .Single(text => text.name == "Destination").text);

            var post = mine.PostAsync("test: from the board");
            yield return Wait(post);
            Assert.AreEqual(post.Result, mine.Posts.Single().ObjectId);

            // 相手の掲示板は新着の知らせで先頭の頁だけを読み直す。
            yield return Until(() => theirs.HasNew, "the other board did not hear of the post");
            yield return Wait(theirs.ReadNewAsync());
            yield return Until(() => theirs.Posts.Any(item => item.ObjectId == post.Result), "the post did not arrive",
                theirs.ReadNewAsync);
            Assert.AreEqual("test: from the board", Shown(boardB).Single().Content);

            // thread を開いて返信する。返信は同じ thread（root の ID）に付く。
            yield return Wait(theirs.OpenThreadAsync(post.Result));
            Assert.AreEqual(post.Result, theirs.ThreadId);
            var reply = theirs.PostAsync("test: reply on the board");
            yield return Wait(reply);
            CollectionAssert.AreEqual(new[] { post.Result, reply.Result }, theirs.Posts.Select(item => item.ObjectId));
            Assert.AreEqual(post.Result, theirs.Posts[1].ReplyTo);

            yield return Wait(mine.OpenThreadAsync(post.Result));
            yield return Until(() => mine.Posts.Any(item => item.ObjectId == reply.Result), "the reply did not arrive",
                mine.ReadNewAsync);
            var shown = mine.Posts.Single(item => item.ObjectId == reply.Result);
            Assert.AreEqual(post.Result, shown.ThreadId);
            Assert.AreEqual("test: reply on the board", shown.Content);
            Assert.IsNotEmpty(shown.Audience);
        }

        [UnityTest]
        public IEnumerator OlderPostsAreReadPageByPageAndTheSubscriptionIsReleasedOnClose()
        {
            var topic = NewTopic();
            a = KukuriClient.Open(KukuriMode.Ffi, "a", NewProfileDirectory());
            yield return Accept(a);
            var board = Board(a, topic, out boardA);
            yield return Wait(board.RefreshAsync());
            for (var index = 0; index < 25; index++)
                yield return Call(a, "create_post", new JObject
                {
                    ["topic"] = topic,
                    ["content"] = $"test: post {index}",
                    ["channel_ref"] = new JObject { ["kind"] = "public" },
                });

            // 1 回の表示は 20 件までで、続きは「さらに表示」で読む。
            yield return Wait(board.RefreshAsync());
            Assert.AreEqual(20, board.Posts.Count);
            Assert.IsTrue(board.HasMore);
            Assert.AreEqual("test: post 24", board.Posts[0].Content);
            yield return Wait(board.LoadMoreAsync());
            Assert.AreEqual(25, board.Posts.Count);
            Assert.AreEqual(25, Shown(boardA).Length);

            // 閉じると、掲示板が始めた購読を外す。
            var subscribed = false;
            yield return Subscribed(a, topic, value => subscribed = value);
            Assert.IsTrue(subscribed);
            yield return Wait(board.ReleaseAsync());
            Assert.IsFalse(board.Subscribed);
            yield return Subscribed(a, topic, value => subscribed = value);
            Assert.IsFalse(subscribed);

            // ゲームの他の部分が先に購読していた topic は、閉じても外さない。
            var shared = NewTopic();
            yield return Call(a, "set_topic_gossip_enabled", new JObject { ["topic"] = shared, ["enabled"] = true });
            board.Topic = shared;
            yield return Wait(board.RefreshAsync());
            yield return Wait(board.ReleaseAsync());
            yield return Subscribed(a, shared, value => subscribed = value);
            Assert.IsTrue(subscribed);
        }
    }
}
