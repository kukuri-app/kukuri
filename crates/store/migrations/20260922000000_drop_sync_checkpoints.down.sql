CREATE TABLE IF NOT EXISTS sync_checkpoints (
  checkpoint_key TEXT PRIMARY KEY NOT NULL,
  checkpoint_value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
