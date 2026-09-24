use super::*;
use sqlx::Row;

fn row(index: usize) -> NotificationRow {
    NotificationRow {
        notification_id: format!("notification-{index}"),
        recipient_pubkey: "recipient".into(),
        kind: NotificationKind::Mention,
        actor_pubkey: "actor".into(),
        source_envelope_id: None,
        source_replica_id: None,
        topic_id: None,
        channel_id: None,
        object_id: None,
        dm_id: None,
        message_id: None,
        preview_text: None,
        content_labels: None,
        created_at: 10,
        received_at: 20,
        read_at: None,
    }
}

async fn assert_dispatch_pages(store: &dyn NotificationStore) {
    assert_eq!(store.notification_dispatch_head().await.unwrap(), 0);
    for index in 0..129 {
        assert!(store.put_notification_if_absent(row(index)).await.unwrap());
    }
    assert!(!store.put_notification_if_absent(row(0)).await.unwrap());
    assert_eq!(store.notification_dispatch_head().await.unwrap(), 129);
    store
        .mark_notification_read("notification-64", 30)
        .await
        .unwrap();

    let first = store.list_notification_dispatch_after(0).await.unwrap();
    let second = store.list_notification_dispatch_after(64).await.unwrap();
    let third = store.list_notification_dispatch_after(128).await.unwrap();
    assert_eq!(first.len(), NOTIFICATION_DISPATCH_PAGE_SIZE);
    assert_eq!(second.len(), NOTIFICATION_DISPATCH_PAGE_SIZE);
    assert_eq!(third.len(), 1);
    assert_eq!(first[0].0, 1);
    assert_eq!(first[0].1.notification_id, "notification-0");
    assert_eq!(second[0].0, 65);
    assert_eq!(second[0].1.read_at, Some(30));
    assert_eq!(third[0].0, 129);
    assert!(
        store
            .list_notification_dispatch_after(129)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn dispatch_pages_follow_insertion_order_for_tied_timestamps() {
    assert_dispatch_pages(&MemoryStore::default()).await;
    let dir = tempdir().unwrap();
    let store = SqliteStore::connect_file(dir.path().join("notifications.db"))
        .await
        .unwrap();
    assert_dispatch_pages(&store).await;
    let plan = sqlx::query(
        "EXPLAIN QUERY PLAN SELECT notification_id FROM notifications WHERE dispatch_seq > 0 ORDER BY dispatch_seq ASC LIMIT 64",
    )
    .fetch_all(store.pool())
    .await
    .unwrap()
    .iter()
    .map(|row| row.get::<String, _>("detail"))
    .collect::<Vec<_>>()
    .join(" | ");
    assert!(plan.contains("idx_notifications_dispatch_seq"), "{plan}");
    drop(store);
    let reopened = SqliteStore::connect_file(dir.path().join("notifications.db"))
        .await
        .unwrap();
    assert_eq!(reopened.notification_dispatch_head().await.unwrap(), 129);
    assert_eq!(
        reopened
            .list_notification_dispatch_after(128)
            .await
            .unwrap()[0]
            .1
            .notification_id,
        "notification-128"
    );
}

#[tokio::test]
async fn notification_inbox_page_uses_the_received_index_and_status_uses_one_state_row() {
    let store = SqliteStore::connect_memory().await.unwrap();
    let page_plan = sqlx::query(
        "EXPLAIN QUERY PLAN SELECT notification_id FROM notification_inbox_rows \
         WHERE (received_at, notification_id) < (20, 'notification-10') \
         ORDER BY received_at DESC, notification_id DESC LIMIT 21",
    )
    .fetch_all(store.pool())
    .await
    .unwrap()
    .iter()
    .map(|row| row.get::<String, _>("detail"))
    .collect::<Vec<_>>()
    .join(" | ");
    assert!(page_plan.contains("idx_notifications_inbox"), "{page_plan}");
    let dispatch_plan = sqlx::query(
        "EXPLAIN QUERY PLAN SELECT dispatch_seq FROM notification_inbox_rows \
         WHERE dispatch_seq > 0 ORDER BY dispatch_seq ASC LIMIT 64",
    )
    .fetch_all(store.pool())
    .await
    .unwrap()
    .iter()
    .map(|row| row.get::<String, _>("detail"))
    .collect::<Vec<_>>()
    .join(" | ");
    assert!(
        dispatch_plan.contains("idx_notifications_dispatch_seq"),
        "{dispatch_plan}"
    );
    let status_plan = sqlx::query(
        "EXPLAIN QUERY PLAN SELECT unread_count FROM notification_inbox_state WHERE singleton = 1",
    )
    .fetch_all(store.pool())
    .await
    .unwrap()
    .iter()
    .map(|row| row.get::<String, _>("detail"))
    .collect::<Vec<_>>()
    .join(" | ");
    assert!(status_plan.contains("INTEGER PRIMARY KEY"), "{status_plan}");
}

#[tokio::test]
async fn deleting_one_notification_updates_the_page_and_unread_count() {
    let store = SqliteStore::connect_memory().await.unwrap();
    for index in 0..25 {
        let mut notification = row(index);
        notification.notification_id = format!("notification-{index:02}");
        store
            .put_notification_if_absent(notification)
            .await
            .unwrap();
    }
    assert_eq!(store.count_unread_notifications().await.unwrap(), 25);
    sqlx::query("DELETE FROM notifications WHERE notification_id = 'notification-24'")
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.count_unread_notifications().await.unwrap(), 24);
    let page = store.list_notifications_page(None, false).await.unwrap();
    assert_eq!(page.len(), NOTIFICATION_PAGE_SIZE + 1);
    assert_eq!(page[0].notification_id, "notification-23");
    store
        .mark_notification_read("notification-23", 30)
        .await
        .unwrap();
    assert_eq!(store.count_unread_notifications().await.unwrap(), 23);
}

#[tokio::test]
async fn read_all_watermark_suppresses_old_dispatch_but_not_new_arrivals() {
    async fn verify(store: &dyn NotificationStore) {
        store.put_notification_if_absent(row(0)).await.unwrap();
        store.mark_all_notifications_read(30).await.unwrap();
        assert_eq!(
            store.list_notification_dispatch_after(0).await.unwrap()[0]
                .1
                .read_at,
            Some(30)
        );
        store.put_notification_if_absent(row(1)).await.unwrap();
        assert_eq!(
            store.list_notification_dispatch_after(1).await.unwrap()[0]
                .1
                .read_at,
            None
        );
        assert_eq!(store.count_unread_notifications().await.unwrap(), 1);
    }
    verify(&MemoryStore::default()).await;
    verify(&SqliteStore::connect_memory().await.unwrap()).await;
}

#[tokio::test]
async fn notification_page_and_one_row_read_stay_bounded_as_history_grows() {
    for history in [100, 1_000, 10_000] {
        let store = MemoryStore::default();
        for index in 0..history {
            store.put_notification_if_absent(row(index)).await.unwrap();
        }
        assert_eq!(
            store
                .list_notifications_page(None, false)
                .await
                .unwrap()
                .len(),
            NOTIFICATION_PAGE_SIZE + 1
        );
        assert_eq!(store.count_unread_notifications().await.unwrap(), history);
        store
            .mark_notification_read("notification-0", 30)
            .await
            .unwrap();
        assert_eq!(
            store.count_unread_notifications().await.unwrap(),
            history - 1
        );
    }
}

#[tokio::test]
async fn notification_cursor_pages_match_between_backends() {
    async fn pages(store: &dyn NotificationStore) -> (Vec<String>, Vec<String>, Vec<String>) {
        for index in 0..45 {
            let mut notification = row(index);
            notification.notification_id = format!("page-{index:02}");
            notification.received_at = index as i64;
            store
                .put_notification_if_absent(notification)
                .await
                .unwrap();
        }
        let first = store.list_notifications_page(None, false).await.unwrap();
        let older = store
            .list_notifications_page(Some(&NotificationCursor::from(&first[19])), false)
            .await
            .unwrap();
        let newer = store
            .list_notifications_page(Some(&NotificationCursor::from(&older[0])), true)
            .await
            .unwrap();
        let ids =
            |rows: Vec<NotificationRow>| rows.into_iter().map(|row| row.notification_id).collect();
        (ids(first), ids(older), ids(newer))
    }
    let sqlite = pages(&SqliteStore::connect_memory().await.unwrap()).await;
    let memory = pages(&MemoryStore::default()).await;
    assert_eq!(sqlite, memory);
    assert_eq!(sqlite.0.len(), 21);
    assert_eq!(sqlite.1.len(), 21);
    assert_eq!(
        sqlite.2.into_iter().rev().collect::<Vec<_>>(),
        sqlite.0[..20]
    );
}

#[tokio::test]
async fn notification_read_all_keeps_new_late_arrivals_unread() {
    async fn state(store: &dyn NotificationStore) -> (Vec<NotificationRow>, usize) {
        let mut existing = row(0);
        existing.received_at = 100;
        store.put_notification_if_absent(existing).await.unwrap();
        store.mark_all_notifications_read(150).await.unwrap();
        let mut late = row(1);
        late.received_at = 50;
        store.put_notification_if_absent(late).await.unwrap();
        (
            store.list_notifications().await.unwrap(),
            store.count_unread_notifications().await.unwrap(),
        )
    }
    let sqlite = state(&SqliteStore::connect_memory().await.unwrap()).await;
    let memory = state(&MemoryStore::default()).await;
    assert_eq!(sqlite, memory);
    assert_eq!(sqlite.1, 1);
    assert_eq!(sqlite.0[0].read_at, Some(150));
    assert_eq!(sqlite.0[1].read_at, None);
}

#[tokio::test]
async fn dispatch_migration_excludes_existing_inbox_rows_without_rewriting_them() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/20260405000000_notifications.up.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO notifications (notification_id, recipient_pubkey, kind, actor_pubkey, created_at, received_at) VALUES ('legacy', 'recipient', 'mention', 'actor', 10, 20)",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::raw_sql(include_str!(
        "../../migrations/20260923000000_notification_dispatch_sequence.up.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let old_sequence: Option<i64> =
        sqlx::query_scalar("SELECT dispatch_seq FROM notifications WHERE notification_id='legacy'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        old_sequence, None,
        "existing inbox history must not be re-toasted"
    );
    sqlx::query(
        "INSERT INTO notifications (notification_id, recipient_pubkey, kind, actor_pubkey, created_at, received_at) VALUES ('new', 'recipient', 'mention', 'actor', 10, 20)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let new_sequence: i64 =
        sqlx::query_scalar("SELECT dispatch_seq FROM notifications WHERE notification_id='new'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(new_sequence, 1);
    sqlx::raw_sql(include_str!(
        "../../migrations/20260924000000_notification_inbox_state.up.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let unread: i64 =
        sqlx::query_scalar("SELECT unread_count FROM notification_inbox_state WHERE singleton = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        unread, 2,
        "existing unread rows enter the one-time count migration"
    );
}
