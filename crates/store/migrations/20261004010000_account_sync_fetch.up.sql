-- #1218 AC-5b / ADR 0061 §10: replica へ書いたかの欄（送信待ち）と、相手(端末)ごとの取得の cursor。
-- 既存の行は未書込みとして始め、送り直しが replica の版と比べて書いたとするか書くかを決める。
ALTER TABLE account_sync_items ADD COLUMN written INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_account_sync_items_unwritten
  ON account_sync_items (item_key) WHERE written = 0;
ALTER TABLE private_channel_epochs ADD COLUMN written INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_private_channel_epochs_unwritten
  ON private_channel_epochs (channel_id, epoch_id) WHERE written = 0;
CREATE TABLE IF NOT EXISTS account_sync_cursors (
  device_id TEXT PRIMARY KEY NOT NULL,
  seq INTEGER NOT NULL,
  head INTEGER NOT NULL,
  cycle_prefix TEXT,
  cycle_head INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_account_sync_cursors_updated
  ON account_sync_cursors (updated_at);
