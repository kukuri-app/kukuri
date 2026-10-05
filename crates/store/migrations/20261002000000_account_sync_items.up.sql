-- #1218 AC-3 / ADR 0061 §4: 本人の端末間の account 同期で採用した item の状態。item ごとに 1 行で、
-- `(updated_at, op_id)` が大きいものだけで置き換える（操作の log・重複排除の台帳は持たない）。value が NULL の行は tombstone。
CREATE TABLE IF NOT EXISTS account_sync_items (
  item_key TEXT PRIMARY KEY NOT NULL,
  op_id TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  value TEXT
);
