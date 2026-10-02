DROP TABLE IF EXISTS account_sync_cursors;
DROP INDEX IF EXISTS idx_private_channel_epochs_unwritten;
ALTER TABLE private_channel_epochs DROP COLUMN written;
DROP INDEX IF EXISTS idx_account_sync_items_unwritten;
ALTER TABLE account_sync_items DROP COLUMN written;
