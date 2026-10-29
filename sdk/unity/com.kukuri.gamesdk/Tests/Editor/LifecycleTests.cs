using System;
using System.Collections;
using System.Diagnostics;
using System.IO;
using System.Threading;
using Kukuri.GameSdk.Editor;
using NUnit.Framework;
using UnityEditor;
using UnityEngine;
using UnityEngine.SceneManagement;
using UnityEngine.TestTools;

namespace Kukuri.GameSdk.Tests
{
    /// Editor の Play・Stop（domain reload の有無）、scene の変更、取消と Dispose で、handle・task・購読が溜まらないこと（#1977 AC-2）。
    public sealed class LifecycleTests
    {
        const string SettingsPath = "Assets/KukuriLifecycleTestSettings.asset";

        // domain reload をまたぐと試験の local 変数は失われるので、残す値は SessionState に置く。
        const string DirectoryKey = "Kukuri.GameSdk.Tests.ProfileDirectory";
        const string ModeKey = "Kukuri.GameSdk.Tests.Mode";
        const string CycleKey = "Kukuri.GameSdk.Tests.Cycle";

        static string ProfileDirectory => SessionState.GetString(DirectoryKey, "");
        static KukuriMode Mode => (KukuriMode)SessionState.GetInt(ModeKey, 0);

        [TearDown]
        public void TearDown()
        {
            EditorSettings.enterPlayModeOptionsEnabled = false;
            AssetDatabase.DeleteAsset(SettingsPath);
            SidecarLauncher.StopAll();
        }

        static void NewProfile(KukuriMode mode)
        {
            SessionState.SetString(DirectoryKey,
                Path.Combine(Path.GetTempPath(), "kgs-" + Guid.NewGuid().ToString("N").Substring(0, 12)));
            SessionState.SetInt(ModeKey, (int)mode);
            SessionState.SetInt(CycleKey, 0);
        }

        /// 同じ profile を開いて要求が通る（前の handle が profile の lock を手放している）。
        static IEnumerator AssertProfileIsFree()
        {
            var client = KukuriClient.Open(Mode, "p", ProfileDirectory);
            try
            {
                var request = client.SendAsync("client.status");
                var clock = Stopwatch.StartNew();
                while (!request.IsCompleted)
                {
                    Assert.Less(clock.Elapsed.TotalSeconds, 30, "no response");
                    client.Pump();
                    yield return null;
                }
                Assert.IsTrue(request.Result.Ok, request.Result.ErrorCode);
            }
            finally
            {
                client.Dispose();
            }
        }

        static IEnumerator WaitForRunnerResponse()
        {
            var clock = Stopwatch.StartNew();
            KukuriRunner runner = null;
            while (runner == null || runner.Client == null)
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 30, "the runner did not open the client");
                runner = UnityEngine.Object.FindFirstObjectByType<KukuriRunner>();
                yield return null;
            }
            var request = runner.Client.SendAsync("get_desktop_startup_status");
            while (!request.IsCompleted)
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 30, "no response");
                yield return null;
            }
            Assert.IsTrue(request.Result.Ok, request.Result.ErrorCode);
        }

        static void CreateRunner()
        {
            AssetDatabase.CreateAsset(KukuriSettings.Create(Mode, "p", ProfileDirectory), SettingsPath);
            var host = new GameObject("kukuri");
            var runner = host.AddComponent<KukuriRunner>();
            runner.Settings = AssetDatabase.LoadAssetAtPath<KukuriSettings>(SettingsPath);
            // Play の中で足したときは OnEnable の後に設定したので、開き直す。
            runner.enabled = false;
            runner.enabled = true;
        }

        [UnityTest]
        public IEnumerator PlayAndStopWithFfiAndDomainReload() => PlayAndStopCycles(KukuriMode.Ffi, true);

        [UnityTest]
        public IEnumerator PlayAndStopWithFfiWithoutDomainReload() => PlayAndStopCycles(KukuriMode.Ffi, false);

        [UnityTest]
        public IEnumerator PlayAndStopWithTheSidecarAndDomainReload() => PlayAndStopCycles(KukuriMode.Sidecar, true);

        [UnityTest]
        public IEnumerator PlayAndStopWithTheSidecarWithoutDomainReload() => PlayAndStopCycles(KukuriMode.Sidecar, false);

        /// Play と Stop を 3 回くり返し、Stop のたびに handle が 0 に戻り、同じ profile をすぐ開き直せる。
        static IEnumerator PlayAndStopCycles(KukuriMode mode, bool reload)
        {
            NewProfile(mode);
            EditorSettings.enterPlayModeOptionsEnabled = true;
            EditorSettings.enterPlayModeOptions = reload ? EnterPlayModeOptions.None : EnterPlayModeOptions.DisableDomainReload;
            CreateRunner();
            while (SessionState.GetInt(CycleKey, 0) < 3)
            {
                yield return new EnterPlayMode();
                yield return WaitForRunnerResponse();
                yield return new ExitPlayMode();
                Assert.AreEqual(0, KukuriClient.OpenHandles,
                    $"cycle {SessionState.GetInt(CycleKey, 0)}: the client is closed when Play stops");
                yield return AssertProfileIsFree();
                SessionState.SetInt(CycleKey, SessionState.GetInt(CycleKey, 0) + 1);
            }
            var runner = UnityEngine.Object.FindFirstObjectByType<KukuriRunner>();
            if (runner != null)
                UnityEngine.Object.DestroyImmediate(runner.gameObject);
            if (Mode == KukuriMode.Sidecar)
                Assert.AreEqual(1, Process.GetProcessesByName("kukuri-sdk-sidecar").Length,
                    "one sidecar keeps running across Play and Stop");
        }

        [UnityTest]
        public IEnumerator AClientLeftOpenIsClosedByADomainReload()
        {
            NewProfile(KukuriMode.Ffi);
            EditorSettings.enterPlayModeOptionsEnabled = true;
            EditorSettings.enterPlayModeOptions = EnterPlayModeOptions.None;
            // Dispose を呼ばずに残す（利用者の閉じ忘れ）。
            KukuriClient.Open(KukuriMode.Ffi, "p", ProfileDirectory);
            yield return new EnterPlayMode();
            // domain の破棄で閉じたので、新しい domain から同じ profile を開ける。
            Assert.AreEqual(0, KukuriClient.OpenHandles);
            yield return AssertProfileIsFree();
            yield return new ExitPlayMode();
        }

        [UnityTest]
        public IEnumerator ChangingTheSceneClosesTheRunnersClient()
        {
            NewProfile(KukuriMode.Ffi);
            yield return new EnterPlayMode();
            CreateRunner();
            yield return WaitForRunnerResponse();
            Assert.AreEqual(1, KukuriClient.OpenHandles);
            var previous = SceneManager.GetActiveScene();
            var next = SceneManager.CreateScene("kukuri-next");
            SceneManager.SetActiveScene(next);
            yield return SceneManager.UnloadSceneAsync(previous);
            Assert.AreEqual(0, KukuriClient.OpenHandles, "the runner closes its client with the scene");
            yield return AssertProfileIsFree();
            yield return new ExitPlayMode();
        }

        [UnityTest]
        public IEnumerator CancelledAndOutstandingRequestsAlwaysEnd()
        {
            NewProfile(KukuriMode.Ffi);
            var client = KukuriClient.Open(KukuriMode.Ffi, "p", ProfileDirectory);
            using var cancellation = new CancellationTokenSource();
            var cancelled = client.SendAsync("get_desktop_startup_status", cancellation: cancellation.Token);
            cancellation.Cancel();
            var outstanding = client.SendAsync("get_desktop_startup_status");
            var clock = Stopwatch.StartNew();
            while (!cancelled.IsCompleted)
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 30, "the cancelled request did not end");
                client.Pump();
                yield return null;
            }
            // 実行前なら cancelled、実行を始めていれば実際の結果（取り消せない）。
            Assert.IsTrue(cancelled.Result.Ok || cancelled.Result.ErrorCode == "cancelled", cancelled.Result.ErrorCode);
            client.Dispose();
            Assert.IsTrue(outstanding.IsCompleted, "Dispose ends the outstanding request");
            Assert.AreEqual(0, KukuriClient.OpenHandles);
        }
    }
}
