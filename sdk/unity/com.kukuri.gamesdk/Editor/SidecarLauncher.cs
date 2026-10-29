using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using UnityEditor;
using Debug = UnityEngine.Debug;

namespace Kukuri.GameSdk.Editor
{
    /// Sidecar の mode の Open の前に、Sidecar が動いていなければ起動する。Play を止めても動かし続け、Editor の終了か
    /// menu の「Sidecar を止める」で止める（2026-10-30 のユーザー判断、ADR 0070 §8.2）。Player の build には入らない。
    [InitializeOnLoad]
    public static class SidecarLauncher
    {
        const string StartedKey = "Kukuri.GameSdk.SidecarProfiles";
        const int ReadyTimeoutMilliseconds = 30_000;

        static SidecarLauncher()
        {
            KukuriClient.EnsureSidecar = Ensure;
            EditorApplication.quitting += StopAll;
        }

        /// package の中の `Editor/Sidecar~/kukuri-sdk-sidecar.exe`（sdk/unity/build-native.ps1 が置く）。
        public static string ExecutablePath
        {
            get
            {
                var package = UnityEditor.PackageManager.PackageInfo.FindForAssembly(typeof(SidecarLauncher).Assembly);
                return Path.Combine(package.resolvedPath, "Editor", "Sidecar~", "kukuri-sdk-sidecar.exe");
            }
        }

        /// 動いていなければ起動して `ready` を待つ。既に動いていれば、二つ目は pipe を作れずに終わる（終了 code 3）。
        public static void Ensure(string profile, string profileDirectory)
        {
            var exe = ExecutablePath;
            if (!File.Exists(exe))
                throw new KukuriException("sidecar_missing", $"{exe} was not found; build it with sdk/unity/build-native.ps1");
            var process = Process.Start(new ProcessStartInfo(exe, Arguments("run", profile, profileDirectory))
            {
                UseShellExecute = false,
                CreateNoWindow = true,
                RedirectStandardError = true,
            });
            var ready = process.StandardError.ReadLineAsync();
            if (!ready.Wait(ReadyTimeoutMilliseconds) && !process.HasExited)
                throw new KukuriException("daemon_unavailable", "the sidecar did not become ready");
            if (ready.Result != "ready")
            {
                process.WaitForExit(ReadyTimeoutMilliseconds);
                if (process.ExitCode != 3)
                    throw new KukuriException("daemon_unavailable", $"the sidecar exited with code {process.ExitCode}");
                return;
            }
            // ready の後に Sidecar が stderr へ書くのは終わるときの 1 行だけなので、読み続けなくても buffer は溢れない。
            Remember(profile, profileDirectory);
        }

        [MenuItem("kukuri/Sidecar を止める")]
        public static void StopAll()
        {
            foreach (var entry in Remembered())
            {
                var parts = entry.Split('\n');
                using var stop = Process.Start(new ProcessStartInfo(ExecutablePath, Arguments("stop", parts[0], parts[1]))
                {
                    UseShellExecute = false,
                    CreateNoWindow = true,
                });
                stop?.WaitForExit(10_000);
            }
            SessionState.EraseString(StartedKey);
        }

        static string Arguments(string command, string profile, string profileDirectory) =>
            $"{command} --profile \"{profile}\" --profile-dir \"{profileDirectory}\"";

        // domain reload をまたいで、この Editor の session で起動した Sidecar を覚えておく。
        static IEnumerable<string> Remembered()
        {
            var value = SessionState.GetString(StartedKey, "");
            return value.Length == 0 ? new string[0] : value.Split('\t');
        }

        static void Remember(string profile, string profileDirectory)
        {
            var entries = new List<string>(Remembered());
            var entry = profile + "\n" + profileDirectory;
            if (!entries.Contains(entry))
                entries.Add(entry);
            SessionState.SetString(StartedKey, string.Join("\t", entries));
        }
    }
}
