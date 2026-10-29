-- #1978: action session の room（ADR 0070 §5.2）。一覧は application ID ごとに更新の新しい順で読み、従来の
-- ScoreGame・Dome の一覧は application_id = '' の範囲だけを読む（どちらも他方の行の数に比例しない）。
ALTER TABLE game_room_cache ADD COLUMN action_json TEXT;
ALTER TABLE game_room_cache ADD COLUMN application_id TEXT NOT NULL DEFAULT '';

DROP INDEX IF EXISTS idx_game_room_cache_topic_updated;
CREATE INDEX IF NOT EXISTS idx_game_room_cache_topic_updated
    ON game_room_cache(topic_id, channel_id, application_id, updated_at DESC, room_id DESC);
