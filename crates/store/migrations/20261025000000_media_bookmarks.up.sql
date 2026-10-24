CREATE TABLE bookmarked_media (
    hash TEXT PRIMARY KEY NOT NULL,
    bookmarked_at INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL
);
CREATE INDEX bookmarked_media_order ON bookmarked_media (bookmarked_at DESC, hash DESC);
