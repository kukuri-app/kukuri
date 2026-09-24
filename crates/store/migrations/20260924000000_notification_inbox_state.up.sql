CREATE TABLE notification_inbox_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    unread_count INTEGER NOT NULL,
    read_through_seq INTEGER NOT NULL DEFAULT 0,
    read_through_at INTEGER,
    legacy_read_at INTEGER
);

INSERT INTO notification_inbox_state (singleton, unread_count)
SELECT 1, COUNT(*) FROM notifications WHERE read_at IS NULL;

CREATE TRIGGER notifications_unread_insert
AFTER INSERT ON notifications WHEN NEW.read_at IS NULL
BEGIN
    UPDATE notification_inbox_state SET unread_count = unread_count + 1 WHERE singleton = 1;
END;

CREATE TRIGGER notifications_unread_read
AFTER UPDATE OF read_at ON notifications
WHEN OLD.read_at IS NULL AND NEW.read_at IS NOT NULL
BEGIN
    UPDATE notification_inbox_state SET unread_count = unread_count - 1 WHERE singleton = 1;
END;

CREATE TRIGGER notifications_unread_delete
AFTER DELETE ON notifications
WHEN OLD.read_at IS NULL AND
    (OLD.dispatch_seq IS NULL AND (SELECT legacy_read_at FROM notification_inbox_state WHERE singleton = 1) IS NULL
     OR OLD.dispatch_seq > (SELECT read_through_seq FROM notification_inbox_state WHERE singleton = 1))
BEGIN
    UPDATE notification_inbox_state SET unread_count = unread_count - 1 WHERE singleton = 1;
END;

CREATE VIEW notification_inbox_rows AS
SELECT n.notification_id, n.recipient_pubkey, n.kind, n.actor_pubkey,
    n.source_envelope_id, n.source_replica_id, n.topic_id, n.channel_id,
    n.object_id, n.dm_id, n.message_id, n.preview_text, n.content_labels_json,
    n.created_at, n.received_at, n.dispatch_seq,
    CASE WHEN n.read_at IS NOT NULL THEN n.read_at
         WHEN n.dispatch_seq IS NULL THEN s.legacy_read_at
         WHEN n.dispatch_seq <= s.read_through_seq THEN s.read_through_at
         ELSE NULL END AS read_at
FROM notifications AS n CROSS JOIN notification_inbox_state AS s;
