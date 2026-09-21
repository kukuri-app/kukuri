-- #1239: 背景で小分けに進む docs の読み出し(自分の replica の follow・block の全件の読み出し、プロフィールの索引の補完)を
-- 廃止した(AGENTS.md: ユースケース上ユーザーが必要としない限り同期・復旧はしない)。その進み具合の表を消す。
DROP TABLE IF EXISTS sync_checkpoints;
