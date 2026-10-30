using System;
using System.Collections.Generic;
using Newtonsoft.Json.Linq;
using Unity.Netcode;
using UnityEngine;

namespace Kukuri.GameSdk.Netcode
{
    /// Netcode for GameObjects の Transport（ADR 0070 §5.2、#1979 AC-2）。kukuri の action session（`join_game_session` で
    /// 参加し、host が認可した session）の datagram と stream に NGO の packet を載せる。NGO を入れた project だけで
    /// compile される（NGO を使わない利用者に NGO を求めない）。
    ///
    /// host は `RoomId` の room に参加した session を client として受け付け、client は `SessionId` の host へつなぐ。
    /// 応答と event は `KukuriRunner`（または呼出元）の `Pump` が運び、この Transport は `PollEvent` で NGO へ渡す。
    ///
    /// 配送: `Unreliable` と `UnreliableSequenced` は datagram（後者は通し番号で古いものを捨てる）、`Reliable`・
    /// `ReliableSequenced`・`ReliableFragmentedSequenced` は順に全部届く stream。NGO の 1 batch と分割する message の上限は
    /// `Initialize` で SDK の上限に合わせ、それを超えるものは送らずに error を出す。
    public sealed class KukuriNetcodeTransport : NetworkTransport
    {
        /// datagram の先頭 1 byte。`Sequenced` には 4 bytes の通し番号が続く。
        const byte Plain = 0;
        const byte Sequenced = 1;
        /// NGO の分割しない 1 batch の上限（datagram の 1100 bytes から先頭の 5 bytes を引き、8 の倍数に揃える）。
        public const int MaxUnfragmentedBytes = 1088;
        /// 分割する message の上限（stream の 1 message）。
        public const int MaxFragmentedBytes = 64 * 1024;
        /// stream の送り待ちが上限で受け付けられなかった分を、相手ごとに持つ上限。超えたらその相手を切る。
        const int MaxPendingBytes = 1 << 20;
        /// host が `game_session_opened` より先に受けた packet を持つ上限（session ごと）。
        const int MaxEarlyPackets = 64;

        [SerializeField] KukuriRunner runner;

        /// `runner` を使わないときの client。
        public KukuriClient Client { get; set; }

        /// host の room（`StartServer` の前に設定する）。
        public string RoomId { get; set; }

        /// client の session（`join_game_session` の応答。`StartClient` の前に設定する）。
        public string SessionId { get; set; }

        public override ulong ServerClientId => 0;

        sealed class Peer
        {
            public string SessionId;
            public uint SentSequence;
            public uint ReceivedSequence;
            public ulong RttMilliseconds;
            public readonly Queue<byte[]> Pending = new Queue<byte[]>();
            public int PendingBytes;
        }

        readonly Dictionary<ulong, Peer> peers = new Dictionary<ulong, Peer>();
        readonly Dictionary<string, ulong> bySession = new Dictionary<string, ulong>();
        readonly Dictionary<string, List<KukuriPacket>> early = new Dictionary<string, List<KukuriPacket>>();
        readonly Queue<(NetworkEvent kind, ulong clientId, ArraySegment<byte> payload)> events =
            new Queue<(NetworkEvent, ulong, ArraySegment<byte>)>();
        KukuriClient client;
        ulong nextClientId = 1;
        bool server;
        bool started;

        public override void Initialize(NetworkManager networkManager = null)
        {
            client = Client ?? (runner != null ? runner.Client : null);
            if (client == null)
                throw new InvalidOperationException("KukuriNetcodeTransport needs an open KukuriClient (set runner or Client)");
            if (networkManager != null)
            {
                networkManager.MaximumTransmissionUnitSize = MaxUnfragmentedBytes;
                networkManager.MaximumFragmentedMessageSize = MaxFragmentedBytes;
            }
        }

        public override bool StartServer()
        {
            if (string.IsNullOrEmpty(RoomId))
                return false;
            server = true;
            Begin();
            return true;
        }

        public override bool StartClient()
        {
            if (string.IsNullOrEmpty(SessionId))
                return false;
            server = false;
            Begin();
            Add(ServerClientId, SessionId);
            events.Enqueue((NetworkEvent.Connect, ServerClientId, default));
            return true;
        }

        void Begin()
        {
            started = true;
            client.EventReceived += OnEvent;
            client.PacketReceived += OnPacket;
        }

        void Add(ulong clientId, string sessionId)
        {
            peers[clientId] = new Peer { SessionId = sessionId };
            bySession[sessionId] = clientId;
        }

        void Remove(ulong clientId)
        {
            if (peers.Remove(clientId, out var peer))
                bySession.Remove(peer.SessionId);
        }

        void OnEvent(JObject @event)
        {
            var type = (string)@event["type"];
            var sessionId = (string)@event["session_id"];
            if (type == "game_session_opened" && server && (string)@event["room_id"] == RoomId &&
                !bySession.ContainsKey(sessionId))
            {
                var clientId = nextClientId++;
                Add(clientId, sessionId);
                events.Enqueue((NetworkEvent.Connect, clientId, default));
                if (early.Remove(sessionId, out var packets))
                    foreach (var packet in packets)
                        OnPacket(packet);
            }
            else if (type == "game_session_closed" && bySession.TryGetValue(sessionId, out var closed))
            {
                Remove(closed);
                events.Enqueue((NetworkEvent.Disconnect, closed, default));
            }
            else if (type == "game_session_path" && bySession.TryGetValue(sessionId, out var measured))
            {
                peers[measured].RttMilliseconds = (ulong?)@event["rtt_ms"] ?? 0;
            }
            else if (type == "game_session_closed")
            {
                early.Remove(sessionId);
            }
        }

        void OnPacket(KukuriPacket packet)
        {
            if (!bySession.TryGetValue(packet.SessionId, out var clientId))
            {
                // host には、開いた知らせより先に参加者の最初の packet が届くことがある。開いたら渡す。
                if (server)
                {
                    if (!early.TryGetValue(packet.SessionId, out var held))
                        early[packet.SessionId] = held = new List<KukuriPacket>();
                    if (held.Count < MaxEarlyPackets)
                        held.Add(packet);
                }
                return;
            }
            var data = packet.Data;
            if (packet.Delivery == KukuriDelivery.Stream)
            {
                events.Enqueue((NetworkEvent.Data, clientId, new ArraySegment<byte>(data)));
                return;
            }
            if (data.Length == 0)
                return;
            if (data[0] == Sequenced)
            {
                if (data.Length < 5 || !Newer(ref peers[clientId].ReceivedSequence, BitConverter.ToUInt32(data, 1)))
                    return;
                events.Enqueue((NetworkEvent.Data, clientId, new ArraySegment<byte>(data, 5, data.Length - 5)));
            }
            else if (data[0] == Plain)
            {
                events.Enqueue((NetworkEvent.Data, clientId, new ArraySegment<byte>(data, 1, data.Length - 1)));
            }
        }

        /// `received` が直前に受けた番号より新しければ記録して true（古いもの・重複は捨てる）。
        internal static bool Newer(ref uint last, uint received)
        {
            if ((int)(received - last) <= 0)
                return false;
            last = received;
            return true;
        }

        public override void Send(ulong clientId, ArraySegment<byte> payload, NetworkDelivery networkDelivery)
        {
            if (!peers.TryGetValue(clientId, out var peer))
                return;
            byte[] bytes;
            KukuriDelivery delivery;
            switch (networkDelivery)
            {
                case NetworkDelivery.Unreliable:
                    bytes = Prefixed(payload, Plain, 0);
                    delivery = KukuriDelivery.Datagram;
                    break;
                case NetworkDelivery.UnreliableSequenced:
                    bytes = Prefixed(payload, Sequenced, ++peer.SentSequence);
                    delivery = KukuriDelivery.Datagram;
                    break;
                case NetworkDelivery.Reliable:
                case NetworkDelivery.ReliableSequenced:
                case NetworkDelivery.ReliableFragmentedSequenced:
                    bytes = payload.ToArray();
                    delivery = KukuriDelivery.Stream;
                    break;
                default:
                    Debug.LogError($"kukuri: the NGO delivery {networkDelivery} is not supported; the message was not sent");
                    return;
            }
            if (delivery == KukuriDelivery.Stream && peer.Pending.Count > 0)
            {
                Hold(clientId, peer, bytes);
                return;
            }
            switch (client.SendPacket(peer.SessionId, delivery, bytes))
            {
                case KukuriSendStatus.Backpressure:
                    Hold(clientId, peer, bytes);
                    break;
                case KukuriSendStatus.TooLarge:
                    Debug.LogError($"kukuri: an NGO {networkDelivery} message of {payload.Count} bytes exceeds the session limit; it was not sent");
                    if (delivery == KukuriDelivery.Stream)
                        Drop(clientId);
                    break;
                case KukuriSendStatus.Closed:
                case KukuriSendStatus.Faulted:
                    Drop(clientId);
                    break;
            }
        }

        static byte[] Prefixed(ArraySegment<byte> payload, byte kind, uint sequence)
        {
            var header = kind == Sequenced ? 5 : 1;
            var bytes = new byte[header + payload.Count];
            bytes[0] = kind;
            if (kind == Sequenced)
                BitConverter.GetBytes(sequence).CopyTo(bytes, 1);
            payload.AsSpan().CopyTo(bytes.AsSpan(header));
            return bytes;
        }

        /// stream の送り待ちが空くまで手元に持つ（順を保つ）。上限を超えたら相手を切る。
        void Hold(ulong clientId, Peer peer, byte[] bytes)
        {
            peer.Pending.Enqueue(bytes);
            peer.PendingBytes += bytes.Length;
            if (peer.PendingBytes > MaxPendingBytes)
            {
                Debug.LogError("kukuri: the session cannot keep up with the reliable NGO messages; disconnecting");
                Drop(clientId);
            }
        }

        void Flush()
        {
            foreach (var pair in new List<KeyValuePair<ulong, Peer>>(peers))
            {
                var peer = pair.Value;
                while (peer.Pending.Count > 0)
                {
                    var status = client.SendPacket(peer.SessionId, KukuriDelivery.Stream, peer.Pending.Peek());
                    if (status == KukuriSendStatus.Backpressure)
                        break;
                    peer.PendingBytes -= peer.Pending.Dequeue().Length;
                    if (status != KukuriSendStatus.Sent)
                    {
                        Drop(pair.Key);
                        break;
                    }
                }
            }
        }

        /// 相手との session を閉じ（host は参加者を外し、client は退出する）、NGO へ切断を渡す。
        void Drop(ulong clientId)
        {
            if (!peers.TryGetValue(clientId, out var peer))
                return;
            Remove(clientId);
            Leave(peer.SessionId);
            events.Enqueue((NetworkEvent.Disconnect, clientId, default));
        }

        /// host は参加者を外し、client は退出する（client を閉じた後なら session はもう閉じている）。
        void Leave(string sessionId)
        {
            try
            {
                _ = client.SendAsync("leave_game_session", new JObject { ["session_id"] = sessionId });
            }
            catch (KukuriException)
            {
                // client を閉じた・要求の待ち行列が満杯。外す・退出する要求は送れないが、NGO への切断は渡す。
            }
        }

        public override NetworkEvent PollEvent(out ulong clientId, out ArraySegment<byte> payload, out float receiveTime)
        {
            receiveTime = Time.realtimeSinceStartup;
            if (started)
                Flush();
            if (events.Count == 0)
            {
                clientId = 0;
                payload = default;
                return NetworkEvent.Nothing;
            }
            var next = events.Dequeue();
            clientId = next.clientId;
            payload = next.payload;
            return next.kind;
        }

        public override void DisconnectRemoteClient(ulong clientId)
        {
            if (peers.TryGetValue(clientId, out var peer))
            {
                Remove(clientId);
                Leave(peer.SessionId);
            }
        }

        public override void DisconnectLocalClient()
        {
            DisconnectRemoteClient(ServerClientId);
        }

        public override ulong GetCurrentRtt(ulong clientId)
        {
            return peers.TryGetValue(clientId, out var peer) ? peer.RttMilliseconds : 0;
        }

        public override void Shutdown()
        {
            if (!started)
                return;
            foreach (var clientId in new List<ulong>(peers.Keys))
                DisconnectRemoteClient(clientId);
            client.EventReceived -= OnEvent;
            client.PacketReceived -= OnPacket;
            early.Clear();
            events.Clear();
            started = false;
        }
    }
}
