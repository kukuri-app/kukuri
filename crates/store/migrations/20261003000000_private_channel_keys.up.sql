-- #1218 AC-4b / ADR 0061 §9: private channel の参加（channel ごと）と世代の鍵（(channel, epoch) ごと）。
-- 1 つの JSON に全件を入れていた registry の代わりに、各操作が対象の行だけを読み書きする。秘密は封をして置く。
CREATE TABLE IF NOT EXISTS private_channels (
  channel_key TEXT PRIMARY KEY NOT NULL,
  topic_id TEXT NOT NULL,
  channel_id TEXT NOT NULL,
  label TEXT NOT NULL,
  creator_pubkey TEXT NOT NULL,
  owner_pubkey TEXT NOT NULL,
  joined_via_pubkey TEXT,
  audience_kind TEXT NOT NULL,
  current_epoch_id TEXT NOT NULL,
  controller TEXT,
  joined INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  op_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_private_channels_channel_id
  ON private_channels (channel_id);
CREATE INDEX IF NOT EXISTS idx_private_channels_joined
  ON private_channels (channel_key) WHERE joined = 1;
CREATE INDEX IF NOT EXISTS idx_private_channels_topic
  ON private_channels (topic_id, channel_key) WHERE joined = 1;
CREATE INDEX IF NOT EXISTS idx_private_channels_owner
  ON private_channels (owner_pubkey, channel_key) WHERE joined = 1;

CREATE TABLE IF NOT EXISTS private_channel_epochs (
  channel_id TEXT NOT NULL,
  epoch_id TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  receive_key_id TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  sealed_secret BLOB NOT NULL,
  PRIMARY KEY (channel_id, epoch_id)
);
CREATE INDEX IF NOT EXISTS idx_private_channel_epochs_receive
  ON private_channel_epochs (receive_key_id);
CREATE INDEX IF NOT EXISTS idx_private_channel_epochs_started
  ON private_channel_epochs (channel_id, started_at, epoch_id);
