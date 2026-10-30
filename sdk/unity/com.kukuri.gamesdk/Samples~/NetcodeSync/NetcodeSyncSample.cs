using Kukuri.GameSdk.Lobby;
using Kukuri.GameSdk.Netcode;
using Unity.Collections;
using Unity.Netcode;
using UnityEngine;

/// NGO の最小同期 sample（#1979 AC-2）。NetworkManager（Transport は KukuriNetcodeTransport）と KukuriLobby と組む。
/// Lobby で room を作ると host、room に参加すると client として NGO を始め、session が閉じたら止める。
/// host は cube を回し、位置を毎 tick `UnreliableSequenced` の named message で配る。接続した client には、最初の状態を
/// `ReliableFragmentedSequenced` の named message で 1 回送る。client は受けた位置へ cube を動かす。
public sealed class NetcodeSyncSample : MonoBehaviour
{
    const string PositionMessage = "kukuri.sample.position";
    const string SnapshotMessage = "kukuri.sample.snapshot";
    /// 最初の状態の大きさ（分割して送る配送を使う）。
    public const int SnapshotInts = 2048;

    [SerializeField] NetworkManager network;
    [SerializeField] KukuriNetcodeTransport transport;
    [SerializeField] KukuriLobby lobby;
    [SerializeField] Transform cube;

    /// client が受けた位置の数と、最初の状態を受けたか。
    public int ReceivedPositions { get; private set; }
    public bool SnapshotReceived { get; private set; }

    void OnEnable()
    {
        lobby.RoomCreated += StartHost;
        lobby.Joined += StartClient;
        lobby.SessionClosed += Stop;
    }

    void OnDisable()
    {
        lobby.RoomCreated -= StartHost;
        lobby.Joined -= StartClient;
        lobby.SessionClosed -= Stop;
        Stop(null);
    }

    public void StartHost(string roomId)
    {
        transport.RoomId = roomId;
        network.OnClientConnectedCallback += SendSnapshot;
        if (network.StartHost())
            network.NetworkTickSystem.Tick += Broadcast;
    }

    public void StartClient(string sessionId)
    {
        transport.SessionId = sessionId;
        if (!network.StartClient())
            return;
        network.CustomMessagingManager.RegisterNamedMessageHandler(PositionMessage, OnPosition);
        network.CustomMessagingManager.RegisterNamedMessageHandler(SnapshotMessage, OnSnapshot);
    }

    void Stop(string reason)
    {
        if (network.NetworkTickSystem != null)
            network.NetworkTickSystem.Tick -= Broadcast;
        network.OnClientConnectedCallback -= SendSnapshot;
        if (network.IsListening)
            network.Shutdown();
    }

    void Broadcast()
    {
        var angle = Time.time;
        cube.position = new Vector3(Mathf.Cos(angle) * 2, 0, Mathf.Sin(angle) * 2);
        using var writer = new FastBufferWriter(12, Allocator.Temp);
        writer.WriteValueSafe(cube.position);
        network.CustomMessagingManager.SendNamedMessageToAll(PositionMessage, writer, NetworkDelivery.UnreliableSequenced);
    }

    void SendSnapshot(ulong clientId)
    {
        if (clientId == NetworkManager.ServerClientId)
            return;
        using var writer = new FastBufferWriter(4 + SnapshotInts * 4, Allocator.Temp);
        writer.WriteValueSafe(SnapshotInts);
        for (var index = 0; index < SnapshotInts; index++)
            writer.WriteValueSafe(index);
        network.CustomMessagingManager.SendNamedMessage(SnapshotMessage, clientId, writer,
            NetworkDelivery.ReliableFragmentedSequenced);
    }

    void OnPosition(ulong sender, FastBufferReader reader)
    {
        reader.ReadValueSafe(out Vector3 position);
        cube.position = position;
        ReceivedPositions++;
    }

    void OnSnapshot(ulong sender, FastBufferReader reader)
    {
        reader.ReadValueSafe(out int count);
        var last = -1;
        for (var index = 0; index < count; index++)
            reader.ReadValueSafe(out last);
        SnapshotReceived = count == SnapshotInts && last == SnapshotInts - 1;
    }
}
