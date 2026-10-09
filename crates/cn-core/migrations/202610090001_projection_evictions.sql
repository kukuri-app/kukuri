-- #1698: 送信防止の適用で索引の真実源から外した投稿の、ArcadeDB の写しの消し待ち。
-- 適用の transaction で入れ、indexer の巡回が写しを消してから外す。
CREATE TABLE IF NOT EXISTS cn_index.projection_evictions (
    scope_kind TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    PRIMARY KEY (scope_kind, scope_id, object_id)
);
