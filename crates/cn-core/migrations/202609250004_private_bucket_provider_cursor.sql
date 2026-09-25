CREATE TABLE cn_index.private_bucket_provider_cursor (
    channel_id TEXT PRIMARY KEY REFERENCES cn_index.channel_secrets(channel_id) ON DELETE CASCADE,
    last_pubkey TEXT NOT NULL DEFAULT '',
    last_endpoint_id TEXT NOT NULL DEFAULT ''
);
CREATE INDEX private_bucket_approved_requesters
    ON cn_index.indexing_requests(target_id, requester_pubkey)
    WHERE kind='private_channel' AND status='approved';
