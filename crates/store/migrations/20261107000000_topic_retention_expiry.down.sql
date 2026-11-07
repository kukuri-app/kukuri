-- 旧版でも失効状態を保つため、暗黙の失効を旧来のreleasingへ戻す。
UPDATE topic_retention_days SET releasing = 1
WHERE bucket < (SELECT expired_before FROM topic_retention_limits WHERE id = 1)
   OR expiry_generation < COALESCE((SELECT generation FROM topic_retention_expiry e
                                   WHERE e.bucket = topic_retention_days.bucket), 0);
DELETE FROM topic_retention_scopes WHERE (topic_id, bucket) IN
    (SELECT topic_id, bucket FROM topic_retention_days WHERE releasing = 1);
DROP INDEX topic_retention_days_expiry;
DROP TABLE topic_retention_expiry;
ALTER TABLE topic_retention_limits DROP COLUMN expired_before;
ALTER TABLE topic_retention_days DROP COLUMN expiry_generation;
