using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using Kukuri.GameSdk;
using Kukuri.GameSdk.Lobby;
using Kukuri.GameSdk.Netcode;
using Newtonsoft.Json.Linq;
using Unity.Collections;
using Unity.Netcode;
using UnityEngine;

namespace CyberDiscArena
{
    /// 対戦の通信と裁定（ADR 0070 §6）。Lobby で room を作った端末が host（player 0）、参加した端末が client（player 1）
    /// として NGO を始める。host だけが `ArenaRules` を進め、ディスクの持ち主・得点・勝敗を確定する。
    /// - client → host: 入力を毎 tick datagram で（落ちても届くよう直近 4 tick 分を重ねる）。投げる・受け止める要求もそこへ重ねる。
    /// - host → 全員: 状態を毎 tick datagram で、確定した出来事を通し番号つきの stream で（直前の出来事は状態にも重ねる）。
    /// client は自分の動きを予測して表示し、host の状態が届いたら、host が適用した入力より後の入力を当て直して補正する。
    /// ディスクの持ち主と得点は確定した出来事でだけ変える。起動引数 `-arena-delay <ms>`・`-arena-loss <%>` で受信を遅らせ、
    /// datagram を落とす（回線の悪い状態の確認用）。
    [DefaultExecutionOrder(-100)]
    public sealed class ArenaSession : MonoBehaviour
    {
        const string InputMessage = "arena.input";
        const string StateMessage = "arena.state";
        const string EventMessage = "arena.event";
        const string SkinMessage = "arena.skin";
        const int InputRedundancy = 4;
        const int History = 64;
        /// 確定した出来事を状態に重ねて送る期間（出来事の送信が落ちても、次の状態で届く）。
        const float RecentEventSeconds = 0.5f;

        [SerializeField] KukuriRunner runner;
        [SerializeField] NetworkManager network;
        [SerializeField] KukuriNetcodeTransport transport;
        [SerializeField] KukuriLobby lobby;

        readonly ArenaInput[] history = new ArenaInput[History];
        readonly ArenaInput[] inputs = new ArenaInput[2];
        readonly SortedList<uint, ArenaInput> pending = new SortedList<uint, ArenaInput>();
        readonly List<(float at, string name, byte[] bytes)> inbox = new List<(float, string, byte[])>();
        readonly List<(float at, ArenaEvent @event)> recentEvents = new List<(float, ArenaEvent)>();
        readonly System.Random random = new System.Random(1983);
        ArenaRules rules;
        ulong opponent = ulong.MaxValue;
        string roomId;
        uint sequence;
        /// client: 最後の操作（通し番号・投げるか・向き）。入力の送信に毎回重ねる。
        (uint id, bool throwing, float yaw) action;
        /// host: 扱った client の操作の通し番号。
        uint remoteAction;
        float delay;
        float loss;
        float stateAt;
        DiscState received;
        Vector3 correction;
        /// 自分で対戦をやめている途中。
        bool ending;

        /// 自分の player の番号（host は 0、client は 1。対戦していなければ -1）。
        public int Local { get; private set; } = -1;
        public bool IsHost => rules != null;
        public bool InMatch => Local >= 0;
        /// 自分で対戦をやめている途中（host は相手の session が閉じるのを待つ）。
        public bool Ending => ending;
        /// 確定した状態（client は受けた出来事から）。
        public ArenaPhase Phase { get; private set; }
        public int Holder { get; private set; } = -1;
        public int Winner { get; private set; } = -1;
        public byte[] Scores { get; } = new byte[2];
        public float TimeLeft { get; private set; }
        /// 適用した最後の出来事の通し番号と、番号の飛びの数（stream なので 0 のはず）。
        public uint LastEvent { get; private set; }
        public int EventGaps { get; private set; }
        /// 表示する 2 人の状態（自分は予測、相手は host の状態）とディスク。
        public PlayerState[] Players { get; } = new PlayerState[2];
        public DiscState Disc { get; private set; }
        /// 予測を補正したときの表示のずれ（少しずつ 0 へ戻す）。
        public Vector3 Correction => correction;
        /// 接続の経路と RTT（`game_session_path`）。
        public string Path { get; private set; } = "";
        public long RttMs { get; private set; } = -1;
        /// 相手が着ているスキン（対戦の開始で互いに送る）。
        public string RemoteSkin { get; private set; } = ArenaProfile.StandardSkin;
        /// 自分が着ているスキン（相手へ送る）。
        public Func<string> LocalSkin { get; set; } = () => ArenaProfile.StandardSkin;
        /// 退出・終了の理由など、画面に出す知らせ。
        public string Notice { get; private set; } = "";

        /// 確定した出来事を適用した（試験・演出用）。
        public event Action<ArenaEvent> Confirmed;

        /// 入力の取り出し口。既定はキーボード（`ArenaControls`）。試験は差し替える。
        public Func<int, ArenaInput> InputSource { get; set; }

        void Awake()
        {
            var args = Environment.GetCommandLineArgs();
            string Arg(string name)
            {
                var at = Array.IndexOf(args, name);
                return at >= 0 && at + 1 < args.Length ? args[at + 1] : null;
            }
            delay = float.TryParse(Arg("-arena-delay"), out var ms) ? ms / 1000f : 0;
            loss = float.TryParse(Arg("-arena-loss"), out var percent) ? percent / 100f : 0;
            network.NetworkConfig.TickRate = (uint)Mathf.RoundToInt(1 / ArenaRules.TickSeconds);
        }

        void OnEnable()
        {
            lobby.RoomCreated += StartHost;
            lobby.Joined += StartClient;
            lobby.SessionClosed += OnSessionClosed;
        }

        void OnDisable()
        {
            lobby.RoomCreated -= StartHost;
            lobby.Joined -= StartClient;
            lobby.SessionClosed -= OnSessionClosed;
            Stop();
        }

        void Update()
        {
            for (var index = 0; index < inbox.Count;)
            {
                if (inbox[index].at > Time.unscaledTime)
                {
                    index++;
                    continue;
                }
                var (_, name, bytes) = inbox[index];
                inbox.RemoveAt(index);
                using var reader = new FastBufferReader(bytes, Allocator.Temp);
                Handle(name, reader);
            }
            correction = Vector3.Lerp(correction, Vector3.zero, 1 - Mathf.Exp(-12 * Time.deltaTime));
            if (!IsHost && Holder < 0 && received.Mode == DiscMode.Flying && Phase == ArenaPhase.Playing)
            {
                var disc = received;
                disc.Position += received.Velocity * (Time.unscaledTime - stateAt);
                Disc = disc;
            }
        }

        void StartHost(string room)
        {
            Reset(0);
            roomId = room;
            rules = new ArenaRules();
            transport.RoomId = room;
            network.OnClientConnectedCallback += OnClientConnected;
            network.OnClientDisconnectCallback += OnClientDisconnected;
            if (!network.StartHost())
            {
                Notice = "対戦を始められませんでした";
                Stop();
                return;
            }
            Register();
            Phase = ArenaPhase.Waiting;
            Notice = "相手を待っています";
        }

        void StartClient(string sessionId)
        {
            Reset(1);
            transport.SessionId = sessionId;
            network.OnClientConnectedCallback += OnClientConnected;
            network.OnClientDisconnectCallback += OnClientDisconnected;
            if (!network.StartClient())
            {
                Notice = "接続できませんでした";
                Stop();
                return;
            }
            Register();
            Phase = ArenaPhase.Waiting;
            Notice = "接続しています";
        }

        void Reset(int local)
        {
            Local = local;
            ending = false;
            inputs[0] = inputs[1] = default;
            Phase = ArenaPhase.Waiting;
            Holder = Winner = -1;
            Scores[0] = Scores[1] = 0;
            LastEvent = 0;
            EventGaps = 0;
            sequence = 0;
            action = default;
            remoteAction = 0;
            pending.Clear();
            inbox.Clear();
            recentEvents.Clear();
            correction = Vector3.zero;
            for (var index = 0; index < 2; index++)
                Players[index] = new PlayerState { Position = ArenaRules.Spawn(index), Yaw = ArenaRules.SpawnYaw(index) };
            Disc = default;
            Path = "";
            RttMs = -1;
            RemoteSkin = ArenaProfile.StandardSkin;
            if (runner.Client != null)
                runner.Client.EventReceived += OnSdkEvent;
        }

        void Register()
        {
            var messages = network.CustomMessagingManager;
            foreach (var name in new[] { InputMessage, StateMessage, EventMessage, SkinMessage })
                messages.RegisterNamedMessageHandler(name, (sender, reader) => Receive(name, reader));
            network.NetworkTickSystem.Tick += OnTick;
        }

        /// 受けた message を写して、遅らせる時間の後に扱う（datagram は割合に従って落とす）。
        void Receive(string name, FastBufferReader reader)
        {
            var unreliable = name == InputMessage || name == StateMessage;
            if (unreliable && loss > 0 && random.NextDouble() < loss)
                return;
            var bytes = new byte[reader.Length - reader.Position];
            reader.ReadBytesSafe(ref bytes, bytes.Length);
            inbox.Add((Time.unscaledTime + delay, name, bytes));
        }

        void Handle(string name, FastBufferReader reader)
        {
            switch (name)
            {
                case InputMessage when IsHost:
                    reader.ReadValueSafe(out uint id);
                    reader.ReadValueSafe(out bool throwing);
                    reader.ReadValueSafe(out float yaw);
                    // 操作はどの入力の送信にも重なって届くので、新しい通し番号だけを扱う（試合中でなければ番号だけ進める）。
                    if (id > remoteAction)
                    {
                        remoteAction = id;
                        if (rules.Phase == ArenaPhase.Playing)
                        {
                            if (throwing)
                                rules.Throw(1, yaw);
                            else
                                rules.Catch(1);
                            SendEvents();
                        }
                    }
                    reader.ReadValueSafe(out byte count);
                    for (var index = 0; index < count; index++)
                    {
                        var input = ReadInput(reader);
                        if (input.Sequence > inputs[1].Sequence && !pending.ContainsKey(input.Sequence))
                            pending.Add(input.Sequence, input);
                    }
                    break;
                case StateMessage when !IsHost:
                    ReadState(reader);
                    break;
                case EventMessage when !IsHost:
                    Apply(ReadEvent(reader));
                    break;
                case SkinMessage:
                    reader.ReadValueSafe(out string skin);
                    RemoteSkin = skin;
                    break;
            }
        }

        void OnTick()
        {
            if (!InMatch)
                return;
            var moving = Phase == ArenaPhase.Countdown || Phase == ArenaPhase.Playing;
            var input = Sample();
            if (IsHost)
            {
                inputs[0] = input;
                inputs[1] = NextRemoteInput();
                rules.Step(inputs, ArenaRules.TickSeconds);
                Publish();
                return;
            }
            history[input.Sequence % History] = input;
            if (moving)
                ArenaRules.Move(ref Players[1], 1, input, ArenaRules.TickSeconds);
            SendInputs();
        }

        /// client: 最後の操作と直近の入力を重ねて送る（落ちても次の送信で届く）。
        void SendInputs()
        {
            using var writer = new FastBufferWriter(10 + InputRedundancy * 22, Allocator.Temp);
            writer.WriteValueSafe(action.id);
            writer.WriteValueSafe(action.throwing);
            writer.WriteValueSafe(action.yaw);
            var count = (byte)Math.Min(InputRedundancy, sequence);
            writer.WriteValueSafe(count);
            for (var back = 0; back < count; back++)
                WriteInput(writer, history[(sequence - (uint)back) % History]);
            network.CustomMessagingManager.SendNamedMessage(InputMessage, NetworkManager.ServerClientId, writer,
                NetworkDelivery.Unreliable);
        }

        ArenaInput Sample()
        {
            var input = InputSource != null ? InputSource(Local) : ArenaControls.Read(this);
            input.Sequence = ++sequence;
            return input;
        }

        /// client の入力を通し番号の順に 1 つずつ使う。届いていなければ前の入力を続ける。溜まりすぎたら古い分を捨てる。
        ArenaInput NextRemoteInput()
        {
            while (pending.Count > 3)
                pending.RemoveAt(0);
            if (pending.Count == 0)
                return inputs[1];
            var next = pending.Values[0];
            pending.RemoveAt(0);
            return next;
        }

        /// host: 確定した出来事を全員へ送って自分にも適用し、状態を配る。
        void Publish()
        {
            SendEvents();
            recentEvents.RemoveAll(recent => recent.at < Time.unscaledTime - RecentEventSeconds);
            for (var index = 0; index < 2; index++)
                Players[index] = rules.Players[index];
            Disc = rules.Disc;
            TimeLeft = rules.TimeLeft;
            if (opponent == ulong.MaxValue)
                return;
            using var state = new FastBufferWriter(129 + recentEvents.Count * 34, Allocator.Temp);
            state.WriteValueSafe(rules.TimeLeft);
            foreach (var player in rules.Players)
                WritePlayer(state, player);
            state.WriteValueSafe((byte)rules.Disc.Mode);
            state.WriteValueSafe(rules.Disc.Position);
            state.WriteValueSafe(rules.Disc.Velocity);
            state.WriteValueSafe((byte)recentEvents.Count);
            foreach (var (_, @event) in recentEvents)
                WriteEvent(state, @event);
            network.CustomMessagingManager.SendNamedMessage(StateMessage, opponent, state, NetworkDelivery.UnreliableSequenced);
        }

        /// host: 確定した出来事を全員へ送って自分にも適用する（client の操作で確定した出来事は tick を待たずに送る）。
        void SendEvents()
        {
            foreach (var @event in rules.Events)
            {
                using var writer = new FastBufferWriter(64, Allocator.Temp);
                WriteEvent(writer, @event);
                network.CustomMessagingManager.SendNamedMessageToAll(EventMessage, writer, NetworkDelivery.ReliableSequenced);
                recentEvents.Add((Time.unscaledTime, @event));
                Apply(@event);
            }
            rules.Events.Clear();
        }

        /// client: host の状態。相手はそのまま、自分は host が適用した入力より後の入力を当て直す（ずれは表示で埋める）。
        void ReadState(FastBufferReader reader)
        {
            reader.ReadValueSafe(out float timeLeft);
            var host = ReadPlayer(reader);
            var mine = ReadPlayer(reader);
            reader.ReadValueSafe(out byte mode);
            reader.ReadValueSafe(out Vector3 position);
            reader.ReadValueSafe(out Vector3 velocity);
            // 状態に重なった出来事のうち、次の番号のものから順に適用する（間が欠けたら、確実に届く送信を待つ）。
            reader.ReadValueSafe(out byte recent);
            for (var index = 0; index < recent; index++)
            {
                var @event = ReadEvent(reader);
                if (@event.Sequence == LastEvent + 1)
                    Apply(@event);
            }
            TimeLeft = timeLeft;
            Players[0] = host;
            var predicted = Players[1].Position;
            if (Phase == ArenaPhase.Countdown || Phase == ArenaPhase.Playing)
                for (var next = mine.LastInput + 1; next <= sequence && sequence - next < History; next++)
                    ArenaRules.Move(ref mine, 1, history[next % History], ArenaRules.TickSeconds);
            Players[1] = mine;
            correction += predicted - mine.Position;
            received = new DiscState { Mode = (DiscMode)mode, Holder = Holder, Position = position, Velocity = velocity };
            stateAt = Time.unscaledTime;
            if (received.Mode == DiscMode.Flying && Holder < 0)
                Disc = received;
        }

        /// 確定した出来事を適用する（client は通し番号の順。重なりは捨てる）。
        void Apply(ArenaEvent @event)
        {
            if (@event.Sequence <= LastEvent)
                return;
            if (@event.Sequence != LastEvent + 1)
                EventGaps++;
            LastEvent = @event.Sequence;
            Phase = @event.Phase;
            Scores[0] = @event.Score0;
            Scores[1] = @event.Score1;
            Winner = @event.Phase == ArenaPhase.Result ? @event.Player : -1;
            Holder = @event.Target;
            if (!IsHost)
            {
                if (@event.Kind == ArenaEventKind.Throw)
                {
                    received = new DiscState { Mode = DiscMode.Flying, Holder = -1, Thrower = @event.Player, Position = @event.Position, Velocity = @event.Velocity };
                    stateAt = Time.unscaledTime;
                    Disc = received;
                }
                else if (Holder >= 0)
                    Disc = new DiscState { Mode = DiscMode.Held, Holder = Holder };
                if (@event.Kind == ArenaEventKind.Phase && @event.Phase == ArenaPhase.Countdown)
                    Players[1] = new PlayerState { Position = ArenaRules.Spawn(1), Yaw = ArenaRules.SpawnYaw(1), LastInput = sequence };
            }
            // 自分で対戦をやめている途中は「部屋を閉じています」を出したままにする。
            if (!ending)
                Notice = @event.Phase switch
                {
                    ArenaPhase.Countdown => "まもなく開始",
                    ArenaPhase.Left => "相手が退出しました",
                    _ => "",
                };
            Confirmed?.Invoke(@event);
        }

        /// 持っていれば投げ、持っていなければ受け止める（E キー）。
        public void Act()
        {
            if (!InMatch || Phase != ArenaPhase.Playing)
                return;
            var throwing = Holder == Local;
            var yaw = ArenaControls.FacingYaw(this);
            if (IsHost)
            {
                if (throwing)
                    rules.Throw(0, yaw);
                else
                    rules.Catch(0);
                return;
            }
            action = (action.id + 1, throwing, yaw);
            SendInputs();
        }

        void OnClientConnected(ulong clientId)
        {
            // 着ているスキンを相手へ送る（client はつながったとき host へ、host は相手が来たとき相手へ）。
            if (!IsHost)
            {
                if (clientId == network.LocalClientId)
                    SendSkin(NetworkManager.ServerClientId);
                return;
            }
            if (clientId == NetworkManager.ServerClientId)
                return;
            // 1 対 1。3 人目は切る。
            if (opponent != ulong.MaxValue)
            {
                network.DisconnectClient(clientId);
                return;
            }
            opponent = clientId;
            SendSkin(clientId);
            rules.Begin();
        }

        void OnClientDisconnected(ulong clientId)
        {
            if (IsHost)
            {
                if (clientId != opponent)
                    return;
                opponent = ulong.MaxValue;
                if (!ending && rules.Phase != ArenaPhase.Result)
                    rules.Leave();
                return;
            }
            if (!InMatch)
                return;
            Notice = "接続が切れました";
            Stop();
        }

        /// 参加中の session が閉じた（client）。自分で退出したとき以外は理由を出してロビーへ戻る。
        void OnSessionClosed(string reason)
        {
            if (IsHost || reason == "left")
                return;
            Notice = reason == "room_ended" ? "ホストが部屋を閉じました" : "接続が切れました";
            Stop();
        }

        /// 対戦をやめてロビーへ戻る。client は退出する。host は room を終え、相手の session が「部屋が終了した」で閉じるのを
        /// 待ってから止める（SDK は session を 5 秒ごとに確かめる。先に止めると相手には接続が切れたように見える）。
        public async void Leave()
        {
            var host = IsHost;
            var room = roomId;
            Notice = host ? "部屋を閉じています" : "";
            ending = true;
            if (!host)
                Stop();
            try
            {
                if (host)
                    await lobby.EndRoomAsync(room);
                else
                    await lobby.LeaveRoomAsync();
            }
            catch (KukuriException error)
            {
                Notice = $"ロビーへの戻りで失敗しました（{error.Code}）";
            }
            var deadline = Time.realtimeSinceStartup + 6;
            while (host && opponent != ulong.MaxValue && Time.realtimeSinceStartup < deadline)
                await Task.Yield();
            if (host)
            {
                Stop();
                if (Notice == "部屋を閉じています")
                    Notice = "";
            }
        }

        void Stop()
        {
            if (network.NetworkTickSystem != null)
                network.NetworkTickSystem.Tick -= OnTick;
            network.OnClientConnectedCallback -= OnClientConnected;
            network.OnClientDisconnectCallback -= OnClientDisconnected;
            if (network.CustomMessagingManager != null)
                foreach (var name in new[] { InputMessage, StateMessage, EventMessage, SkinMessage })
                    network.CustomMessagingManager.UnregisterNamedMessageHandler(name);
            if (network.IsListening)
                network.Shutdown();
            if (runner.Client != null)
                runner.Client.EventReceived -= OnSdkEvent;
            rules = null;
            opponent = ulong.MaxValue;
            roomId = null;
            Local = -1;
            pending.Clear();
            inbox.Clear();
        }

        void SendSkin(ulong target)
        {
            using var writer = new FastBufferWriter(256, Allocator.Temp);
            writer.WriteValueSafe(LocalSkin());
            network.CustomMessagingManager.SendNamedMessage(SkinMessage, target, writer, NetworkDelivery.ReliableSequenced);
        }

        void OnSdkEvent(JObject @event)
        {
            if ((string)@event["type"] != "game_session_path")
                return;
            Path = (string)@event["path"] switch
            {
                "direct_p2p" => "Direct P2P",
                "relay_supported_p2p" => "Relay Supported P2P",
                "relay_fallback" => "Relay Fallback",
                var other => other,
            };
            RttMs = (long?)@event["rtt_ms"] ?? -1;
        }

        static void WriteInput(FastBufferWriter writer, ArenaInput input)
        {
            writer.WriteValueSafe(input.Sequence);
            writer.WriteValueSafe(input.Move);
            writer.WriteValueSafe(input.Yaw);
            writer.WriteValueSafe((byte)((input.Jump ? 1 : 0) | (input.Crouch ? 2 : 0)));
        }

        static ArenaInput ReadInput(FastBufferReader reader)
        {
            reader.ReadValueSafe(out uint sequence);
            reader.ReadValueSafe(out Vector2 move);
            reader.ReadValueSafe(out float yaw);
            reader.ReadValueSafe(out byte flags);
            return new ArenaInput { Sequence = sequence, Move = move, Yaw = yaw, Jump = (flags & 1) != 0, Crouch = (flags & 2) != 0 };
        }

        static void WritePlayer(FastBufferWriter writer, PlayerState player)
        {
            writer.WriteValueSafe(player.Position);
            writer.WriteValueSafe(player.VerticalSpeed);
            writer.WriteValueSafe(player.Yaw);
            writer.WriteValueSafe(player.Crouching);
            writer.WriteValueSafe(player.SlideTime);
            writer.WriteValueSafe(player.SlideDirection);
            writer.WriteValueSafe(player.LastInput);
        }

        static PlayerState ReadPlayer(FastBufferReader reader)
        {
            var player = new PlayerState();
            reader.ReadValueSafe(out player.Position);
            reader.ReadValueSafe(out player.VerticalSpeed);
            reader.ReadValueSafe(out player.Yaw);
            reader.ReadValueSafe(out player.Crouching);
            reader.ReadValueSafe(out player.SlideTime);
            reader.ReadValueSafe(out player.SlideDirection);
            reader.ReadValueSafe(out player.LastInput);
            return player;
        }

        static void WriteEvent(FastBufferWriter writer, ArenaEvent @event)
        {
            writer.WriteValueSafe(@event.Sequence);
            writer.WriteValueSafe((byte)@event.Kind);
            writer.WriteValueSafe((byte)@event.Phase);
            writer.WriteValueSafe((sbyte)@event.Player);
            writer.WriteValueSafe((sbyte)@event.Target);
            writer.WriteValueSafe(@event.Position);
            writer.WriteValueSafe(@event.Velocity);
            writer.WriteValueSafe(@event.Score0);
            writer.WriteValueSafe(@event.Score1);
        }

        static ArenaEvent ReadEvent(FastBufferReader reader)
        {
            reader.ReadValueSafe(out uint sequence);
            reader.ReadValueSafe(out byte kind);
            reader.ReadValueSafe(out byte phase);
            reader.ReadValueSafe(out sbyte player);
            reader.ReadValueSafe(out sbyte target);
            reader.ReadValueSafe(out Vector3 position);
            reader.ReadValueSafe(out Vector3 velocity);
            reader.ReadValueSafe(out byte score0);
            reader.ReadValueSafe(out byte score1);
            return new ArenaEvent
            {
                Sequence = sequence,
                Kind = (ArenaEventKind)kind,
                Phase = (ArenaPhase)phase,
                Player = player,
                Target = target,
                Position = position,
                Velocity = velocity,
                Score0 = score0,
                Score1 = score1,
            };
        }
    }
}
