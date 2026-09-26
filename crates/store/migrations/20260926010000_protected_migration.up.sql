-- #1221 R5-G: 旧 iroh-data から保護所有先への移行の進み。kind ごとに次に読む位置と、
-- 最後に終端へ達した時刻を持つ(途中で止まっても、保存した位置から続ける)。
CREATE TABLE protected_migration (
    kind TEXT PRIMARY KEY,
    cursor TEXT NOT NULL,
    caught_up_at INTEGER
);

-- live・game の行は更新しても rowid が変わらないため、反映した時刻の順で歩く。
CREATE INDEX idx_live_session_cache_derived
    ON live_session_cache(derived_at, session_id);
CREATE INDEX idx_game_room_cache_derived
    ON game_room_cache(derived_at, room_id);

-- rowid の順で歩く表は、最大の行が消えた直後の追加で同じ rowid が再利用される。移行済みの位置より前の rowid で
-- 行が入ったら、その行の手前まで位置を戻して読み直す(読み飛ばして旧領域だけに残さない)。
CREATE TRIGGER protected_migration_rewind_own_envelope
AFTER INSERT ON envelopes
WHEN NEW.rowid <= (SELECT CAST(cursor AS INTEGER) FROM protected_migration WHERE kind = 'own_envelope')
BEGIN
    UPDATE protected_migration SET cursor = CAST(NEW.rowid - 1 AS TEXT), caught_up_at = NULL
    WHERE kind = 'own_envelope';
END;
CREATE TRIGGER protected_migration_rewind_bookmark
AFTER INSERT ON bookmarked_posts
WHEN NEW.rowid <= (SELECT CAST(cursor AS INTEGER) FROM protected_migration WHERE kind = 'bookmark')
BEGIN
    UPDATE protected_migration SET cursor = CAST(NEW.rowid - 1 AS TEXT), caught_up_at = NULL
    WHERE kind = 'bookmark';
END;
CREATE TRIGGER protected_migration_rewind_reaction_bookmark
AFTER INSERT ON bookmarked_custom_reactions
WHEN NEW.rowid <= (SELECT CAST(cursor AS INTEGER) FROM protected_migration WHERE kind = 'reaction_bookmark')
BEGIN
    UPDATE protected_migration SET cursor = CAST(NEW.rowid - 1 AS TEXT), caught_up_at = NULL
    WHERE kind = 'reaction_bookmark';
END;
CREATE TRIGGER protected_migration_rewind_dm_message
AFTER INSERT ON dm_messages
WHEN NEW.rowid <= (SELECT CAST(cursor AS INTEGER) FROM protected_migration WHERE kind = 'dm_message')
BEGIN
    UPDATE protected_migration SET cursor = CAST(NEW.rowid - 1 AS TEXT), caught_up_at = NULL
    WHERE kind = 'dm_message';
END;
