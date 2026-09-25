-- One bounded cursor rotates both public and registered private epoch scopes.
CREATE TABLE cn_index.bucket_reader_cursor (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    last_kind TEXT NOT NULL DEFAULT '',
    last_scope_id TEXT NOT NULL DEFAULT '',
    last_peer_pubkey TEXT NOT NULL DEFAULT '',
    last_peer_endpoint_id TEXT NOT NULL DEFAULT ''
);
INSERT INTO cn_index.bucket_reader_cursor (id, last_kind, last_scope_id)
SELECT TRUE, 'public_topic', last_fair_topic
FROM cn_index.public_bucket_reader_cursor WHERE id = TRUE;
DROP TABLE cn_index.public_bucket_reader_cursor;
