using System.IO;
using UnityEngine;

namespace Kukuri.GameSdk
{
    /// 接続の設定（mode と profile）。秘密の値は持たない（ADR 0070 §4）。
    [CreateAssetMenu(fileName = "KukuriSettings", menuName = "kukuri/Settings")]
    public sealed class KukuriSettings : ScriptableObject
    {
        [Tooltip("本番の Player の既定は FFI。Sidecar は Editor での開発用で、Editor が Play の開始時に起動する")]
        [SerializeField] KukuriMode mode = KukuriMode.Ffi;

        [Tooltip("profile の名前。profile ごとに別のアカウント・DB を持つ")]
        [SerializeField] string profile = "default";

        [Tooltip("空なら Application.persistentDataPath/kukuri/profiles/<profile>")]
        [SerializeField] string profileDirectory = "";

        public KukuriMode Mode => mode;
        public string Profile => profile;

        public string ProfileDirectory => string.IsNullOrEmpty(profileDirectory)
            ? Path.GetFullPath(Path.Combine(Application.persistentDataPath, "kukuri", "profiles", profile))
            : Path.GetFullPath(profileDirectory);

        public static KukuriSettings Create(KukuriMode mode, string profile, string profileDirectory)
        {
            var settings = CreateInstance<KukuriSettings>();
            settings.mode = mode;
            settings.profile = profile;
            settings.profileDirectory = profileDirectory;
            return settings;
        }
    }
}
