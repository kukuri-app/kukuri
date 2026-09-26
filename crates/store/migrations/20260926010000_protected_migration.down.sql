DROP TRIGGER IF EXISTS protected_migration_rewind_own_envelope;
DROP TRIGGER IF EXISTS protected_migration_rewind_bookmark;
DROP TRIGGER IF EXISTS protected_migration_rewind_reaction_bookmark;
DROP TRIGGER IF EXISTS protected_migration_rewind_dm_message;
DROP INDEX IF EXISTS idx_game_room_cache_derived;
DROP INDEX IF EXISTS idx_live_session_cache_derived;
DROP TABLE IF EXISTS protected_migration;
