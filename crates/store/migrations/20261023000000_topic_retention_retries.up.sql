-- #1909 AC-2: 保持する topic で、参照する blob を取れなかった対象の取り直しの予定と、諦めた数。
CREATE TABLE topic_retention_retries (
    topic_id TEXT NOT NULL,
    object_kind TEXT NOT NULL,
    object_id TEXT NOT NULL,
    bucket INTEGER NOT NULL,
    attempts INTEGER NOT NULL,
    next_at INTEGER NOT NULL,
    PRIMARY KEY (topic_id, object_kind, object_id)
);
CREATE INDEX topic_retention_retries_due ON topic_retention_retries (topic_id, next_at);
ALTER TABLE topic_retention ADD COLUMN given_up INTEGER NOT NULL DEFAULT 0;
