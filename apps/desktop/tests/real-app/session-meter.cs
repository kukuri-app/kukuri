// Windows の既定の出力で、指定した process の音声 session の peak を 20 ms ごとに標準出力へ書く(#1800)。
// PowerShell の Add-Type で読み込み、[SessionMeter]::Run(pid) で動かす。
using System;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Threading;

public static class SessionMeter
{
    [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
    class DeviceEnumerator { }

    [Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IMMDeviceEnumerator
    {
        int EnumAudioEndpoints(int flow, int mask, out IntPtr devices);
        int GetDefaultAudioEndpoint(int flow, int role, out IMMDevice device);
    }

    [Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IMMDevice
    {
        int Activate(ref Guid iid, int context, IntPtr parameters, [MarshalAs(UnmanagedType.IUnknown)] out object value);
    }

    [Guid("77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IAudioSessionManager2
    {
        int GetAudioSessionControl(IntPtr session, int flags, out IntPtr control);
        int GetSimpleAudioVolume(IntPtr session, int flags, out IntPtr volume);
        int GetSessionEnumerator(out IAudioSessionEnumerator sessions);
    }

    [Guid("E2F5BB11-0570-40CA-ACDD-3AA01277DEE8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IAudioSessionEnumerator
    {
        int GetCount(out int count);
        int GetSession(int index, out IAudioSessionControl2 session);
    }

    [Guid("BFB7FF88-7239-4FC9-8FA2-07C950BE9C6D"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IAudioSessionControl2
    {
        int GetState(out int state);
        int GetDisplayName(out IntPtr name);
        int SetDisplayName(IntPtr name, IntPtr context);
        int GetIconPath(out IntPtr path);
        int SetIconPath(IntPtr path, IntPtr context);
        int GetGroupingParam(out Guid grouping);
        int SetGroupingParam(ref Guid grouping, IntPtr context);
        int RegisterAudioSessionNotification(IntPtr client);
        int UnregisterAudioSessionNotification(IntPtr client);
        int GetSessionIdentifier(out IntPtr id);
        int GetSessionInstanceIdentifier(out IntPtr id);
        int GetProcessId(out uint pid);
    }

    [Guid("C02216F6-8C67-4B5B-9D00-D008E73E0064"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IAudioMeterInformation
    {
        int GetPeakValue(out float peak);
    }

    public static void Run(uint pid)
    {
        IMMDevice device;
        ((IMMDeviceEnumerator)new DeviceEnumerator()).GetDefaultAudioEndpoint(0, 0, out device);
        Guid iid = typeof(IAudioSessionManager2).GUID;
        object manager;
        device.Activate(ref iid, 23, IntPtr.Zero, out manager);
        for (;;)
        {
            float peak = 0;
            IAudioSessionEnumerator sessions;
            ((IAudioSessionManager2)manager).GetSessionEnumerator(out sessions);
            int count;
            sessions.GetCount(out count);
            for (int index = 0; index < count; index++)
            {
                IAudioSessionControl2 session;
                sessions.GetSession(index, out session);
                uint owner;
                session.GetProcessId(out owner);
                if (owner != pid) continue;
                float value;
                ((IAudioMeterInformation)session).GetPeakValue(out value);
                peak = Math.Max(peak, value);
            }
            Console.WriteLine(peak.ToString(CultureInfo.InvariantCulture));
            Thread.Sleep(20);
        }
    }
}
