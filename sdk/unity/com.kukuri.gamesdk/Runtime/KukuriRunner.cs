using UnityEngine;

namespace Kukuri.GameSdk
{
    /// scene に置くと `settings` の profile を開き、毎 frame の Update で応答と event を main thread へ渡す。
    /// 置かなければ SDK は何も起動しない。
    public sealed class KukuriRunner : MonoBehaviour
    {
        [SerializeField] KukuriSettings settings;

        public KukuriClient Client { get; private set; }

        /// 開けなかった理由（DLL が無い・版が違う等）。開けたら null。
        public KukuriException OpenError { get; private set; }

        public KukuriSettings Settings
        {
            get => settings;
            set => settings = value;
        }

        void OnEnable()
        {
            if (settings == null || Client != null)
                return;
            try
            {
                Client = KukuriClient.Open(settings.Mode, settings.Profile, settings.ProfileDirectory);
                OpenError = null;
            }
            catch (KukuriException error)
            {
                OpenError = error;
                Debug.LogWarning($"kukuri: the SDK did not start ({error.Code}): {error.Message}");
            }
        }

        void Update()
        {
            Client?.Pump();
        }

        void OnDisable()
        {
            Client?.Dispose();
            Client = null;
        }
    }
}
