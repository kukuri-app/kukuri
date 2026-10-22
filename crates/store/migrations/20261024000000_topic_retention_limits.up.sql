-- #1909 AC-3: 保持の期間と容量。used_bytes は保持の保護参照を付けた record と blob の大きさの合計、released_through は
-- 容量を超えて保護を外した最も新しい日(それより前の日は読まない)。
CREATE TABLE topic_retention_limits (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    retention_days INTEGER NOT NULL,
    capacity_bytes INTEGER NOT NULL,
    used_bytes INTEGER NOT NULL,
    released_through INTEGER
);
INSERT INTO topic_retention_limits (id, retention_days, capacity_bytes, used_bytes)
VALUES (1, 30, 10737418240, 0);

-- topic の日ごとの保護の合計と、最後まで読んだか、保護参照を外している途中か。外し終えた日の行は消す。
CREATE TABLE topic_retention_days (
    topic_id TEXT NOT NULL,
    bucket INTEGER NOT NULL,
    used_bytes INTEGER NOT NULL DEFAULT 0,
    read_done INTEGER NOT NULL DEFAULT 0,
    releasing INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (topic_id, bucket)
);
CREATE INDEX topic_retention_days_release ON topic_retention_days (releasing, bucket, topic_id);
CREATE INDEX topic_retention_days_unread ON topic_retention_days (topic_id, bucket)
WHERE read_done = 0 AND releasing = 0;

-- 保持する日の topic の bucket の replica。書込みの record に、その日の保護参照を付ける。protect_after は、登録より前に
-- 置かれた record に保護参照を付けた位置(cache key)で、付け終えたら NULL。
CREATE TABLE topic_retention_scopes (
    scope_key TEXT PRIMARY KEY NOT NULL,
    topic_id TEXT NOT NULL,
    bucket INTEGER NOT NULL,
    protect_after TEXT
);
CREATE INDEX topic_retention_scopes_day ON topic_retention_scopes (topic_id, bucket);
CREATE INDEX topic_retention_scopes_pending ON topic_retention_scopes (scope_key)
WHERE protect_after IS NOT NULL;
