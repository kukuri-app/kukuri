using System;
using System.Collections;
using System.Diagnostics;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using Kukuri.GameSdk.Editor;
using Newtonsoft.Json.Linq;
using NUnit.Framework;
using UnityEngine;
using UnityEngine.TestTools;

namespace Kukuri.GameSdk.Tests
{
    /// 実際の kukuri_game_sdk.dll を C# から読み込み、FFI と Sidecar の両方で同じ要求を流す（#1977 AC-1）。
    public sealed class ClientTests
    {
        string profileDirectory;
        int mainThread;

        [SetUp]
        public void SetUp()
        {
            // 同意の前の profile だけを使う。runtime（node・keyring）は組み立てない。
            profileDirectory = Path.Combine(Path.GetTempPath(), "kgs-" + Guid.NewGuid().ToString("N").Substring(0, 12));
            mainThread = Thread.CurrentThread.ManagedThreadId;
        }

        [TearDown]
        public void TearDown()
        {
            SidecarLauncher.StopAll();
        }

        [Test]
        public void TheNativeLibraryLoadsAndSpeaksTheSameAbi()
        {
            Assert.AreEqual(KukuriClient.AbiVersion, Native.kukuri_sdk_abi_version());
        }

        [Test]
        public void ADifferentAbiIsRefusedWithAReasonAndNothingStarts()
        {
            var before = KukuriClient.OpenHandles;
            var error = Assert.Throws<KukuriException>(() =>
                KukuriClient.Open(KukuriMode.Ffi, "p", profileDirectory, KukuriClient.AbiVersion + 1));
            Assert.AreEqual("abi_mismatch", error.Code);
            StringAssert.Contains("expects", error.Message);
            Assert.AreEqual(before, KukuriClient.OpenHandles);
            Assert.IsFalse(Directory.Exists(profileDirectory), "no profile is created");
        }

        [Test]
        public void ARunnerWithoutSettingsAndAnEmptySceneStartNothing()
        {
            var before = KukuriClient.OpenHandles;
            var gameObject = new GameObject("runner");
            var runner = gameObject.AddComponent<KukuriRunner>();
            Assert.IsNull(runner.Client);
            UnityEngine.Object.DestroyImmediate(gameObject);
            Assert.AreEqual(before, KukuriClient.OpenHandles);
        }

        [UnityTest]
        public IEnumerator AStartupStatusRequestCompletesOnTheMainThreadWithFfi()
        {
            return RequestRoundTrip(KukuriMode.Ffi);
        }

        [UnityTest]
        public IEnumerator AStartupStatusRequestCompletesOnTheMainThreadWithTheSidecar()
        {
            return RequestRoundTrip(KukuriMode.Sidecar);
        }

        IEnumerator RequestRoundTrip(KukuriMode mode)
        {
            var client = KukuriClient.Open(mode, "p", profileDirectory);
            try
            {
                var eventThreads = new System.Collections.Generic.List<int>();
                client.EventReceived += _ => eventThreads.Add(Thread.CurrentThread.ManagedThreadId);
                var completedOn = -1;
                var request = client.SendAsync("get_desktop_startup_status");
                request.ContinueWith(_ => completedOn = Thread.CurrentThread.ManagedThreadId,
                    TaskContinuationOptions.ExecuteSynchronously);
                var clock = Stopwatch.StartNew();
                while (!request.IsCompleted)
                {
                    Assert.Less(clock.Elapsed.TotalSeconds, 30, "no response");
                    client.Pump();
                    yield return null;
                }
                var response = request.Result;
                Assert.IsTrue(response.Ok, response.ErrorCode);
                Assert.AreEqual("consent_required", (string)response.Data["status"]);
                Assert.AreEqual(mainThread, completedOn, "the response completes on the main thread");
                foreach (var thread in eventThreads)
                    Assert.AreEqual(mainThread, thread, "events arrive on the main thread");
                if (mode == KukuriMode.Ffi)
                    Assert.IsNotEmpty(eventThreads, "the startup notice arrives as an event");
            }
            finally
            {
                client.Dispose();
            }
        }

        [UnityTest]
        public IEnumerator AnUnknownCommandIsATypedErrorAndDisposeEndsOutstandingRequests()
        {
            var client = KukuriClient.Open(KukuriMode.Ffi, "p", profileDirectory);
            var unknown = client.SendAsync("no.such.command");
            var clock = Stopwatch.StartNew();
            while (!unknown.IsCompleted)
            {
                Assert.Less(clock.Elapsed.TotalSeconds, 30, "no response");
                client.Pump();
                yield return null;
            }
            Assert.IsFalse(unknown.Result.Ok);
            Assert.AreEqual("not_found", unknown.Result.ErrorCode);
            client.Dispose();
            var error = Assert.Throws<KukuriException>(() => client.SendAsync("client.status"));
            Assert.AreEqual("closed", error.Code);
        }
    }
}
