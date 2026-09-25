-- The old channel capability remains first-writer-wins while a consenting client
-- identifies the epoch for bounded bucket reads. NULL keeps older requests on the legacy path.
ALTER TABLE cn_index.channel_secrets ADD COLUMN epoch_id TEXT;
