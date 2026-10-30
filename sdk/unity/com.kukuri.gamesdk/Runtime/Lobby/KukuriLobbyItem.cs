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
        [SerializeField] Button joinButton;
        [SerializeField] Button leaveButton;
        [SerializeField] Button endButton;

        string roomId;
        Action<string> join;
        Action leave;
        Action<string> end;

        void Awake()
        {
            if (joinButton != null)
                joinButton.onClick.AddListener(() => join?.Invoke(roomId));
            if (leaveButton != null)
                leaveButton.onClick.AddListener(() => leave?.Invoke());
            if (endButton != null)
                endButton.onClick.AddListener(() => end?.Invoke(roomId));
        }

        public string Title => titleText != null ? titleText.text : "";
        public string Detail => detailText != null ? detailText.text : "";

        internal void Show(LobbyRoom room, Action<string> onJoin, Action onLeave, Action<string> onEnd)
        {
            roomId = room.RoomId;
            (join, leave, end) = (onJoin, onLeave, onEnd);
            var open = room.Status != "Ended";
            if (titleText != null)
                titleText.text = room.Title;
            if (detailText != null)
                detailText.text = $"{StatusLabel(room.Status)}・最大 {room.MaxPlayers} 人" + (room.IsJoined ? "・参加中" : "");
            Toggle(joinButton, open && !room.IsMine && !room.IsJoined);
            Toggle(leaveButton, room.IsJoined);
            Toggle(endButton, open && room.IsMine);
        }

        static void Toggle(Button button, bool visible)
        {
            if (button != null)
                button.gameObject.SetActive(visible);
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
