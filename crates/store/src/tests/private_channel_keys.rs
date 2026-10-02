//! #1218 AC-4b: private channel の参加と世代の鍵の行（ADR 0061 §9）を、SQLite と Memory の両方で同じ意味に保つ。

use super::*;
use crate::{
    ACCOUNT_SYNC_CURSOR_LIMIT, AccountSyncCursor, AccountSyncRow, AccountSyncStore,
    PrivateChannelEpochRange, PrivateChannelEpochRow, PrivateChannelFilter, PrivateChannelKeyStore,
    PrivateChannelRow,
};

fn channel(topic: &str, channel: &str, owner: &str, joined: bool) -> PrivateChannelRow {
    PrivateChannelRow {
        channel_key: format!("{topic}::{channel}"),
        topic_id: topic.to_string(),
        channel_id: channel.to_string(),
        label: "label".to_string(),
        creator_pubkey: owner.to_string(),
        owner_pubkey: owner.to_string(),
        joined_via_pubkey: None,
        audience_kind: "invite_only".to_string(),
        current_epoch_id: "epoch-3".to_string(),
        controller: None,
        joined,
        updated_at: 1,
        op_id: String::new(),
    }
}

fn epoch(channel: &str, epoch: &str, started_at: i64, secret: u8) -> PrivateChannelEpochRow {
    PrivateChannelEpochRow {
        channel_id: channel.to_string(),
        epoch_id: epoch.to_string(),
        started_at,
        receive_key_id: format!("{channel}/{epoch}"),
        updated_at: 1,
        sealed_secret: vec![secret],
        rotation_from: None,
        rotation_after: None,
    }
}

fn ids(epochs: &[PrivateChannelEpochRow]) -> Vec<&str> {
    epochs.iter().map(|epoch| epoch.epoch_id.as_str()).collect()
}

async fn assert_private_channel_keys(store: &dyn PrivateChannelKeyStore) {
    let a = channel("t1", "a", "owner", true);
    store
        .put_private_channel(
            &a,
            &[epoch("a", "epoch-1", 10, 1), epoch("a", "epoch-2", 20, 2)],
        )
        .await
        .unwrap();
    // 既にある鍵の行は置き換えない(同じ (channel, epoch) の再保存は何もしない)。
    store
        .put_private_channel(
            &a,
            &[epoch("a", "epoch-2", 20, 9), epoch("a", "epoch-3", 30, 3)],
        )
        .await
        .unwrap();
    store
        .put_private_channel(&channel("t1", "b", "other", true), &[])
        .await
        .unwrap();
    store
        .put_private_channel(&channel("t2", "c", "owner", true), &[])
        .await
        .unwrap();
    store
        .put_private_channel(&channel("t1", "d", "owner", false), &[])
        .await
        .unwrap();

    assert_eq!(
        store.get_private_channel("t1::a").await.unwrap(),
        Some(a.clone())
    );
    assert_eq!(store.get_private_channel_by_id("a").await.unwrap(), Some(a));
    let keys = |rows: Vec<PrivateChannelRow>| {
        rows.into_iter()
            .map(|row| row.channel_key)
            .collect::<Vec<_>>()
    };
    // 一覧は参加中の行だけを、key の順に cursor から上限まで。
    assert_eq!(
        keys(
            store
                .list_joined_private_channels(PrivateChannelFilter::All, "", 10)
                .await
                .unwrap()
        ),
        ["t1::a", "t1::b", "t2::c"]
    );
    assert_eq!(
        keys(
            store
                .list_joined_private_channels(PrivateChannelFilter::Topic("t1"), "t1::a", 10)
                .await
                .unwrap()
        ),
        ["t1::b"]
    );
    assert_eq!(
        keys(
            store
                .list_joined_private_channels(PrivateChannelFilter::Owner("owner"), "", 1)
                .await
                .unwrap()
        ),
        ["t1::a"]
    );

    assert_eq!(
        store
            .get_private_channel_epoch("a", "epoch-2")
            .await
            .unwrap()
            .map(|row| row.sealed_secret),
        Some(vec![2])
    );
    assert_eq!(
        store
            .find_private_channel_epoch("a/epoch-3")
            .await
            .unwrap()
            .map(|row| row.epoch_id),
        Some("epoch-3".to_string())
    );
    assert_eq!(store.find_private_channel_epoch("a/x").await.unwrap(), None);
    assert_eq!(
        ids(&store
            .list_private_channel_epochs("a", PrivateChannelEpochRange::AtOrBefore(25), 8)
            .await
            .unwrap()),
        ["epoch-2", "epoch-1"]
    );
    assert_eq!(
        ids(&store
            .list_private_channel_epochs("a", PrivateChannelEpochRange::After(10), 1)
            .await
            .unwrap()),
        ["epoch-2"]
    );

    // 参加の行の無い channel の鍵の行だけを足せる。同じ行は足さない。
    assert!(
        store
            .put_private_channel_epoch(&epoch("z", "epoch-9", 90, 9))
            .await
            .unwrap()
    );
    assert!(
        !store
            .put_private_channel_epoch(&epoch("z", "epoch-9", 90, 1))
            .await
            .unwrap()
    );
    assert_eq!(store.get_private_channel_by_id("z").await.unwrap(), None);

    // #1219 AC-2: 鍵更新で予約した世代は、終わるまで (channel, epoch) の順に読め、確定の段の後は cursor を持つ。
    // 参加の行の書き直し(確定)は予約の印を消さない。
    for (channel, epoch_id) in [("z", "epoch-10"), ("a", "epoch-4")] {
        let mut reserved = epoch(channel, epoch_id, 100, 4);
        reserved.rotation_from = Some("epoch-3".into());
        store.put_private_channel_epoch(&reserved).await.unwrap();
    }
    store
        .put_private_channel(
            &channel("t1", "a", "owner", true),
            &[epoch("a", "epoch-4", 100, 4)],
        )
        .await
        .unwrap();
    let pending = |rows: Vec<PrivateChannelEpochRow>| {
        rows.into_iter()
            .map(|row| (row.channel_id, row.epoch_id, row.rotation_after))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        pending(
            store
                .list_private_channel_rotations(("", ""), 8)
                .await
                .unwrap()
        ),
        [
            ("a".to_string(), "epoch-4".to_string(), None),
            ("z".to_string(), "epoch-10".to_string(), None)
        ]
    );
    store
        .set_private_channel_rotation("a", "epoch-4", Some("p"))
        .await
        .unwrap();
    assert_eq!(
        pending(
            store
                .list_private_channel_rotations(("a", "epoch-3"), 1)
                .await
                .unwrap()
        ),
        [(
            "a".to_string(),
            "epoch-4".to_string(),
            Some("p".to_string())
        )]
    );
    store
        .set_private_channel_rotation("a", "epoch-4", None)
        .await
        .unwrap();
    assert_eq!(
        pending(
            store
                .list_private_channel_rotations(("a", "epoch-4"), 8)
                .await
                .unwrap()
        ),
        [("z".to_string(), "epoch-10".to_string(), None)]
    );
    let finished = store
        .get_private_channel_epoch("a", "epoch-4")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (finished.rotation_from, finished.rotation_after),
        (None, None)
    );

    // 鍵の行は channel ごとに page で消す。
    assert_eq!(
        store
            .delete_private_channel_epochs("a", 2)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        store
            .delete_private_channel_epochs("a", 2)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        store
            .list_private_channel_epochs("a", PrivateChannelEpochRange::After(i64::MIN), 8)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn private_channel_key_rows_have_the_same_meaning_on_both_stores() {
    assert_private_channel_keys(&MemoryStore::default()).await;
    let tempdir = tempfile::tempdir().expect("tempdir");
    let sqlite = SqliteStore::connect_file(&tempdir.path().join("private-channel-keys.db"))
        .await
        .expect("sqlite store");
    assert_private_channel_keys(&sqlite).await;
}

// 一覧・世代の範囲・受信 route の識別子の読み出しが、索引の範囲の読み出しになること(件数に比例して走査しない)。
#[tokio::test]
async fn private_channel_key_queries_are_index_range_reads() {
    use crate::sqlite::private_channel_keys::{
        LIST_JOINED_ALL, LIST_JOINED_BY_OWNER, LIST_JOINED_BY_TOPIC,
    };
    use sqlx::Row;

    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect_file(&tempdir.path().join("private-channel-plans.db"))
        .await
        .expect("sqlite store");
    for (name, query, index) in [
        ("all", LIST_JOINED_ALL, "idx_private_channels_joined"),
        ("topic", LIST_JOINED_BY_TOPIC, "idx_private_channels_topic"),
        ("owner", LIST_JOINED_BY_OWNER, "idx_private_channels_owner"),
    ] {
        let mut explain = sqlx::QueryBuilder::<sqlx::Sqlite>::new("EXPLAIN QUERY PLAN ");
        explain.push(query);
        let plan = explain
            .build()
            .fetch_all(store.pool())
            .await
            .expect("query plan")
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(plan.contains(index), "{name}: {plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{name}: {plan}");
    }
}

/// replica へ未書込みの行と相手ごとの cursor（#1218 AC-5b、ADR 0061 §10）。採用・足した行は未書込みで始まり、書いた
/// 版の印で外れる。予約した鍵更新の確定前の世代は一覧に出ない。cursor は自分の行を除いて上限まで。
async fn assert_account_sync_writes<S: AccountSyncStore + PrivateChannelKeyStore>(store: &S) {
    let row = AccountSyncRow {
        key: "profile".into(),
        op_id: "b".repeat(32),
        updated_at: 5,
        value: Some("{}".into()),
    };
    assert!(store.adopt_account_sync_row(&row).await.unwrap());
    assert_eq!(
        store.list_unwritten_account_sync_rows(8).await.unwrap(),
        vec![row.clone()]
    );
    let older = AccountSyncRow {
        updated_at: 4,
        ..row.clone()
    };
    store.mark_account_sync_written(&older).await.unwrap();
    assert_eq!(
        store
            .list_unwritten_account_sync_rows(8)
            .await
            .unwrap()
            .len(),
        1
    );
    store.mark_account_sync_written(&row).await.unwrap();
    assert!(
        store
            .list_unwritten_account_sync_rows(8)
            .await
            .unwrap()
            .is_empty()
    );

    let mut reserved = epoch("c", "epoch-2", 20, 2);
    reserved.rotation_from = Some("epoch-1".into());
    store
        .put_private_channel_epoch(&epoch("c", "epoch-1", 10, 1))
        .await
        .unwrap();
    store.put_private_channel_epoch(&reserved).await.unwrap();
    assert_eq!(
        ids(&store
            .list_unwritten_private_channel_epochs(8)
            .await
            .unwrap()),
        ["epoch-1"]
    );
    // 確定の段の後（配布の cursor を持つ）は、書けていなければ送り直す。
    store
        .set_private_channel_rotation("c", "epoch-2", Some(""))
        .await
        .unwrap();
    assert_eq!(
        ids(&store
            .list_unwritten_private_channel_epochs(8)
            .await
            .unwrap()),
        ["epoch-1", "epoch-2"]
    );
    for epoch_id in ["epoch-1", "epoch-2"] {
        store
            .mark_private_channel_epoch_written("c", epoch_id)
            .await
            .unwrap();
    }
    assert!(
        store
            .list_unwritten_private_channel_epochs(8)
            .await
            .unwrap()
            .is_empty()
    );

    let cursor = |device: &str, updated_at: i64| AccountSyncCursor {
        device_id: device.into(),
        seq: 1,
        head: 1,
        cycle_prefix: None,
        cycle_head: 0,
        updated_at,
    };
    store
        .put_account_sync_cursor(&cursor("own", 0), "own")
        .await
        .unwrap();
    for index in 0..=ACCOUNT_SYNC_CURSOR_LIMIT {
        let device = format!("peer-{index:02}");
        store
            .put_account_sync_cursor(&cursor(&device, index as i64 + 1), "own")
            .await
            .unwrap();
    }
    let kept = store.list_account_sync_cursors().await.unwrap();
    assert_eq!(kept.len(), ACCOUNT_SYNC_CURSOR_LIMIT + 1);
    assert!(kept.iter().any(|cursor| cursor.device_id == "own"));
    assert!(
        store
            .get_account_sync_cursor("peer-00")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn account_sync_writes_and_cursors_have_the_same_meaning_on_both_stores() {
    assert_account_sync_writes(&MemoryStore::default()).await;
    let tempdir = tempfile::tempdir().expect("tempdir");
    let sqlite = SqliteStore::connect_file(&tempdir.path().join("account-sync-writes.db"))
        .await
        .expect("sqlite store");
    assert_account_sync_writes(&sqlite).await;
}
