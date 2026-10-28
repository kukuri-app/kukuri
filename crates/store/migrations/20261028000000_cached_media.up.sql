-- #1949: 表示のために手元へ置いた他人の画像・動画の一覧。行は cache の blob（主 hash か poster）の回収と同じ
-- transaction で消すので、cache より多くは残らない。
CREATE TABLE cached_media (
    hash TEXT PRIMARY KEY NOT NULL,
    poster_hash TEXT,
    cached_at INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL
);
CREATE INDEX cached_media_order ON cached_media (cached_at DESC, hash DESC);
CREATE INDEX cached_media_poster ON cached_media (poster_hash) WHERE poster_hash IS NOT NULL;
