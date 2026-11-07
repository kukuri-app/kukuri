-- #2053: topic数によらない、日付ごとの失効世代と最古の保持可能日。
ALTER TABLE topic_retention_days ADD COLUMN expiry_generation INTEGER NOT NULL DEFAULT 0;
ALTER TABLE topic_retention_limits ADD COLUMN expired_before INTEGER NOT NULL DEFAULT 0;
CREATE TABLE topic_retention_expiry (
    bucket INTEGER PRIMARY KEY,
    generation INTEGER NOT NULL
);
CREATE INDEX topic_retention_days_expiry ON topic_retention_days (bucket, releasing, expiry_generation, topic_id);
