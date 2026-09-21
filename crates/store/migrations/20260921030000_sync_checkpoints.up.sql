-- #1239: 背景で小分けに進む docs の読み出し(自分の replica の follow・block の読み出し、プロフィールの索引の補完)の
-- 進み具合。途中で止まっても続きから再開し、読み終えたら繰り返さない。key は処理と対象の replica ごとに 1 行。
CREATE TABLE IF NOT EXISTS sync_checkpoints (
  checkpoint_key TEXT PRIMARY KEY NOT NULL,
  checkpoint_value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

-- #1239 / ADR 0053 §6: author ごとの docs author の id。著者が署名つきの envelope の tag で申告した値だけを置く。
-- author replica の record を「docs author と key の組」で読むために使う。行は profile cache と同じく author ごとに 1 行。
CREATE TABLE IF NOT EXISTS author_docs_authors (
  author_pubkey TEXT PRIMARY KEY NOT NULL,
  docs_author TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
