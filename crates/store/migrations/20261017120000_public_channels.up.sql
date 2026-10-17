-- #1840 / ADR 0068: 公開 channel（公開スレッド）の一覧の行。時刻はどちらもミリ秒で、last_post_at の初期値は作成時刻。
-- 他人が作った行は remote cache（kind 'public_channel'、cache_key は channel_id）で容量を数え、7 日使わなければ回収する。
CREATE TABLE public_channels (
  channel_id TEXT PRIMARY KEY NOT NULL,
  topic_id TEXT NOT NULL,
  title TEXT NOT NULL,
  creator_pubkey TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  first_post_id TEXT,
  last_post_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX idx_public_channels_created ON public_channels (topic_id, created_at, channel_id);
CREATE INDEX idx_public_channels_last_post ON public_channels (topic_id, last_post_at, channel_id);
