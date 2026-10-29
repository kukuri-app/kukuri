using System;
using System.Runtime.InteropServices;

namespace Kukuri.GameSdk
{
    /// C ABI（crates/game-sdk の lib.rs、ADR 0070 §3）の宣言。関数は network を待たない（close を除く）。
    internal static unsafe class Native
    {
        const string Library = "kukuri_game_sdk";

        internal const int Ok = 0;
        internal const int InvalidArgument = 1;
        internal const int Backpressure = 2;
        internal const int Closed = 3;
        internal const int Faulted = 4;
        internal const int AbiMismatch = 5;

        internal const uint ItemResponse = 1;
        internal const uint ItemSecret = 2;
        internal const uint ItemEvent = 3;

        [StructLayout(LayoutKind.Sequential)]
        internal struct Item
        {
            public uint Kind;
            public byte* Data;
            public UIntPtr Length;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct Batch
        {
            public Item* Items;
            public UIntPtr Count;
        }

        [DllImport(Library)]
        internal static extern uint kukuri_sdk_abi_version();

        [DllImport(Library)]
        internal static extern int kukuri_sdk_open(uint abiVersion, byte* config, UIntPtr length, out IntPtr handle);

        [DllImport(Library)]
        internal static extern int kukuri_sdk_send(IntPtr handle, byte* request, UIntPtr length, byte* secret, UIntPtr secretLength);

        [DllImport(Library)]
        internal static extern int kukuri_sdk_cancel(IntPtr handle, byte* requestId, UIntPtr length);

        [DllImport(Library)]
        internal static extern int kukuri_sdk_poll(IntPtr handle, UIntPtr maxItems, UIntPtr maxBytes, out Batch* batch);

        [DllImport(Library)]
        internal static extern int kukuri_sdk_batch_release(IntPtr handle, Batch* batch);

        [DllImport(Library)]
        internal static extern int kukuri_sdk_close(IntPtr handle);

        [DllImport(Library)]
        internal static extern void kukuri_sdk_free(IntPtr handle);
    }
}
