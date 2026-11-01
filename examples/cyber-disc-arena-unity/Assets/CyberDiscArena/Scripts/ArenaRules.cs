using System.Collections.Generic;
using UnityEngine;

namespace CyberDiscArena
{
    /// 1 tick の入力。移動は画面の向き（`Yaw`、度）に対する -1〜1。
    public struct ArenaInput
    {
        public uint Sequence;
        public Vector2 Move;
        public float Yaw;
        public bool Jump;
        public bool Crouch;
    }

    public struct PlayerState
    {
        /// 足元の位置。
        public Vector3 Position;
        public float VerticalSpeed;
        public float Yaw;
        public bool Crouching;
        /// slide の残り時間と向き。
        public float SlideTime;
        public Vector2 SlideDirection;
        /// host が最後に適用した入力の通し番号（client の予測の補正に使う）。
        public uint LastInput;

        public float Height => Crouching ? ArenaRules.CrouchHeight : ArenaRules.StandHeight;
        public Vector3 Forward => Quaternion.Euler(0, Yaw, 0) * Vector3.forward;
        public Vector3 Hand => Position + Vector3.up * ArenaRules.DiscHeight + Forward * 0.6f;
    }

    public enum DiscMode : byte { Held, Flying }

    public struct DiscState
    {
        public DiscMode Mode;
        public int Holder;
        public int Thrower;
        public Vector3 Position;
        public Vector3 Velocity;
        public int Bounces;
        public float FlightTime;
    }

    public enum ArenaPhase : byte { Waiting, Countdown, Playing, Result, Left }

    /// 確定した出来事。
    public enum ArenaEventKind : byte { Phase, Possess, Throw, Catch, Hit }

    public struct ArenaEvent
    {
        public uint Sequence;
        public ArenaEventKind Kind;
        public ArenaPhase Phase;
        /// 投げた・当てた側。`Phase` では勝者（無ければ -1）。
        public int Player;
        /// 出来事の後の持ち主（飛んでいれば -1）。
        public int Target;
        public Vector3 Position;
        public Vector3 Velocity;
        public byte Score0;
        public byte Score1;
    }

    /// host だけが進める裁定（1 対 1・ディスク 1 枚）。当てたら投げた側に 1 点で、ディスクは当たった側へ。受け止めたら
    /// 持ち主が替わる。壁で 3 回跳ねるか 3 秒飛ぶと止まり、近い側が拾う。先に 5 点で勝ち、3 分で点の多い方（同点なら
    /// 次の 1 点で決着）。ディスクの持ち主はここでしか変わらず、変わるたびに通し番号つきの出来事を出す。
    public sealed class ArenaRules
    {
        public const float TickSeconds = 1f / 30;
        public const float RingRadius = 9f;
        public const float CenterGap = 0.5f;
        public const float PlayerRadius = 0.45f;
        public const float StandHeight = 1.8f;
        public const float CrouchHeight = 0.9f;
        public const float DiscHeight = 1.1f;
        public const float RunSpeed = 6f;
        public const float SlideSpeed = 11f;
        public const float SlideSeconds = 0.4f;
        public const float JumpSpeed = 7.5f;
        public const float Gravity = 20f;
        public const float DiscSpeed = 16f;
        public const int MaxBounces = 3;
        public const float MaxFlightSeconds = 3f;
        public const float CatchRange = 1.6f;
        public const float CatchWindow = 0.25f;
        public const int WinningScore = 5;
        public const float MatchSeconds = 180f;
        public const float CountdownSeconds = 3f;

        public readonly PlayerState[] Players = new PlayerState[2];
        public readonly byte[] Scores = new byte[2];
        /// まだ送っていない出来事（host が送ったら消す）。
        public readonly List<ArenaEvent> Events = new List<ArenaEvent>();
        public DiscState Disc;
        public ArenaPhase Phase { get; private set; }
        /// 残り時間（秒）。延長中は 0。
        public float TimeLeft { get; private set; }
        public int Winner { get; private set; } = -1;

        readonly float[] catchUntil = new float[2];
        float clock;
        uint sequence;

        public static Vector3 Spawn(int player) => new Vector3(0, 0, player == 0 ? -5 : 5);

        public static float SpawnYaw(int player) => player == 0 ? 0 : 180;

        /// 入力で 1 tick 動かす（client の予測と同じ計算）。自分の側（player 0 は奥行きが負、1 は正）から出ない。
        public static void Move(ref PlayerState player, int side, ArenaInput input, float seconds)
        {
            player.Yaw = input.Yaw;
            var grounded = player.Position.y <= 0 && player.VerticalSpeed <= 0;
            var turned = Quaternion.Euler(0, input.Yaw, 0) * new Vector3(input.Move.x, 0, input.Move.y);
            var direction = Vector2.ClampMagnitude(new Vector2(turned.x, turned.z), 1);
            if (input.Crouch && !player.Crouching && grounded && direction.sqrMagnitude > 0.25f)
            {
                player.SlideTime = SlideSeconds;
                player.SlideDirection = direction.normalized;
            }
            player.Crouching = input.Crouch;
            if (!input.Crouch)
                player.SlideTime = 0;
            var velocity = player.SlideTime > 0
                ? player.SlideDirection * SlideSpeed
                : direction * (player.Crouching ? RunSpeed * 0.4f : RunSpeed);
            player.SlideTime = Mathf.Max(0, player.SlideTime - seconds);
            if (input.Jump && grounded && !player.Crouching)
                player.VerticalSpeed = JumpSpeed;
            if (player.Position.y > 0 || player.VerticalSpeed > 0)
                player.VerticalSpeed -= Gravity * seconds;
            var position = player.Position + new Vector3(velocity.x, player.VerticalSpeed, velocity.y) * seconds;
            if (position.y <= 0)
            {
                position.y = 0;
                player.VerticalSpeed = 0;
            }
            var flat = Vector2.ClampMagnitude(new Vector2(position.x, position.z), RingRadius - PlayerRadius);
            flat.y = side == 0 ? Mathf.Min(flat.y, -CenterGap) : Mathf.Max(flat.y, CenterGap);
            player.Position = new Vector3(flat.x, position.y, flat.y);
            player.LastInput = input.Sequence;
        }

        /// 2 人がそろったら試合を始める（カウントダウンの後に開始）。
        public void Begin()
        {
            for (var index = 0; index < 2; index++)
            {
                Players[index] = new PlayerState { Position = Spawn(index), Yaw = SpawnYaw(index) };
                Scores[index] = 0;
            }
            Winner = -1;
            SetPhase(ArenaPhase.Countdown, CountdownSeconds);
            Give(0);
        }

        /// 相手が抜けた。勝敗は付けずに終える。
        public void Leave()
        {
            Winner = -1;
            SetPhase(ArenaPhase.Left, 0);
        }

        /// 持ち主の投げる要求。持っていなければ何もしない。
        public void Throw(int player, float yaw)
        {
            if (Phase != ArenaPhase.Playing || Disc.Mode != DiscMode.Held || Disc.Holder != player)
                return;
            Players[player].Yaw = yaw;
            var hand = Players[player].Hand;
            Disc = new DiscState
            {
                Mode = DiscMode.Flying,
                Holder = -1,
                Thrower = player,
                Position = new Vector3(hand.x, DiscHeight, hand.z),
                Velocity = Players[player].Forward * DiscSpeed,
            };
            Emit(new ArenaEvent { Kind = ArenaEventKind.Throw, Player = player, Target = -1, Position = Disc.Position, Velocity = Disc.Velocity });
        }

        /// 受け止める要求。少しの間（`CatchWindow`）有効で、その間にディスクが届けば受け止める。
        public void Catch(int player) => catchUntil[player] = clock + CatchWindow;

        /// 1 tick 進める。出した出来事は `Events` に足す（送った側が消す）。
        public void Step(ArenaInput[] inputs, float seconds)
        {
            clock += seconds;
            if (Phase == ArenaPhase.Waiting || Phase == ArenaPhase.Result || Phase == ArenaPhase.Left)
                return;
            for (var index = 0; index < 2; index++)
                Move(ref Players[index], index, inputs[index], seconds);
            TimeLeft = Mathf.Max(0, TimeLeft - seconds);
            if (Phase == ArenaPhase.Countdown)
            {
                if (TimeLeft <= 0)
                    SetPhase(ArenaPhase.Playing, MatchSeconds);
                HoldDisc();
                return;
            }
            if (Disc.Mode == DiscMode.Held)
                HoldDisc();
            else
                Fly(seconds);
            if (Phase == ArenaPhase.Playing && TimeLeft <= 0 && Scores[0] != Scores[1])
                End(Scores[0] > Scores[1] ? 0 : 1);
        }

        void HoldDisc()
        {
            Disc.Position = Players[Disc.Holder].Hand;
            Disc.Velocity = Vector3.zero;
        }

        void Fly(float seconds)
        {
            Disc.Position += Disc.Velocity * seconds;
            Disc.FlightTime += seconds;
            var flat = new Vector2(Disc.Position.x, Disc.Position.z);
            if (flat.magnitude > RingRadius - 0.2f)
            {
                var normal = flat.normalized;
                var velocity = new Vector2(Disc.Velocity.x, Disc.Velocity.z);
                velocity -= 2 * Vector2.Dot(velocity, normal) * normal;
                flat = normal * (RingRadius - 0.2f);
                Disc.Position = new Vector3(flat.x, Disc.Position.y, flat.y);
                Disc.Velocity = new Vector3(velocity.x, 0, velocity.y);
                Disc.Bounces++;
            }
            for (var index = 0; index < 2; index++)
            {
                if (index == Disc.Thrower || catchUntil[index] < clock || !Near(index, CatchRange, 0.5f))
                    continue;
                Give(index, ArenaEventKind.Catch);
                return;
            }
            var target = 1 - Disc.Thrower;
            if (Near(target, PlayerRadius + 0.25f, 0))
            {
                var thrower = Disc.Thrower;
                Scores[thrower]++;
                Give(target, ArenaEventKind.Hit, thrower);
                if (Scores[thrower] >= WinningScore || TimeLeft <= 0)
                    End(thrower);
                return;
            }
            if (Disc.Bounces > MaxBounces || Disc.FlightTime > MaxFlightSeconds)
            {
                var nearest = (Disc.Position - Players[0].Position).sqrMagnitude <=
                    (Disc.Position - Players[1].Position).sqrMagnitude ? 0 : 1;
                Give(nearest);
            }
        }

        /// ディスクがその player の体（受け止めるときは手の届く範囲）にあるか。
        bool Near(int player, float range, float reach)
        {
            var body = Players[player];
            var flat = new Vector2(Disc.Position.x - body.Position.x, Disc.Position.z - body.Position.z);
            return flat.magnitude <= range && Disc.Position.y >= body.Position.y - reach &&
                Disc.Position.y <= body.Position.y + body.Height + reach;
        }

        void Give(int player, ArenaEventKind kind = ArenaEventKind.Possess, int thrower = -1)
        {
            Disc = new DiscState { Mode = DiscMode.Held, Holder = player, Thrower = -1 };
            catchUntil[0] = catchUntil[1] = 0;
            HoldDisc();
            Emit(new ArenaEvent { Kind = kind, Player = kind == ArenaEventKind.Hit ? thrower : player, Target = player });
        }

        void End(int winner)
        {
            Winner = winner;
            SetPhase(ArenaPhase.Result, 0);
        }

        void SetPhase(ArenaPhase phase, float time)
        {
            Phase = phase;
            TimeLeft = time;
            Emit(new ArenaEvent { Kind = ArenaEventKind.Phase, Player = Winner, Target = Disc.Mode == DiscMode.Held ? Disc.Holder : -1 });
        }

        void Emit(ArenaEvent @event)
        {
            @event.Sequence = ++sequence;
            @event.Phase = Phase;
            @event.Score0 = Scores[0];
            @event.Score1 = Scores[1];
            Events.Add(@event);
        }
    }
}
