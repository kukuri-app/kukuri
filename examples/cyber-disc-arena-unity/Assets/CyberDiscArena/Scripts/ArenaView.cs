using UnityEngine;
using UnityEngine.EventSystems;
using UnityEngine.UI;

namespace CyberDiscArena
{
    /// キーボードの入力。WASD で移動（向きは常に相手）、Space でジャンプ、Ctrl でしゃがむ（走りながらなら slide）、
    /// E で投げる・受け止める。文字を入力している間は対戦の操作に使わない。
    public static class ArenaControls
    {
        public static ArenaInput Read(ArenaSession session)
        {
            var input = new ArenaInput { Yaw = FacingYaw(session) };
            if (Typing())
                return input;
            input.Move = new Vector2(Axis(KeyCode.D, KeyCode.A), Axis(KeyCode.W, KeyCode.S));
            input.Jump = Input.GetKey(KeyCode.Space);
            input.Crouch = Input.GetKey(KeyCode.LeftControl) || Input.GetKey(KeyCode.RightControl);
            return input;
        }

        static float Axis(KeyCode positive, KeyCode negative) =>
            (Input.GetKey(positive) ? 1 : 0) - (Input.GetKey(negative) ? 1 : 0);

        public static bool Typing()
        {
            var selected = EventSystem.current != null ? EventSystem.current.currentSelectedGameObject : null;
            return selected != null && selected.GetComponent<InputField>() != null;
        }

        /// 自分から相手への向き（度）。
        public static float FacingYaw(ArenaSession session)
        {
            var me = session.Players[session.Local < 0 ? 0 : session.Local].Position;
            var other = session.Players[session.Local == 1 ? 0 : 1].Position;
            return Mathf.Atan2(other.x - me.x, other.z - me.z) * Mathf.Rad2Deg;
        }
    }

    /// 対戦の表示。2 人とディスクを置き、camera を自分の後ろに付け、得点・残り時間・経路・RTT・知らせを出す。対戦して
    /// いない間はロビーを出す。
    public sealed class ArenaView : MonoBehaviour
    {
        [SerializeField] ArenaSession session;
        [SerializeField] Canvas lobbyCanvas;
        [SerializeField] Transform[] players = new Transform[2];
        [SerializeField] Transform disc;
        [SerializeField] Transform view;

        Vector3 remote;
        GUIStyle label;
        GUIStyle big;

        void Update()
        {
            if (session.InMatch && session.InputSource == null && Input.GetKeyDown(KeyCode.E) && !ArenaControls.Typing())
                session.Act();
        }

        void LateUpdate()
        {
            lobbyCanvas.enabled = !session.InMatch;
            var shown = new Vector3[2];
            for (var index = 0; index < 2; index++)
            {
                var state = session.Players[index];
                var position = state.Position;
                if (index == session.Local && !session.IsHost)
                    position += session.Correction;
                if (index != session.Local && !session.IsHost)
                    position = remote = Vector3.Lerp(remote, position, 1 - Mathf.Exp(-15 * Time.deltaTime));
                shown[index] = position;
                var body = players[index];
                body.position = position + Vector3.up * state.Height / 2;
                body.localScale = new Vector3(0.9f, state.Height / 2, 0.9f);
                body.rotation = Quaternion.Euler(0, state.Yaw, 0);
            }
            var held = session.Holder;
            disc.position = held >= 0
                ? shown[held] + session.Players[held].Hand - session.Players[held].Position
                : session.Disc.Position;
            disc.Rotate(0, 720 * Time.deltaTime, 0, Space.Self);
            if (!session.InMatch)
            {
                view.SetPositionAndRotation(new Vector3(0, 13, -17), Quaternion.Euler(35, 0, 0));
                return;
            }
            var me = shown[session.Local];
            var facing = Quaternion.Euler(0, session.Players[session.Local].Yaw, 0);
            // 後ろの高い位置から見下ろす。ring の端でもガラスの外から覗かないよう、camera を ring の内側に留める。
            var back = me + facing * new Vector3(0, 5f, -5f);
            var flat = Vector2.ClampMagnitude(new Vector2(back.x, back.z), ArenaRules.RingRadius - 0.6f);
            view.position = new Vector3(flat.x, back.y, flat.y);
            view.LookAt(me + Vector3.up + facing * Vector3.forward * 4);
        }

        void OnGUI()
        {
            label ??= new GUIStyle(GUI.skin.label) { fontSize = 22, normal = { textColor = new Color(0.6f, 1f, 1f) } };
            big ??= new GUIStyle(label) { fontSize = 48, alignment = TextAnchor.MiddleCenter };
            if (!session.InMatch)
            {
                if (session.Notice.Length > 0)
                    GUI.Label(new Rect(480, 20, Screen.width - 500, 40), session.Notice, label);
                return;
            }
            var me = session.Local;
            var time = session.Phase == ArenaPhase.Playing && session.TimeLeft <= 0
                ? "延長（次の 1 点で決着）"
                : $"残り {(int)session.TimeLeft / 60}:{(int)session.TimeLeft % 60:00}";
            GUI.Label(new Rect(20, 16, 900, 36), $"あなた {session.Scores[me]} - {session.Scores[1 - me]} 相手    {time}", label);
            var path = session.Path.Length > 0 ? $"経路: {session.Path} / RTT {session.RttMs} ms" : "経路: 確認中";
            GUI.Label(new Rect(20, 52, 900, 36), path, label);
            GUI.Label(new Rect(20, Screen.height - 50, Screen.width - 40, 40),
                "WASD 移動 / Space ジャンプ / Ctrl しゃがむ・スライド / E 投げる・受け止める", label);
            if (GUI.Button(new Rect(Screen.width - 140, 16, 120, 40), "退出"))
                session.Leave();
            var headline = session.Phase switch
            {
                ArenaPhase.Result => session.Winner == me ? "勝ち" : "負け",
                ArenaPhase.Left => "相手が退出しました",
                _ => session.Notice,
            };
            if (headline.Length > 0)
                GUI.Label(new Rect(0, Screen.height / 2 - 120, Screen.width, 80), headline, big);
            if ((session.Phase == ArenaPhase.Result || session.Phase == ArenaPhase.Left) &&
                GUI.Button(new Rect(Screen.width / 2 - 110, Screen.height / 2, 220, 56), "ロビーへ戻る"))
                session.Leave();
        }
    }
}
