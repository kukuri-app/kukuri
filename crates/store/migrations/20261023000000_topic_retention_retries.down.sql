ALTER TABLE topic_retention DROP COLUMN given_up;
DROP INDEX topic_retention_retries_due;
DROP TABLE topic_retention_retries;
