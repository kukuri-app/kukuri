-- #1909 / ADR 0055 §1.1: CLI daemon が保持する公開 topic と、topic の bucket の時刻順の索引を取り込んだ続きの位置。
CREATE TABLE topic_retention (
    topic_id TEXT PRIMARY KEY NOT NULL,
    retained_at INTEGER NOT NULL
);

-- 索引（`indexes/timeline/` など）と bucket ごとに、読み終えた最後の entry の（時刻, object id）。
CREATE TABLE topic_retention_cursors (
    topic_id TEXT NOT NULL,
    index_prefix TEXT NOT NULL,
    bucket INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    object_id TEXT NOT NULL,
    PRIMARY KEY (topic_id, index_prefix, bucket)
);
