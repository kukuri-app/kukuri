CREATE TABLE cn_private_index_grants (
    base_url TEXT NOT NULL,
    topic_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    applied_epoch_id TEXT NOT NULL,
    node_revision INTEGER NOT NULL DEFAULT 0,
    channel_revision INTEGER NOT NULL DEFAULT 0,
    last_checked_at_ms INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    PRIMARY KEY (base_url, topic_id, channel_id)
);
CREATE INDEX cn_private_index_grants_due
    ON cn_private_index_grants (base_url, active, last_checked_at_ms, topic_id, channel_id);
CREATE TABLE cn_private_index_stops (
    kind TEXT NOT NULL,
    id TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (kind, id)
);
