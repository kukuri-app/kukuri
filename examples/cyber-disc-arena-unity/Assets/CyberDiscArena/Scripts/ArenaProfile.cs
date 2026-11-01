using System.Collections.Generic;
using System.IO;
using Kukuri.GameSdk;
using Newtonsoft.Json;
using UnityEngine;

namespace CyberDiscArena
{
    /// game player ごとのゲームの状態（選択中のスキン）。kukuri の profile の directory の `cyber-disc-arena.json` に、
    /// game player ID をキーにして置く（SDK はセーブを持たない。ADR 0070 §4.2）。同じ鍵で引き継いでも、今の進行を残して
    /// まとめても game player ID は変わらないので、状態はそのまま続く。まとめる先のアカウントにこの端末で遊んだ進行があれば、
    /// 確定の画面で残す進行として選べる（選ばなかった進行もここに残る）。Steam で配るときは、この file も Steam Auto-Cloud の
    /// 対象にする。
    public sealed class ArenaProfile : MonoBehaviour
    {
        public const string StandardSkin = "standard";

        [SerializeField] KukuriRunner runner;
        [SerializeField] KukuriAccountScreen account;

        sealed class Entry
        {
            public string Pubkey = "";
            public string Skin = StandardSkin;
        }

        Dictionary<string, Entry> players = new Dictionary<string, Entry>();
        string loaded;

        /// 今の game player ID（開始する前は空）。
        public string GamePlayerId => account.GamePlayerId;

        /// 選択中のスキン。変えると保存する。
        public string Skin
        {
            get => Current?.Skin ?? StandardSkin;
            set
            {
                if (Current == null)
                    return;
                Current.Skin = value;
                Save();
            }
        }

        /// 今の game player の状態を読み終えた（開始の後、1 frame ほどで true）。
        public bool Ready => Current != null && Current.Pubkey == account.PublicKey;

        Entry Current => GamePlayerId.Length > 0 && players.TryGetValue(GamePlayerId, out var entry) ? entry : null;

        string FilePath => Path.Combine(runner.Settings.ProfileDirectory, "cyber-disc-arena.json");

        void OnEnable() => account.OtherProgress = OtherProgress;

        void Update()
        {
            if (account.Screen != "playing" || (loaded == GamePlayerId && Current?.Pubkey == account.PublicKey))
                return;
            if (loaded == null && File.Exists(FilePath))
                players = JsonConvert.DeserializeObject<Dictionary<string, Entry>>(File.ReadAllText(FilePath)) ?? players;
            loaded = GamePlayerId;
            if (!players.TryGetValue(GamePlayerId, out var entry))
                players[GamePlayerId] = entry = new Entry();
            entry.Pubkey = account.PublicKey;
            Save();
        }

        /// まとめる先のアカウントで、この端末に残っている別の進行。
        (string id, string label)? OtherProgress(string pubkey)
        {
            foreach (var (id, entry) in players)
                if (id != GamePlayerId && entry.Pubkey == pubkey)
                    return (id, $"スキン {entry.Skin}");
            return null;
        }

        void Save()
        {
            var temporary = FilePath + ".tmp";
            File.WriteAllText(temporary, JsonConvert.SerializeObject(players, Formatting.Indented));
            File.Copy(temporary, FilePath, true);
            File.Delete(temporary);
        }
    }
}
