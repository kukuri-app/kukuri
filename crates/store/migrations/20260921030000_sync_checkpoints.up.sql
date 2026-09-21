-- #1239: 背景で小分けに進む docs の読み出し(自分の replica の follow・block の読み出し、プロフィールの索引の補完)の
-- 進み具合。途中で止まっても続きから再開し、読み終えたら繰り返さない。key は処理と対象の replica ごとに 1 行。
CREATE TABLE IF NOT EXISTS sync_checkpoints (
  checkpoint_key TEXT PRIMARY KEY NOT NULL,
  checkpoint_value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
