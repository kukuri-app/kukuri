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
        public const uint AbiVersion = 1;

        /// Editor が Sidecar の mode の Open の前に呼ぶ（動いていなければ Sidecar を起動する）。Player では null。
        public static Action<string, string> EnsureSidecar;

        static int openHandles;

        /// 開いている handle の数（使っていない機能が動いていないことの確認に使う）。
        public static int OpenHandles => Volatile.Read(ref openHandles);

        IntPtr handle;
        readonly string profile;
        readonly int ownerThread;
        readonly Dictionary<string, Pending> pending = new Dictionary<string, Pending>();
        /// native から取り出して、まだ処理していない応答と event（次の frame の Pump へ持ち越す分）。
        readonly Queue<Received> backlog = new Queue<Received>();
        long nextRequest;
        bool closed;

        struct Received
        {
            public uint Kind;
            public byte[] Data;
            public byte[] Secret;
        }

        sealed class Pending
        {
            public TaskCompletionSource<KukuriResponse> Completion;
            public CancellationTokenRegistration Cancellation;
        }

        /// runtime の event（`RuntimeEvent` と `events_lost`）。`Pump` の中から main thread で呼ぶ。
        public event Action<JObject> EventReceived;

        public string Profile => profile;

        KukuriClient(IntPtr handle, string profile)
        {
            this.handle = handle;
            this.profile = profile;
            ownerThread = Thread.CurrentThread.ManagedThreadId;
            Interlocked.Increment(ref openHandles);
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
            bool acceptsSecretOutput = false, CancellationToken cancellation = default)
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
                    // 秘密の出力は直前の応答と組にする（SDK は必ず応答の直後に置く）。
                    if (item.Kind == Native.ItemResponse && index + 1 < count &&
                        batch->Items[index + 1].Kind == Native.ItemSecret)
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
                if (!events || EventReceived == null)
                    return;
                try
                {
                    EventReceived(JObject.Parse(Encoding.UTF8.GetString(received.Data)));
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
                Interlocked.Decrement(ref openHandles);
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
