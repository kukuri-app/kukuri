using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace Kukuri.GameSdk
{
    /// 要求の行き先。本番の Player の既定は FFI（process の中で runtime を動かす）。Sidecar は開発用（ADR 0070 §8.2）。
    public enum KukuriMode
    {
        Ffi,
        Sidecar,
    }

    /// SDK が返す失敗。`Code` は `kukuri-cli` の error code（ADR 0070 §3）か、C# の側の理由。
    public sealed class KukuriException : Exception
    {
        public string Code { get; }

        public KukuriException(string code, string message) : base(message)
        {
            Code = code;
        }
    }

    /// action session の packet の配送（ADR 0070 §5.2）。最新の状態は欠けてよい `Datagram`（1 件 1100 bytes まで）、
    /// 確定させる操作は順に全部届く `Stream`（1 件 64 KiB まで）。
    public enum KukuriDelivery
    {
        Datagram = 1,
        Stream = 2,
    }

    /// `SendPacket` の結果。`TooLarge` は上限を超える大きさ、`Backpressure` は stream の送り待ちが上限で受け付けなかった、
    /// `Closed` は閉じた・知らない session。
    public enum KukuriSendStatus
    {
        Sent,
        TooLarge,
        Backpressure,
        Closed,
        Faulted,
    }

    /// 受け取った packet。`Data` は呼出元のもの（SDK は保持しない）。
    public readonly struct KukuriPacket
    {
        public string SessionId { get; }
        public KukuriDelivery Delivery { get; }
        public byte[] Data { get; }

        internal KukuriPacket(string sessionId, KukuriDelivery delivery, byte[] data)
        {
            SessionId = sessionId;
            Delivery = delivery;
            Data = data;
        }
    }

    /// 視聴中の live の復号した frame（ADR 0070 §8.3、#1982）。`I420` は上の行から並んだ Y（幅×高さ）・U・V（それぞれ半分の
    /// 幅×半分の高さ）で、呼出元のもの。`AudioClockUs` はそのときの音声の再生時刻（音声が無ければ 0）。
    public readonly struct KukuriLiveFrame
    {
        public string SessionId { get; }
        public int Width { get; }
        public int Height { get; }
        public long TimestampUs { get; }
        public long AudioClockUs { get; }
        public byte[] I420 { get; }

        internal KukuriLiveFrame(JObject parsed, byte[] i420)
        {
            SessionId = (string)parsed["session_id"];
            Width = (int)parsed["width"];
            Height = (int)parsed["height"];
            TimestampUs = (long)parsed["timestamp_us"];
            AudioClockUs = (long)parsed["audio_clock_us"];
            I420 = i420;
        }
    }

    /// 要求への応答。秘密の出力（招待 token・暗号化した鍵の export）は `Secret` だけに入り、`Raw` には入らない。
    public sealed class KukuriResponse
    {
        public string RequestId { get; internal set; }
        public string Command { get; internal set; }
        public bool Ok { get; internal set; }
        public JToken Data { get; internal set; }
        public string ErrorCode { get; internal set; }
        public string ErrorMessage { get; internal set; }
        public byte[] Secret { get; internal set; }
        public JObject Raw { get; internal set; }
    }

    /// kukuri の client の handle。要求は即座に受け付け、応答と event は `Pump` が呼出元の thread（Unity の main thread）で返す。
    public sealed unsafe class KukuriClient : IDisposable
    {
        /// この C# が対応する C ABI の版。DLL の版と違えば開かない。
        public const uint AbiVersion = 2;

        /// Editor が Sidecar の mode の Open の前に呼ぶ（動いていなければ Sidecar を起動する）。Player では null。
        public static Action<string, string> EnsureSidecar;

        /// 開いている client。Dispose されないまま Editor の domain reload で domain が破棄されても、native の handle と
        /// profile の lock を残さないように閉じる（#1977 AC-2）。
        static readonly HashSet<KukuriClient> open = new HashSet<KukuriClient>();

        static KukuriClient()
        {
            AppDomain.CurrentDomain.DomainUnload += (_, _) => DisposeAll();
        }

        /// 開いている handle の数（使っていない機能が動いていないこと・閉じ忘れが無いことの確認に使う）。
        public static int OpenHandles
        {
            get
            {
                lock (open)
                    return open.Count;
            }
        }

        static void DisposeAll()
        {
            KukuriClient[] clients;
            lock (open)
                clients = new List<KukuriClient>(open).ToArray();
            foreach (var client in clients)
                client.Dispose();
        }

        IntPtr handle;
        readonly string profile;
        readonly int ownerThread;
        readonly Dictionary<string, Pending> pending = new Dictionary<string, Pending>();
        /// native から取り出して、まだ処理していない応答と event（次の frame の Pump へ持ち越す分）。
        readonly Queue<Received> backlog = new Queue<Received>();
        long nextRequest;
        volatile bool closed;
        /// audio thread の送信と解放を排他する（`SendLiveAudio` だけが owner の thread の外から呼ばれる）。
        readonly object binaryGate = new object();
        byte[] videoBuffer = Array.Empty<byte>();

        struct Received
        {
            public uint Kind;
            public byte[] Data;
            /// 応答の秘密の出力、または packet の event の bytes。
            public byte[] Secret;
        }

        sealed class Pending
        {
            public TaskCompletionSource<KukuriResponse> Completion;
            public CancellationTokenRegistration Cancellation;
        }

        /// runtime の event（`RuntimeEvent` と `events_lost`）。`Pump` の中から main thread で呼ぶ。
        public event Action<JObject> EventReceived;

        /// 参加中・受け付けた session の packet（`game_session_packet`）。`Pump` の中から main thread で呼ぶ。
        public event Action<KukuriPacket> PacketReceived;

        /// 視聴中の live の frame（`live_video_frame`。session ごとに最新の 1 枚）。`Pump` の中から main thread で呼ぶ。
        public event Action<KukuriLiveFrame> LiveFrameReceived;

        public string Profile => profile;

        KukuriClient(IntPtr handle, string profile)
        {
            this.handle = handle;
            this.profile = profile;
            ownerThread = Thread.CurrentThread.ManagedThreadId;
            lock (open)
                open.Add(this);
        }

        /// profile を開く。`profileDirectory` は絶対 path。DLL が無い・版が違うときは理由つきの `KukuriException` で、handle を作らない。
        public static KukuriClient Open(KukuriMode mode, string profile, string profileDirectory)
        {
            return Open(mode, profile, profileDirectory, AbiVersion);
        }

        internal static KukuriClient Open(KukuriMode mode, string profile, string profileDirectory, uint expectedAbi)
        {
            if (string.IsNullOrWhiteSpace(profile) || !Path.IsPathRooted(profileDirectory))
                throw new KukuriException("invalid_argument", "a profile name and an absolute profile directory are required");
            uint native;
            try
            {
                native = Native.kukuri_sdk_abi_version();
            }
            catch (DllNotFoundException)
            {
                throw new KukuriException("native_library_missing", "kukuri_game_sdk.dll was not found; build the native library (sdk/unity/build-native.ps1)");
            }
            catch (EntryPointNotFoundException)
            {
                throw new KukuriException("abi_mismatch", "kukuri_game_sdk.dll does not provide the kukuri SDK ABI");
            }
            if (native != expectedAbi)
                throw new KukuriException("abi_mismatch", $"the native library speaks ABI {native}, but this package expects {expectedAbi}; update both to the same release");
            if (mode == KukuriMode.Sidecar)
                EnsureSidecar?.Invoke(profile, profileDirectory);
            var config = JsonConvert.SerializeObject(new
            {
                profile,
                profile_dir = profileDirectory,
                mode = mode == KukuriMode.Sidecar ? "sidecar" : "ffi",
            });
            var bytes = Encoding.UTF8.GetBytes(config);
            int status;
            IntPtr opened;
            fixed (byte* data = bytes)
                status = Native.kukuri_sdk_open(expectedAbi, data, (UIntPtr)bytes.Length, out opened);
            if (status == Native.AbiMismatch)
                throw new KukuriException("abi_mismatch", "the native library refused this ABI version");
            if (status != Native.Ok || opened == IntPtr.Zero)
                throw new KukuriException(StatusCode(status), "the SDK handle could not be opened");
            return new KukuriClient(opened, profile);
        }

        /// 要求を送る（handle を開いた thread から）。結果は `Pump` が完了させる。`secret` は秘密の入力（passphrase・export・招待 token）で、JSON や log に出さない。
        public Task<KukuriResponse> SendAsync(string command, JObject payload = null, byte[] secret = null,
            bool acceptsSecretOutput = false, CancellationToken cancellation = default, int timeoutMilliseconds = 0)
        {
            RequireOwnerThread();
            if (closed)
                throw new KukuriException("closed", "the client is closed");
            var requestId = "u" + Interlocked.Increment(ref nextRequest);
            var envelope = new JObject
            {
                ["protocol_version"] = 1,
                ["request_id"] = requestId,
                ["command"] = command,
                ["profile"] = profile,
                ["payload"] = payload ?? new JObject(),
            };
            if (secret != null)
                envelope["secret_bytes"] = secret.Length;
            if (acceptsSecretOutput)
                envelope["accepts_secret_output"] = true;
            // 要求の期限（既定 30 秒、上限 5 分。ADR 0070 §3）。大きなアセットの取得などで延ばす。
            if (timeoutMilliseconds > 0)
                envelope["timeout_ms"] = timeoutMilliseconds;
            var bytes = Encoding.UTF8.GetBytes(envelope.ToString(Formatting.None));
            int status;
            fixed (byte* request = bytes)
            fixed (byte* secretBytes = secret)
                status = Native.kukuri_sdk_send(handle, request, (UIntPtr)bytes.Length, secretBytes,
                    (UIntPtr)(secret?.Length ?? 0));
            if (status != Native.Ok)
                throw new KukuriException(StatusCode(status), $"the request `{command}` was not accepted");
            var entry = new Pending { Completion = new TaskCompletionSource<KukuriResponse>() };
            pending[requestId] = entry;
            if (cancellation.CanBeCanceled)
                entry.Cancellation = cancellation.Register(() => Cancel(requestId));
            return entry.Completion.Task;
        }

        /// session の packet を送り待ちに置いて即座に戻る（network を待たない）。datagram は送り待ちが溢れたら古いものを捨てる。
        public KukuriSendStatus SendPacket(string sessionId, KukuriDelivery delivery, ReadOnlySpan<byte> data)
        {
            RequireOwnerThread();
            return closed ? KukuriSendStatus.Closed : SendBinary((uint)delivery, sessionId, data);
        }

        /// 配信中の live に、下の行から並んだ RGBA32（`AsyncGPUReadback` の並び）の frame を置く（待たない。#1982）。幅と高さは
        /// 1280×720 まで。配信を始める前と止めた後は `Closed`。SDK が圧縮し、送り待ちは 1 枚で古い frame を捨てる。
        public KukuriSendStatus SendLiveVideo(string sessionId, int width, int height, ReadOnlySpan<byte> rgba)
        {
            RequireOwnerThread();
            if (closed)
                return KukuriSendStatus.Closed;
            var length = 4 + rgba.Length;
            if (videoBuffer.Length != length)
                videoBuffer = new byte[length];
            videoBuffer[0] = (byte)width;
            videoBuffer[1] = (byte)(width >> 8);
            videoBuffer[2] = (byte)height;
            videoBuffer[3] = (byte)(height >> 8);
            rgba.CopyTo(new Span<byte>(videoBuffer, 4, rgba.Length));
            return SendBinary(Native.BinaryLiveVideo, sessionId, videoBuffer);
        }

        /// 配信中の live に、interleave したゲームの音（`OnAudioFilterRead` の sample。mic の音は混ぜてから渡す）を置く。audio
        /// thread から呼べる（待たない）。送り待ちは 200 ms 分で、溢れたら古いものから捨てる。
        public KukuriSendStatus SendLiveAudio(string sessionId, int sampleRate, int channels, ReadOnlySpan<float> samples)
        {
            var bytes = new byte[6 + samples.Length * 4];
            BitConverter.TryWriteBytes(new Span<byte>(bytes, 0, 4), sampleRate);
            BitConverter.TryWriteBytes(new Span<byte>(bytes, 4, 2), (ushort)channels);
            MemoryMarshal.AsBytes(samples).CopyTo(new Span<byte>(bytes, 6, samples.Length * 4));
            lock (binaryGate)
                return closed ? KukuriSendStatus.Closed : SendBinary(Native.BinaryLiveAudio, sessionId, bytes);
        }

        KukuriSendStatus SendBinary(uint kind, string sessionId, ReadOnlySpan<byte> data)
        {
            var target = Encoding.UTF8.GetBytes(sessionId);
            int status;
            fixed (byte* targetBytes = target)
            fixed (byte* dataBytes = data)
                status = Native.kukuri_sdk_send_binary(handle, kind, targetBytes, (UIntPtr)target.Length, dataBytes,
                    (UIntPtr)data.Length);
            switch (status)
            {
                case Native.Ok: return KukuriSendStatus.Sent;
                case Native.InvalidArgument: return KukuriSendStatus.TooLarge;
                case Native.Backpressure: return KukuriSendStatus.Backpressure;
                case Native.Closed: return KukuriSendStatus.Closed;
                default: return KukuriSendStatus.Faulted;
            }
        }

        /// 実行前の要求を取り消す（応答は `cancelled`）。実行中の要求は取り消せず、実際の結果が返る。
        void Cancel(string requestId)
        {
            if (closed)
                return;
            var bytes = Encoding.UTF8.GetBytes(requestId);
            fixed (byte* id = bytes)
                Native.kukuri_sdk_cancel(handle, id, (UIntPtr)bytes.Length);
        }

        /// 届いている応答と event を処理する（1 frame に 1 回呼ぶ）。native からの取り出しは 1 回で既定 64 件・1 MiB まで、
        /// 処理は `budgetMilliseconds` を超えたら残りを次の frame へ回す（ADR 0070 §8.5）。取り出した batch はすぐ managed の
        /// memory へ写して解放するので、callback の中で Dispose しても解放済みの memory を読まない。
        /// 呼出元は handle を開いた thread（Unity の main thread）に限る。
        public int Pump(int maxItems = 64, int maxBytes = 1 << 20, double budgetMilliseconds = 1.0)
        {
            RequireOwnerThread();
            if (closed)
                return 0;
            var clock = Stopwatch.StartNew();
            if (backlog.Count == 0)
                Receive(maxItems, maxBytes);
            var handled = 0;
            while (!closed && backlog.Count > 0)
            {
                Deliver(backlog.Dequeue(), events: true);
                handled++;
                if (clock.Elapsed.TotalMilliseconds >= budgetMilliseconds)
                    break;
            }
            return handled;
        }

        /// native の batch を 1 回取り出し、managed の memory へ写してから解放する。
        void Receive(int maxItems, int maxBytes)
        {
            var status = Native.kukuri_sdk_poll(handle, (UIntPtr)maxItems, (UIntPtr)maxBytes, out var batch);
            if (status == Native.Faulted)
            {
                FailAll("internal_error", "the SDK handle faulted");
                return;
            }
            if (status != Native.Ok || batch == null)
                return;
            try
            {
                var count = (int)batch->Count;
                for (var index = 0; index < count; index++)
                {
                    var item = batch->Items[index];
                    var received = new Received { Kind = item.Kind, Data = Copy(item) };
                    // 秘密の出力は直前の応答と、bytes は直前の packet の event と組にする（SDK は必ず直後に置く）。
                    var next = index + 1 < count ? batch->Items[index + 1].Kind : 0;
                    if ((item.Kind == Native.ItemResponse && next == Native.ItemSecret) ||
                        (item.Kind == Native.ItemEvent && next == Native.ItemBytes))
                        received.Secret = Copy(batch->Items[++index]);
                    if (item.Kind == Native.ItemResponse || item.Kind == Native.ItemEvent)
                        backlog.Enqueue(received);
                }
            }
            finally
            {
                Native.kukuri_sdk_batch_release(handle, batch);
            }
        }

        static byte[] Copy(Native.Item item)
        {
            var bytes = new byte[(int)item.Length];
            Marshal.Copy((IntPtr)item.Data, bytes, 0, bytes.Length);
            return bytes;
        }

        void Deliver(Received received, bool events)
        {
            if (received.Kind == Native.ItemEvent)
            {
                if (!events)
                    return;
                try
                {
                    var parsed = JObject.Parse(Encoding.UTF8.GetString(received.Data));
                    if (received.Secret != null && (string)parsed["type"] == "live_video_frame")
                        LiveFrameReceived?.Invoke(new KukuriLiveFrame(parsed, received.Secret));
                    else if (received.Secret != null)
                        PacketReceived?.Invoke(new KukuriPacket((string)parsed["session_id"],
                            (string)parsed["delivery"] == "stream" ? KukuriDelivery.Stream : KukuriDelivery.Datagram,
                            received.Secret));
                    else
                        EventReceived?.Invoke(parsed);
                }
                catch (Exception error)
                {
                    // 利用者の handler の例外で、残りの応答と event を失わない。
                    UnityEngine.Debug.LogException(error);
                }
                return;
            }
            Complete(JObject.Parse(Encoding.UTF8.GetString(received.Data)), received.Secret);
        }

        void Complete(JObject raw, byte[] secret)
        {
            var requestId = (string)raw["request_id"];
            if (requestId == null || !pending.TryGetValue(requestId, out var entry))
                return;
            pending.Remove(requestId);
            entry.Cancellation.Dispose();
            var error = raw["error"] as JObject;
            entry.Completion.TrySetResult(new KukuriResponse
            {
                RequestId = requestId,
                Command = (string)raw["command"],
                Ok = (bool?)raw["ok"] == true,
                Data = raw["data"],
                ErrorCode = (string)error?["code"],
                ErrorMessage = (string)error?["message"],
                Secret = secret,
                Raw = raw,
            });
        }

        void FailAll(string code, string message)
        {
            var entries = new List<Pending>(pending.Values);
            pending.Clear();
            foreach (var entry in entries)
            {
                entry.Cancellation.Dispose();
                entry.Completion.TrySetException(new KukuriException(code, message));
            }
        }

        void RequireOwnerThread()
        {
            if (Thread.CurrentThread.ManagedThreadId != ownerThread)
                throw new InvalidOperationException("the client must be used on the thread that opened it (the Unity main thread)");
        }

        /// 閉じる。未完了の要求は SDK が返す最後の結果（`interrupted`・`operation_outcome_unknown` 等）で終え、handle を解放する。
        /// 閉じた後は event を渡さない。
        public void Dispose()
        {
            if (closed)
                return;
            lock (binaryGate)
                closed = true;
            try
            {
                Native.kukuri_sdk_close(handle);
                // close の後も poll でき、未完了だった要求の結果が届く（ADR 0070 §3）。
                int before;
                do
                {
                    before = backlog.Count;
                    Receive(256, 1 << 20);
                } while (backlog.Count > before);
                while (backlog.Count > 0)
                    Deliver(backlog.Dequeue(), events: false);
                FailAll("closed", "the client was closed before the response arrived");
            }
            finally
            {
                Native.kukuri_sdk_free(handle);
                handle = IntPtr.Zero;
                lock (open)
                    open.Remove(this);
            }
        }

        static string StatusCode(int status)
        {
            switch (status)
            {
                case Native.InvalidArgument: return "invalid_argument";
                case Native.Backpressure: return "backpressure";
                case Native.Closed: return "closed";
                case Native.Faulted: return "faulted";
                case Native.AbiMismatch: return "abi_mismatch";
                default: return "unknown_status";
            }
        }
    }
}
