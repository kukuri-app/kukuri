//! #1218 AC-4b: private channel の参加と世代の鍵の行（ADR 0061 §9）を、SQLite と Memory の両方で同じ意味に保つ。

use super::*;
use crate::{
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
