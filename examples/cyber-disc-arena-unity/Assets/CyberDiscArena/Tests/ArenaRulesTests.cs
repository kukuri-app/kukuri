using System.Linq;
using NUnit.Framework;
using UnityEngine;

namespace CyberDiscArena.Tests
{
    /// host の裁定の規則（当てたら 1 点・受け止めたら持ち主が替わる・跳ねて止まったら近い側・5 点か 3 分で終わり）。
    public sealed class ArenaRulesTests
    {
        static readonly ArenaInput[] Still = new ArenaInput[2];

        static ArenaRules Playing()
        {
            var rules = new ArenaRules();
            rules.Begin();
            Run(rules, ArenaRules.CountdownSeconds + 0.1f);
            Assert.AreEqual(ArenaPhase.Playing, rules.Phase);
            return rules;
        }

        static void Run(ArenaRules rules, float seconds, ArenaInput[] inputs = null)
        {
            for (var elapsed = 0f; elapsed < seconds; elapsed += ArenaRules.TickSeconds)
                rules.Step(inputs ?? Still, ArenaRules.TickSeconds);
        }

        static float Toward(ArenaRules rules, int from)
        {
            var delta = rules.Players[1 - from].Position - rules.Players[from].Position;
            return Mathf.Atan2(delta.x, delta.z) * Mathf.Rad2Deg;
        }

        [Test]
        public void AHitScoresForTheThrowerAndHandsTheDiscToTheTarget()
        {
            var rules = Playing();
            Assert.AreEqual(0, rules.Disc.Holder);
            rules.Throw(0, Toward(rules, 0));
            Run(rules, 1);
            CollectionAssert.AreEqual(new byte[] { 1, 0 }, rules.Scores);
            Assert.AreEqual(DiscMode.Held, rules.Disc.Mode);
            Assert.AreEqual(1, rules.Disc.Holder);
            var hit = rules.Events.Single(@event => @event.Kind == ArenaEventKind.Hit);
            Assert.AreEqual((0, 1, (byte)1), (hit.Player, hit.Target, hit.Score0));
        }

        [Test]
        public void ACatchChangesTheHolderWithoutAPoint()
        {
            var rules = Playing();
            rules.Throw(0, Toward(rules, 0));
            Run(rules, 0.45f);
            rules.Catch(1);
            Run(rules, 0.3f);
            CollectionAssert.AreEqual(new byte[] { 0, 0 }, rules.Scores);
            Assert.AreEqual(1, rules.Disc.Holder);
            Assert.AreEqual(1, rules.Events.Single(@event => @event.Kind == ArenaEventKind.Catch).Target);
        }

        [Test]
        public void CrouchingOrJumpingLetsTheDiscPass()
        {
            foreach (var dodge in new[] { new ArenaInput { Crouch = true, Yaw = 180 }, new ArenaInput { Jump = true, Yaw = 180 } })
            {
                var rules = Playing();
                rules.Throw(0, Toward(rules, 0));
                // 届く少し前にかわす（ジャンプは早すぎると、届くころには降りている）。
                Run(rules, 0.2f);
                Run(rules, 0.5f, new[] { new ArenaInput(), dodge });
                CollectionAssert.AreEqual(new byte[] { 0, 0 }, rules.Scores, dodge.Crouch ? "crouch" : "jump");
            }
        }

        [Test]
        public void AFlightThatStopsGoesToTheNearestPlayer()
        {
            var rules = Playing();
            // 横の壁へ投げると、跳ねて回り、3 秒で止まる。
            rules.Throw(0, 90);
            Run(rules, ArenaRules.MaxFlightSeconds + 0.2f);
            Assert.AreEqual(DiscMode.Held, rules.Disc.Mode);
            Assert.AreEqual(ArenaEventKind.Possess, rules.Events.Last().Kind);
            CollectionAssert.AreEqual(new byte[] { 0, 0 }, rules.Scores);
        }

        [Test]
        public void OnlyTheHolderCanThrowSoTheDiscHasOneOwner()
        {
            var rules = Playing();
            rules.Throw(1, Toward(rules, 1));
            Assert.AreEqual(DiscMode.Held, rules.Disc.Mode);
            Assert.AreEqual(0, rules.Disc.Holder);
            rules.Throw(0, Toward(rules, 0));
            rules.Throw(0, Toward(rules, 0));
            Assert.AreEqual(1, rules.Events.Count(@event => @event.Kind == ArenaEventKind.Throw));
        }

        [Test]
        public void FivePointsWinAndTheEventsAreNumberedWithoutGaps()
        {
            var rules = Playing();
            for (var point = 0; point < ArenaRules.WinningScore; point++)
            {
                // 当たった側が持つので投げ返させ、player 0 が受け止めてから当てる。
                if (rules.Disc.Holder == 1)
                {
                    rules.Throw(1, Toward(rules, 1));
                    Run(rules, 0.45f);
                    rules.Catch(0);
                    Run(rules, 0.3f);
                }
                Assert.AreEqual(0, rules.Disc.Holder);
                rules.Throw(0, Toward(rules, 0));
                Run(rules, 1);
            }
            Assert.AreEqual(ArenaPhase.Result, rules.Phase);
            Assert.AreEqual(0, rules.Winner);
            CollectionAssert.AreEqual(new byte[] { 5, 0 }, rules.Scores);
            var numbers = rules.Events.Select(@event => @event.Sequence).ToList();
            CollectionAssert.AreEqual(Enumerable.Range(1, numbers.Count).Select(number => (uint)number), numbers);
            Assert.AreEqual(ArenaPhase.Result, rules.Events.Last().Phase);
        }

        [Test]
        public void WhenTimeRunsOutTheLeaderWinsAndATieGoesToTheNextPoint()
        {
            var rules = Playing();
            Run(rules, ArenaRules.MatchSeconds + 1);
            Assert.AreEqual(ArenaPhase.Playing, rules.Phase, "a 0-0 tie continues");
            rules.Throw(0, Toward(rules, 0));
            Run(rules, 1);
            Assert.AreEqual(ArenaPhase.Result, rules.Phase);
            Assert.AreEqual(0, rules.Winner);
        }

        [Test]
        public void PlayersStayOnTheirOwnSideAndInsideTheRing()
        {
            var player = new PlayerState { Position = ArenaRules.Spawn(0) };
            for (var tick = 0; tick < 200; tick++)
                ArenaRules.Move(ref player, 0, new ArenaInput { Move = new Vector2(0.3f, 1) }, ArenaRules.TickSeconds);
            Assert.LessOrEqual(player.Position.z, -ArenaRules.CenterGap);
            Assert.LessOrEqual(new Vector2(player.Position.x, player.Position.z).magnitude, ArenaRules.RingRadius);
        }
    }
}
