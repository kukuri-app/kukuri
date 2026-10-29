DELETE FROM game_room_cache WHERE application_id <> '';
DROP INDEX IF EXISTS idx_game_room_cache_topic_updated;
CREATE INDEX IF NOT EXISTS idx_game_room_cache_topic_updated
    ON game_room_cache(topic_id, channel_id, updated_at DESC, room_id DESC);
ALTER TABLE game_room_cache DROP COLUMN application_id;
ALTER TABLE game_room_cache DROP COLUMN action_json;
