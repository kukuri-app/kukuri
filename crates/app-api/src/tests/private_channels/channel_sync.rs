//! #1218 AC-4c: 本人の別の端末の private channel の item（参加・世代の鍵・担当）の merge（ADR 0061 §9）。
//! 参加・退会は `(updated_at, op_id)` で採り、鍵は追加だけで欠落を削除としない。退会より古い鍵・古い参加では参加に
//! 戻らない。各 merge は対象の channel の行だけを読む。

use super::super::*;

use kukuri_core::{AccountSyncItem, AccountSyncItemKey, ChannelMembershipV1};
use kukuri_docs_sync::DocFetchPolicy;
use kukuri_store::{PrivateChannelEpochRange, SqliteStore};

const TOPIC: &str = "kukuri:topic:channel-sync";
const OWNER: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn device(
    store: Arc<dyn ProjectionStore>,
    store_base: Arc<dyn Store>,
    keys: &KukuriKeys,
) -> AppService {
    let transport = Arc::new(FakeTransport::new("device-b", FakeNetwork::default()));
    app_service_from_dependencies(
        store_base,
        store,
        transport.clone(),
        transport,
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        keys.clone(),
    )
}

/// 起動時の参加の行の読み直しと、account の replica の秘密の登録（鍵待ちの読み直しはこの replica を読む）。
/// 差分の取得・送り直しの task は起こさない（merge の規則だけを確かめる。取得は `account_sync_fetch` の試験）。
async fn start(app: &AppService) {
    app.restore_joined_private_channels()
        .await
        .expect("restore");
    super::super::account_sync::register_account_replica(app).await;
}

fn epoch(day: i64) -> String {
    format!("epoch-{}-x", day * 86_400_000)
}

fn secret(day: i64) -> String {
    hex::encode([u8::try_from(day).expect("day"); 32])
}

/// 本人の別の端末が書いた参加の item（`current` は参加したときの世代）。
fn joined(channel: &str, updated_at: i64, op: char, current: i64) -> AccountSyncItem {
    AccountSyncItem {
        key: AccountSyncItemKey::ChannelMembership {
            channel_id: ChannelId::new(channel),
        },
        op_id: op.to_string().repeat(32),
        updated_at,
        value: Some(
            serde_json::to_value(ChannelMembershipV1 {
                topic_id: TOPIC.into(),
                label: channel.into(),
                creator_pubkey: OWNER.into(),
                owner_pubkey: OWNER.into(),
                joined_via_pubkey: Some(OWNER.into()),
                audience_kind: ChannelAudienceKind::InviteOnly,
                current_epoch_id: epoch(current),
            })
            .expect("membership"),
        ),
    }
}

/// 本人の別の端末が書いた退会の item。
fn left(channel: &str, updated_at: i64, op: char) -> AccountSyncItem {
    AccountSyncItem {
        key: AccountSyncItemKey::ChannelMembership {
            channel_id: ChannelId::new(channel),
        },
        op_id: op.to_string().repeat(32),
        updated_at,
        value: None,
    }
}

/// 本人の別の端末が受け取った世代の鍵の item。
fn key(channel: &str, day: i64, updated_at: i64) -> AccountSyncItem {
    AccountSyncItem::channel_epoch(
        &ChannelId::new(channel),
        &epoch(day),
        updated_at,
        serde_json::to_value(PrivateChannelEpochCapability {
            epoch_id: epoch(day),
            namespace_secret_hex: secret(day),
        })
        .expect("epoch"),
    )
    .expect("epoch item")
}

async fn current(app: &AppService, channel: &str) -> Option<String> {
    app.joined_private_channel_state(TOPIC, channel)
        .await
        .map(|state| state.current_epoch_id)
}

async fn key_rows(app: &AppService, channel: &str) -> usize {
    app.services
        .projection_store
        .list_private_channel_epochs(channel, PrivateChannelEpochRange::AtOrBefore(i64::MAX), 64)
        .await
        .expect("key rows")
        .len()
}

/// join → 世代の追加 → 退会 → 旧 snapshot の再送 → 明示の再参加 → 容量の回収の後、の固定の遷移。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_items_follow_join_epoch_leave_stale_resend_and_rejoin() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let sqlite = Arc::new(
        SqliteStore::connect_file(&tempdir.path().join("channel-sync.db"))
            .await
            .expect("sqlite"),
    );
    let keys = generate_keys();
    let app = device(sqlite.clone(), sqlite.clone(), &keys);
    start(&app).await;
    let merge = |item: AccountSyncItem| {
        let app = &app;
        async move { app.merge_account_sync_item(item).await.expect("merge") }
    };

    // 参加: 参加の記録が先に届くと鍵待ち(一覧に出ない)。鍵が届くと、その世代で参加する。
    assert!(merge(joined("room", 100, 'a', 1)).await);
    assert_eq!(current(&app, "room").await, None);
    assert!(merge(key("room", 1, 100)).await);
    assert_eq!(current(&app, "room").await, Some(epoch(1)));
    assert_eq!(
        app.joined_private_channel_state(TOPIC, "room")
            .await
            .expect("joined")
            .current_epoch_secret_hex,
        secret(1)
    );
    // 世代の追加: 新しい鍵で現在の世代が進む。同じ鍵の再送・欠落は何も変えない。
    assert!(merge(key("room", 2, 200)).await);
    assert_eq!(current(&app, "room").await, Some(epoch(2)));
    assert!(!merge(key("room", 1, 100)).await);
    assert_eq!(key_rows(&app, "room").await, 2);

    // 退会: 参加を外し、鍵の行を消す。
    assert!(merge(left("room", 300, 'b')).await);
    assert_eq!(current(&app, "room").await, None);
    assert_eq!(key_rows(&app, "room").await, 0);
    // 旧 snapshot の再送: 退会より古い参加・鍵では参加に戻らず、鍵も保存しない。
    assert!(!merge(joined("room", 100, 'a', 1)).await);
    assert!(!merge(key("room", 1, 100)).await);
    assert!(!merge(key("room", 2, 300)).await);
    assert_eq!(current(&app, "room").await, None);
    assert_eq!(key_rows(&app, "room").await, 0);

    // 明示の再参加: 新しい参加の版と、その後に受け取った鍵で、新しい世代に参加する(退会の行に残る世代へ戻らない)。
    assert!(merge(joined("room", 400, 'c', 3)).await);
    assert_eq!(current(&app, "room").await, None);
    assert!(merge(key("room", 3, 400)).await);
    assert_eq!(current(&app, "room").await, Some(epoch(3)));

    // 容量の回収は、鍵と参加の行を消さない。
    while sqlite.reclaim_remote_cache_step().await.expect("reclaim") > 0 {}
    assert_eq!(current(&app, "room").await, Some(epoch(3)));
    assert_eq!(key_rows(&app, "room").await, 1);
}

/// 鍵待ちになったら、手元の account の replica から、その channel の鍵を新しい順に読み直す(退会で消した鍵は、
/// 差分の取得の cursor を既に過ぎている)。最新の世代で参加する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_membership_rereads_the_channel_keys_from_the_local_replica() {
    let store = Arc::new(MemoryStore::default());
    let keys = generate_keys();
    let app = device(store.clone(), store, &keys);
    start(&app).await;
    // 鍵の item は手元の account の replica にある(差分の取得で読み過ぎたもの)。
    for day in [1, 2, 3] {
        app.write_account_sync_item(&key("room", day, 100))
            .await
            .expect("replica item");
    }
    assert!(
        app.merge_account_sync_item(joined("room", 100, 'a', 1))
            .await
            .expect("merge")
    );
    assert_eq!(current(&app, "room").await, Some(epoch(3)));
    assert_eq!(key_rows(&app, "room").await, 3);

    // 読み直しが鍵に届く前に止まっても、同じ版の再送で読み直しを再開する。
    let late = joined("late", 100, 'a', 1);
    assert!(
        app.merge_account_sync_item(late.clone())
            .await
            .expect("merge")
    );
    assert_eq!(current(&app, "late").await, None);
    for day in [1, 2] {
        app.write_account_sync_item(&key("late", day, 100))
            .await
            .expect("replica item");
    }
    app.merge_account_sync_item(late).await.expect("resend");
    assert_eq!(current(&app, "late").await, Some(epoch(2)));
}

/// 担当の記録は ADR 0018 §8 の規則で採る(世代の大きい方、同じ世代で同じ端末なら移譲中の記録)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_items_follow_the_generation_rule() {
    let store = Arc::new(MemoryStore::default());
    let keys = generate_keys();
    let app = device(store.clone(), store, &keys);
    start(&app).await;
    app.merge_account_sync_item(joined("room", 100, 'a', 1))
        .await
        .expect("join");
    app.merge_account_sync_item(key("room", 1, 100))
        .await
        .expect("key");
    let controller = |generation: u64, device: &str, transfer_to: Option<&str>| AccountSyncItem {
        key: AccountSyncItemKey::ChannelController {
            channel_id: ChannelId::new("room"),
        },
        op_id: "d".repeat(32),
        updated_at: 1,
        value: Some(
            serde_json::to_value(PrivateChannelController {
                device_id: device.into(),
                generation,
                transfer_to: transfer_to.map(str::to_string),
            })
            .expect("controller"),
        ),
    };
    let merge = |item: AccountSyncItem| {
        let app = &app;
        async move { app.merge_account_sync_item(item).await.expect("merge") }
    };
    assert!(merge(controller(1, "a", None)).await);
    assert!(merge(controller(1, "a", Some("b"))).await);
    // 移譲の停止は戻らず、同じ世代の別の端末・古い世代は採らない。
    assert!(!merge(controller(1, "a", None)).await);
    assert!(!merge(controller(1, "c", None)).await);
    assert!(merge(controller(2, "b", None)).await);
    assert!(!merge(controller(1, "a", Some("b"))).await);
    let state = app
        .joined_private_channel_state(TOPIC, "room")
        .await
        .expect("joined");
    assert_eq!(
        state.controller,
        Some(PrivateChannelController {
            device_id: "b".into(),
            generation: 2,
            transfer_to: None,
        })
    );
}

/// 各 merge が読み書きする行の数は、他の channel を 10 倍にしても変わらない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_merge_touches_only_its_channel_regardless_of_other_channels() {
    async fn measure(others: usize) -> Vec<usize> {
        let store = Arc::new(MemoryStore::default());
        let keys = generate_keys();
        let app = device(store.clone(), store.clone(), &keys);
        app.gossip_disabled_topics
            .lock()
            .await
            .insert(TOPIC.to_string());
        start(&app).await;
        for index in 0..others {
            let channel = format!("other-{index:05}");
            app.merge_account_sync_item(joined(&channel, 100, 'a', 1))
                .await
                .expect("other join");
            app.merge_account_sync_item(key(&channel, 1, 100))
                .await
                .expect("other key");
        }
        let mut counts = Vec::new();
        for item in [
            joined("room", 100, 'a', 1),
            key("room", 1, 100),
            key("room", 2, 200),
            left("room", 300, 'b'),
            key("room", 1, 100),
        ] {
            let before = store.private_channel_key_rows_touched();
            app.merge_account_sync_item(item).await.expect("merge");
            counts.push(store.private_channel_key_rows_touched() - before);
        }
        counts
    }
    // 両方とも lease の上限(64)の内側で比べる(上限の外の channel は購読しないので、購読の作り直しの読みが無い)。
    assert_eq!(measure(6).await, measure(60).await);
}

/// 購読の枠(64)を超えて同期された channel は、一覧に出し、購読しない(2026-10-02 の決定)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_over_the_scope_limit_are_listed_but_not_subscribed() {
    let store = Arc::new(MemoryStore::default());
    let keys = generate_keys();
    let app = device(store.clone(), store, &keys);
    app.gossip_disabled_topics
        .lock()
        .await
        .insert(TOPIC.to_string());
    start(&app).await;
    // 枠より 1 件多く同期すると、最後の 1 件が枠の外になる。
    let channels = (0..=MAX_ACTIVE_SCOPES)
        .map(|index| format!("room-{index:02}"))
        .collect::<Vec<_>>();
    for channel in &channels {
        app.merge_account_sync_item(joined(channel, 100, 'a', 1))
            .await
            .expect("join");
        app.merge_account_sync_item(key(channel, 1, 100))
            .await
            .expect("key");
    }
    let leases = app.subscription_registry.scope_leases.lock().await.keys();
    let subscribed =
        |channel: &str| leases.contains(&ScopeKey::Channel(TOPIC.to_string(), channel.to_string()));
    assert!(subscribed(&channels[0]));
    assert!(!subscribed(&channels[MAX_ACTIVE_SCOPES]));
    let mut listed = Vec::new();
    let mut cursor = None;
    loop {
        let page = app
            .list_joined_private_channels(TOPIC, cursor.as_deref())
            .await
            .expect("list");
        listed.extend(page.items.into_iter().map(|item| item.channel_id));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(listed.len(), MAX_ACTIVE_SCOPES + 1);
}

/// 退会の item と、この端末の redeem・別の端末の世代の追加が並行しても、参加に戻らない(ADR 0061 §9 の判定)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn leave_races_with_epoch_advances_without_rejoining() {
    let store = Arc::new(MemoryStore::default());
    let keys = generate_keys();
    let app = device(store.clone(), store, &keys);
    start(&app).await;
    app.merge_account_sync_item(joined("room", 100, 'a', 1))
        .await
        .expect("join");
    app.merge_account_sync_item(key("room", 1, 100))
        .await
        .expect("key");
    let mut redeemed = app
        .joined_private_channel_state(TOPIC, "room")
        .await
        .expect("joined");
    redeemed.current_epoch_id = epoch(2);
    redeemed.current_epoch_secret_hex = secret(2);

    // 別の端末の退会を採った後に、この端末の redeem(世代 1 → 2)が確定しようとする。
    app.merge_account_sync_item(left("room", 300, 'b'))
        .await
        .expect("leave");
    let advanced = app
        .commit_joined_private_channel_state(
            redeemed,
            ChannelCommit::Advance {
                from_epoch: &epoch(1),
            },
        )
        .await
        .expect("advance");
    assert_eq!(advanced, None);
    assert_eq!(current(&app, "room").await, None);

    // 退会を知らない別の端末の redeem の鍵(退会より新しい時刻)は残るが、参加にはしない。
    assert!(
        app.merge_account_sync_item(key("room", 3, 350))
            .await
            .expect("key after leave")
    );
    assert_eq!(current(&app, "room").await, None);
    assert_eq!(key_rows(&app, "room").await, 1);
}

/// 参加の行の無い端末に、鍵が退会より先に届いても、退会で鍵を消す(key の順は鍵が参加の版より先)。その後の再参加は、
/// 消した鍵の世代に戻らず鍵待ちになる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn leave_removes_keys_that_arrived_before_any_membership() {
    let store = Arc::new(MemoryStore::default());
    let keys = generate_keys();
    let app = device(store.clone(), store, &keys);
    start(&app).await;
    assert!(
        app.merge_account_sync_item(key("room", 1, 100))
            .await
            .expect("key")
    );
    assert_eq!(key_rows(&app, "room").await, 1);
    assert!(
        app.merge_account_sync_item(left("room", 300, 'b'))
            .await
            .expect("leave")
    );
    assert_eq!(key_rows(&app, "room").await, 0);
    assert!(
        app.merge_account_sync_item(joined("room", 400, 'c', 2))
            .await
            .expect("rejoin")
    );
    assert_eq!(current(&app, "room").await, None);
}

/// この端末の参加・世代の追加・退会が書く item: 参加は参加の版と現在の世代の鍵、世代の追加は鍵だけ(参加の版を
/// 変えない)、退会は tombstone。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_join_rotate_and_leave_write_their_items() {
    let store = Arc::new(MemoryStore::default());
    let keys = generate_keys();
    let app = device(store.clone(), store, &keys);
    start(&app).await;
    let _ = app.list_timeline(TOPIC, None, 20).await;
    let channel_id = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(TOPIC),
            label: "writes".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create")
        .channel_id;
    let channel = ChannelId::new(channel_id.as_str());
    let read = |key: AccountSyncItemKey| {
        let app = &app;
        async move {
            app.read_account_sync_item(&key, DocFetchPolicy::LocalOnly)
                .await
                .expect("read item")
        }
    };
    let membership_key = AccountSyncItemKey::ChannelMembership {
        channel_id: channel.clone(),
    };
    let epoch_key = |epoch_id: String| AccountSyncItemKey::ChannelCapability {
        channel_id: channel.clone(),
        epoch_id,
    };
    let first = app
        .joined_private_channel_state(TOPIC, channel_id.as_str())
        .await
        .expect("joined");
    let joined_item = read(membership_key.clone()).await.expect("membership");
    let value: ChannelMembershipV1 =
        serde_json::from_value(joined_item.value.clone().expect("value")).expect("membership");
    assert_eq!(value.current_epoch_id, first.current_epoch_id);
    assert!(
        read(epoch_key(first.current_epoch_id.clone()))
            .await
            .is_some()
    );

    let rotated = app
        .rotate_private_channel(TOPIC, channel_id.as_str())
        .await
        .expect("rotate");
    assert_ne!(rotated.current_epoch_id, first.current_epoch_id);
    assert!(read(epoch_key(rotated.current_epoch_id)).await.is_some());
    assert_eq!(read(membership_key.clone()).await, Some(joined_item));

    app.leave_private_channel(TOPIC, channel_id.as_str())
        .await
        .expect("leave");
    let tombstone = read(membership_key).await.expect("tombstone");
    assert_eq!(tombstone.value, None);
}
