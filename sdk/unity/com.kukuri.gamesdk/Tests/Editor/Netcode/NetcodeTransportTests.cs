using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Text.RegularExpressions;
using System.Threading.Tasks;
using Newtonsoft.Json.Linq;
using NUnit.Framework;
using Unity.Netcode;
using UnityEngine;
using UnityEngine.TestTools;
using Object = UnityEngine.Object;

namespace Kukuri.GameSdk.Netcode.Tests
{
    /// NGO の Transport の契約（#1979 AC-2）。2 つの独立した profile の action session の上で、connect・send・poll・
    /// disconnect（client が切る・host が外す）と各配送を確かめる。test-netcode.ps1 が NGO 2.13.3 を入れた project で、keyring を切って loopback だけで
    /// 動かす。
    public sealed class NetcodeTransportTests
    {
        KukuriClient a;
        KukuriClient b;
        readonly List<GameObject> objects = new List<GameObject>();

        [TearDown]
        public void TearDown()
        {
            foreach (var created in objects)
                Object.DestroyImmediate(created);
            a?.Dispose();
            b?.Dispose();
        }

        static string NewProfileDirectory() =>
            Path.Combine(Path.GetTempPath(), "kgs-" + Guid.NewGuid().ToString("N").Substring(0, 12));

        void Pump()
        {
            a?.Pump();
            b?.Pump();
        }

        IEnumerator Wait(Task task)
        {
            var clock = Stopwatch.StartNew();
            while (!task.IsCompleted)
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 60, "no response");
                Pump();
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

        KukuriNetcodeTransport Transport(KukuriClient client)
        {
            var created = new GameObject("transport");
            objects.Add(created);
            var transport = created.AddComponent<KukuriNetcodeTransport>();
            transport.Client = client;
            transport.Initialize();
            return transport;
        }

        /// `transport` の event を、`condition` が満たされるまで集める。
        IEnumerator Collect(KukuriNetcodeTransport transport, List<(NetworkEvent kind, ulong id, byte[] data)> into,
            Func<bool> condition, string what)
        {
            var clock = Stopwatch.StartNew();
            while (true)
            {
                Pump();
                NetworkEvent kind;
                while ((kind = transport.PollEvent(out var id, out var payload, out _)) != NetworkEvent.Nothing)
                    into.Add((kind, id, payload.Array == null ? Array.Empty<byte>() : payload.ToArray()));
                if (condition())
                    yield break;
                Assert.Less(clock.Elapsed.TotalSeconds, 60, what);
                yield return null;
            }
        }

        [UnityTest]
        public IEnumerator NgoPacketsOfEveryDeliveryFlowOverTheSessionUntilTheClientDisconnects()
        {
            var topic = "kukuri:topic:test-ngo-" + Guid.NewGuid().ToString("N").Substring(0, 8);
            a = KukuriClient.Open(KukuriMode.Ffi, "a", NewProfileDirectory());
            b = KukuriClient.Open(KukuriMode.Ffi, "b", NewProfileDirectory());
            yield return Accept(a);
            yield return Accept(b);
            foreach (var client in new[] { a, b })
                yield return Call(client, "set_topic_gossip_enabled", new JObject { ["topic"] = topic, ["enabled"] = true });
            string ticketA = null, ticketB = null;
            yield return Call(a, "get_local_peer_ticket", null, data => ticketA = (string)data);
            yield return Call(b, "get_local_peer_ticket", null, data => ticketB = (string)data);
            yield return Call(b, "import_peer_ticket", new JObject { ["ticket"] = ticketA });
            yield return Call(a, "import_peer_ticket", new JObject { ["ticket"] = ticketB });
            string room = null;
            yield return Call(a, "create_game_room", new JObject
            {
                ["topic"] = topic,
                ["channel_ref"] = new JObject { ["kind"] = "public" },
                ["title"] = "ngo",
                ["description"] = "",
                ["participants"] = new JArray(),
                ["action"] = new JObject { ["application_id"] = "com.kukuri.test.ngo", ["max_players"] = 4 },
            }, data => room = (string)data);

            var host = Transport(a);
            host.RoomId = room;
            Assert.IsTrue(host.StartServer());
            // room が b の手元に届くまでは参加を断られる。
            string session = null;
            var clock = Stopwatch.StartNew();
            while (session == null)
            {
                var join = b.SendAsync("join_game_session", new JObject
                {
                    ["topic"] = topic,
                    ["scope"] = new JObject { ["kind"] = "public" },
                    ["room_id"] = room,
                });
                yield return Wait(join);
                if (join.Result.Ok)
                    session = (string)join.Result.Data["session_id"];
                Assert.Less(clock.Elapsed.TotalSeconds, 120, "the room did not reach the guest");
            }
            var guest = Transport(b);
            guest.SessionId = session;
            Assert.IsTrue(guest.StartClient());

            var hostEvents = new List<(NetworkEvent kind, ulong id, byte[] data)>();
            var guestEvents = new List<(NetworkEvent kind, ulong id, byte[] data)>();
            yield return Collect(guest, guestEvents, () => guestEvents.Count == 1, "the guest connects");
            Assert.AreEqual(NetworkEvent.Connect, guestEvents[0].kind);
            Assert.AreEqual(guest.ServerClientId, guestEvents[0].id);
            yield return Collect(host, hostEvents, () => hostEvents.Count == 1, "the host accepts the guest");
            Assert.AreEqual(NetworkEvent.Connect, hostEvents[0].kind);
            var guestId = hostEvents[0].id;

            // 5 種類の配送の packet が、先頭の印を除いた payload のまま届く。
            var deliveries = (NetworkDelivery[])Enum.GetValues(typeof(NetworkDelivery));
            foreach (var delivery in deliveries)
                guest.Send(guest.ServerClientId, new ArraySegment<byte>(new[] { (byte)delivery, (byte)7 }), delivery);
            yield return Collect(host, hostEvents, () => hostEvents.Count == 1 + deliveries.Length,
                "the host receives every delivery");
            CollectionAssert.AreEquivalent(deliveries.Select(delivery => (int)delivery),
                hostEvents.Skip(1).Select(received => (int)received.data[0]));
            Assert.IsTrue(hostEvents.Skip(1).All(received => received.kind == NetworkEvent.Data &&
                received.id == guestId && received.data.Length == 2 && received.data[1] == 7));
            host.Send(guestId, new ArraySegment<byte>(new byte[] { 42 }), NetworkDelivery.ReliableSequenced);
            yield return Collect(guest, guestEvents, () => guestEvents.Count == 2, "the guest receives the reply");
            CollectionAssert.AreEqual(new byte[] { 42 }, guestEvents[1].data);

            // 対応しない配送は、送らずに理由を出す。
            LogAssert.Expect(LogType.Error, new Regex("not supported"));
            guest.Send(guest.ServerClientId, new ArraySegment<byte>(new byte[] { 1 }), (NetworkDelivery)99);

            // client が切ると、host には切断が届く。
            guest.DisconnectLocalClient();
            yield return Collect(host, hostEvents,
                () => hostEvents.Any(received => received.kind == NetworkEvent.Disconnect), "the host sees the guest leave");
            Assert.AreEqual(guestId, hostEvents.Last().id);
            Assert.AreEqual(1 + deliveries.Length + 1, hostEvents.Count, "nothing else arrived");
            guest.Shutdown();

            // host が外すと（NGO の DisconnectRemoteClient）、client には切断が届く。
            var rejoin = b.SendAsync("join_game_session", new JObject
            {
                ["topic"] = topic,
                ["scope"] = new JObject { ["kind"] = "public" },
                ["room_id"] = room,
            });
            yield return Wait(rejoin);
            Assert.IsTrue(rejoin.Result.Ok, rejoin.Result.ErrorCode);
            var second = Transport(b);
            second.SessionId = (string)rejoin.Result.Data["session_id"];
            Assert.IsTrue(second.StartClient());
            var secondEvents = new List<(NetworkEvent kind, ulong id, byte[] data)>();
            hostEvents.Clear();
            yield return Collect(host, hostEvents, () => hostEvents.Count == 1, "the host accepts the guest again");
            Assert.AreEqual(NetworkEvent.Connect, hostEvents[0].kind);
            Assert.AreNotEqual(guestId, hostEvents[0].id, "a new client id");
            host.DisconnectRemoteClient(hostEvents[0].id);
            yield return Collect(second, secondEvents,
                () => secondEvents.Any(received => received.kind == NetworkEvent.Disconnect), "the guest is removed");
            second.Shutdown();
            host.Shutdown();
        }

        /// `UnreliableSequenced` は古い番号・重複を捨てる（番号は 1 周しても比べられる）。
        [Test]
        public void SequencedDatagramsDropOlderNumbers()
        {
            uint last = 0;
            Assert.IsTrue(KukuriNetcodeTransport.Newer(ref last, 1));
            Assert.IsTrue(KukuriNetcodeTransport.Newer(ref last, 3));
            Assert.IsFalse(KukuriNetcodeTransport.Newer(ref last, 2), "older");
            Assert.IsFalse(KukuriNetcodeTransport.Newer(ref last, 3), "duplicate");
            last = uint.MaxValue;
            Assert.IsTrue(KukuriNetcodeTransport.Newer(ref last, 0), "wraps around");
        }
    }
}
