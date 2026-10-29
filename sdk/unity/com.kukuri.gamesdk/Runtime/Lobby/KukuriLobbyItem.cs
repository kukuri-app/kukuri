using System;
using UnityEngine;
using UnityEngine.UI;

namespace Kukuri.GameSdk.Lobby
{
    /// ロビーの一覧の 1 行。見た目は prefab で差し替えられる。
    public sealed class KukuriLobbyItem : MonoBehaviour
    {
        [SerializeField] Text titleText;
        [SerializeField] Text detailText;
        [SerializeField] Button endButton;

        string roomId;
        Action<string> end;

        void Awake()
        {
            if (endButton != null)
                endButton.onClick.AddListener(() => end?.Invoke(roomId));
        }

        public string Title => titleText != null ? titleText.text : "";
        public string Detail => detailText != null ? detailText.text : "";

        internal void Show(LobbyRoom room, Action<string> onEnd)
        {
            roomId = room.RoomId;
            end = onEnd;
            if (titleText != null)
                titleText.text = room.Title;
            if (detailText != null)
                detailText.text = $"{StatusLabel(room.Status)}・最大 {room.MaxPlayers} 人";
            if (endButton != null)
                endButton.gameObject.SetActive(room.IsMine && room.Status != "Ended");
        }

        static string StatusLabel(string status) => status switch
        {
            "Waiting" => "待機中",
            "Running" => "進行中",
            "Paused" => "中断中",
            "Ended" => "終了",
            _ => status,
        };
    }
}
