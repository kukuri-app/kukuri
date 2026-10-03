DROP TRIGGER IF EXISTS private_channel_participant_counts_update;
DROP TRIGGER IF EXISTS private_channel_participant_counts_insert;
DROP TABLE IF EXISTS private_channel_participant_counts;
DROP INDEX IF EXISTS idx_private_channel_participants_member;
ALTER TABLE private_channel_participants DROP COLUMN stale;
DROP INDEX IF EXISTS idx_private_channel_epochs_rotation;
ALTER TABLE private_channel_epochs DROP COLUMN rotation_after;
ALTER TABLE private_channel_epochs DROP COLUMN rotation_from;
