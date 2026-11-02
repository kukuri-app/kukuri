using CyberDiscArena;
using Kukuri.GameSdk;
using Kukuri.GameSdk.Lobby;
using UnityEditor;
using UnityEditor.Build;
using UnityEditor.SceneManagement;
using UnityEngine;

/// 計測用の Player を build する（slo.ps1 が `-executeMethod` で呼ぶ）。Arena と ArenaStreamViewer の最小の scene に probe を
/// 置き、空の Boot から役の scene へ移る（NGO の NetworkManager は DontDestroyOnLoad なので、Arena を経由しない）。
public static class SloProbeBuild
{
    static void Link(Object component, string field, Object value)
    {
        var serialized = new SerializedObject(component);
        serialized.FindProperty(field).objectReferenceValue = value;
        serialized.ApplyModifiedPropertiesWithoutUndo();
    }

    static string Probe(string source, string name)
    {
        var scene = EditorSceneManager.OpenScene(source);
        var runner = Object.FindAnyObjectByType<KukuriRunner>();
        runner.enabled = false;
        var probe = new GameObject("slo-probe").AddComponent<SloProbe>();
        var broadcast = Object.FindAnyObjectByType<ArenaBroadcast>();
        Link(probe, "runner", runner);
        Link(probe, "lobby", Object.FindAnyObjectByType<KukuriLobby>());
        Link(probe, "session", Object.FindAnyObjectByType<ArenaSession>());
        Link(probe, "account", Object.FindAnyObjectByType<KukuriAccountScreen>());
        Link(probe, "skins", Object.FindAnyObjectByType<ArenaSkins>());
        Link(probe, "broadcast", broadcast);
        Link(probe, "feed", new SerializedObject(broadcast).FindProperty("feed").objectReferenceValue);
        var path = $"Assets/SloProbe/{name}.unity";
        EditorSceneManager.SaveScene(scene, path);
        return path;
    }

    public static void Build()
    {
        var arena = Probe("Assets/CyberDiscArena/Scenes/Arena.unity", "SloArena");
        var viewer = Probe("Assets/CyberDiscArena/Scenes/Minimal/ArenaStreamViewer.unity", "ArenaStreamViewer");
        var boot = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);
        new GameObject("slo-probe").AddComponent<SloProbe>();
        EditorSceneManager.SaveScene(boot, "Assets/SloProbe/Boot.unity");
        PlayerSettings.SetScriptingBackend(NamedBuildTarget.Standalone, ScriptingImplementation.__BACKEND__);
        var report = BuildPipeline.BuildPlayer(new BuildPlayerOptions
        {
            scenes = new[] { "Assets/SloProbe/Boot.unity", arena, viewer },
            locationPathName = "Builds/__BACKEND__/CyberDiscArena.exe",
            target = BuildTarget.StandaloneWindows64,
        });
        Debug.Log("KGS_BUILD " + report.summary.result);
        EditorApplication.Exit(report.summary.result == UnityEditor.Build.Reporting.BuildResult.Succeeded ? 0 : 1);
    }
}
