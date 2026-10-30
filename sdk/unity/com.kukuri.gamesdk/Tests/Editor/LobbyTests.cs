using System;
using System.Collections;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Kukuri.GameSdk.Lobby;
using Newtonsoft.Json.Linq;
using NUnit.Framework;
using UnityEditor;
using UnityEngine;
using UnityEngine.TestTools;
using Object = UnityEngine.Object;

namespace Kukuri.GameSdk.Tests
{
    /// 2 つの独立した profile で、作成 → 発見（後から参加した側）→ 表示 → 参加 → 終了を Lobby prefab で確かめる（#1978
    /// AC-1・AC-2、F-LOB-1・F-LOB-2）。test.ps1 が keyring を切り、loopback だけで動かす。
    public sealed class LobbyTests
    {
        const string PrefabPath = "Packages/com.kukuri.gamesdk/Runtime/Lobby/KukuriLobby.prefab";
        const string Application = "com.kukuri.test.lobby";

        KukuriClient a;
        KukuriClient b;
        GameObject lobbyA;
        GameObject lobbyB;

        [TearDown]
        public void TearDown()
        {
            a?.Dispose();
            b?.Dispose();
            if (lobbyA != null)
                Object.DestroyImmediate(lobbyA);
            if (lobbyB != null)
                Object.DestroyImmediate(lobbyB);
        }

        static string NewProfileDirectory() =>
            Path.Combine(Path.GetTempPath(), "kgs-" + Guid.NewGuid().ToString("N").Substring(0, 12));

        /// 応答が来るまで両方の client を pump する。
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

        /// 新しい profile で現在の文書に同意し、アカウントを作る。
        IEnumerator Accept(KukuriClient client)
        {
            JToken consent = null;
            yield return Call(client, "get_app_consent_status", null, data => consent = data);
            var documents = new JArray(((JArray)consent["documents"]).Select(document =>
                new JObject { ["slug"] = document["slug"], ["version"] = document["currentVersion"] }));
            yield return Call(client, "accept_app_consents",
                new JObject { ["documents"] = documents, ["language"] = "ja", ["age_attested"] = true });
        }

        static KukuriLobby Lobby(KukuriClient client, string topic, out GameObject instance)
        {
            instance = Object.Instantiate(AssetDatabase.LoadAssetAtPath<GameObject>(PrefabPath));
            var lobby = instance.GetComponent<KukuriLobby>();
            lobby.Client = client;
            lobby.ApplicationId = Application;
            lobby.Topic = topic;
            return lobby;
        }

        /// 一覧に描いた行（prefab の雛形の複製のうち、表示中のもの）。
        static KukuriLobbyItem[] Shown(GameObject lobby) => lobby.GetComponentsInChildren<KukuriLobbyItem>(true)
            .Where(item => item.name.EndsWith("(Clone)") && item.gameObject.activeSelf)
            .ToArray();

        IEnumerator RefreshUntil(KukuriLobby lobby, Func<bool> condition, string what)
        {
            var clock = Stopwatch.StartNew();
            while (true)
            {
                yield return Wait(lobby.RefreshAsync());
                if (condition())
                    yield break;
                Assert.Less(clock.Elapsed.TotalSeconds, 120, what);
                var pause = Stopwatch.StartNew();
                while (pause.Elapsed.TotalSeconds < 1)
                {
                    a.Pump();
                    b.Pump();
                    yield return null;
                }
            }
        }

        [UnityTest]
        public IEnumerator ARoomCreatedByOneProfileIsFoundJoinedAndEndedForAnother()
        {
            var topic = "kukuri:topic:test-lobby-" + Guid.NewGuid().ToString("N").Substring(0, 8);
            a = KukuriClient.Open(KukuriMode.Ffi, "a", NewProfileDirectory());
            yield return Accept(a);
            var hostLobby = Lobby(a, topic, out lobbyA);
            var create = hostLobby.CreateRoomAsync("決勝戦の練習");
            yield return Wait(create);
            var roomId = create.Result;
            Assert.AreEqual(roomId, hostLobby.Rooms.Single().RoomId);
            Assert.IsTrue(hostLobby.Rooms.Single().IsMine);

            // B は room が作られた後に参加する（late join）。
            b = KukuriClient.Open(KukuriMode.Ffi, "b", NewProfileDirectory());
            yield return Accept(b);
            string ticketA = null, ticketB = null;
            yield return Call(a, "get_local_peer_ticket", null, data => ticketA = (string)data);
            yield return Call(b, "get_local_peer_ticket", null, data => ticketB = (string)data);
            yield return Call(b, "import_peer_ticket", new JObject { ["ticket"] = ticketA });
            yield return Call(a, "import_peer_ticket", new JObject { ["ticket"] = ticketB });
            var guestLobby = Lobby(b, topic, out lobbyB);
            yield return RefreshUntil(guestLobby, () => guestLobby.Rooms.Any(room => room.RoomId == roomId),
                "the other profile did not find the room");
            var found = guestLobby.Rooms.Single();
            Assert.AreEqual("決勝戦の練習", found.Title);
            Assert.AreEqual("Waiting", found.Status);
            Assert.AreEqual(4, found.MaxPlayers);
            Assert.IsFalse(found.IsMine);
            Assert.IsNotEmpty(found.HostEndpointId);
            var shown = Shown(lobbyB).Single();
            Assert.AreEqual("決勝戦の練習", shown.Title);
            StringAssert.Contains("待機中", shown.Detail);

            // 参加すると「参加中」になり、host が終えると session が閉じた知らせが届く。
            string closedReason = null;
            guestLobby.SessionClosed += reason => closedReason = reason;
            var join = guestLobby.JoinRoomAsync(roomId);
            yield return Wait(join);
            Assert.AreEqual(join.Result, guestLobby.JoinedSessionId);
            StringAssert.Contains("参加中", Shown(lobbyB).Single().Detail);

            yield return Wait(hostLobby.EndRoomAsync(roomId));
            Assert.AreEqual("Ended", hostLobby.Rooms.Single().Status);
            var clock = Stopwatch.StartNew();
            while (closedReason == null)
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 60, "the session did not close");
                a.Pump();
                b.Pump();
                yield return null;
            }
            Assert.AreEqual("room_ended", closedReason);
            Assert.IsNull(guestLobby.JoinedSessionId);
            yield return RefreshUntil(guestLobby, () => guestLobby.Rooms.Single().Status == "Ended",
                "the other profile did not see the room end");
            StringAssert.Contains("終了", Shown(lobbyB).Single().Detail);
        }
    }
}
