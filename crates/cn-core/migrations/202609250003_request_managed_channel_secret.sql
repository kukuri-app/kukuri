ALTER TABLE cn_index.channel_secrets
    ADD COLUMN request_managed BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE cn_index.channel_secrets
    ADD COLUMN rotated BOOLEAN NOT NULL DEFAULT FALSE;
