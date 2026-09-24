DROP TRIGGER IF EXISTS notifications_unread_delete;
DROP TRIGGER IF EXISTS notifications_unread_read;
DROP TRIGGER IF EXISTS notifications_unread_insert;

UPDATE notifications
SET read_at = (SELECT read_at FROM notification_inbox_rows
               WHERE notification_id = notifications.notification_id)
WHERE read_at IS NULL AND notification_id IN
    (SELECT notification_id FROM notification_inbox_rows WHERE read_at IS NOT NULL);

DROP VIEW IF EXISTS notification_inbox_rows;
DROP TABLE IF EXISTS notification_inbox_state;
