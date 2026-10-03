//! #1219 W6 AC-3: 担当の記録と新しい世代の鍵を本人の別の端末へ届け、担当でない端末の鍵更新を伴う共有・投稿を
//! account 同期の依頼で担当へ渡す（ADR 0018 §8、ADR 0061 §9）。

use super::*;
use crate::{PrivateChannelCapability, PrivateChannelControllerPending};
use kukuri_core::ChannelRotationRequestV1;
use kukuri_store::{
    PrivateChannelEpochRange, PrivateChannelKeyStore, PrivateChannelParticipantRow,
};

pub(super) const TOPIC: &str = "kukuri:topic:controller-requests";

/// 起動の復元（private の世代の秘密の参照を docs へ入れる）を済ませた端末。
pub(super) async fn restored(apps: &[&AppService]) {
    for app in apps {
        app.restore_joined_private_channels()
            .await
            .expect("restore");
    }
}

pub(super) async fn create(app: &AppService, audience_kind: ChannelAudienceKind) -> String {
    let _ = app.list_timeline(TOPIC, None, 20).await;
    app.create_private_channel(CreatePrivateChannelInput {
        topic_id: TopicId::new(TOPIC),
        label: "requests".into(),
        audience_kind,
    })
    .await
    .expect("create private channel")
    .channel_id
}

async fn state(app: &AppService, channel_id: &str) -> JoinedPrivateChannelState {
    app.joined_private_channel_state(TOPIC, channel_id)
        .await
        .expect("joined channel")
}

/// 参加の行の現在の世代（正本。メモリは lease の task が行から読み直す）。
async fn epoch(app: &AppService, channel_id: &str) -> String {
    app.services
        .projection_store
        .get_private_channel_by_id(channel_id)
        .await
        .expect("channel row")
        .expect("joined channel")
        .current_epoch_id
}

async fn post(app: &AppService, channel_id: &str) -> Result<String> {
    app.create_post_in_channel(
        TOPIC,
        ChannelRef::PrivateChannel {
            channel_id: ChannelId::new(channel_id),
        },
        "private post",
        None,
    )
    .await
}

fn controller_a() -> Option<PrivateChannelController> {
    Some(PrivateChannelController {
        device_id: "device-a".into(),
        generation: 1,
        transfer_to: None,
    })
}

/// B が鍵更新の予約・凍結・grant の配布のどれも行っていない。
async fn assert_b_created_nothing(store: &MemoryStore) {
    assert!(
        store
            .list_private_channel_rotations(("", ""), 8)
            .await
            .expect("rotations")
            .is_empty(),
        "no rotation is reserved on B"
    );
    assert!(
        store
            .list_direct_message_outbox()
            .await
            .expect("outbox")
            .is_empty(),
        "no handoff grant is queued on B"
    );
}

/// C1・C2: 担当の記録は周回でも変更の窓でも B に届く（旧保存の移行の分は送り直しで届く）。A の鍵更新の後、取得元が
/// 不在の間は B は旧い世代のまま未同期を示し、A が戻ったら新しい世代の鍵を保存してから現在の世代を進め、投稿できる。
/// 旧い世代の item の再送では戻らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_controller_and_a_new_epoch_reach_the_other_device() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    restored(&[&a, &b]).await;
    let first = create(&a, ChannelAudienceKind::InviteOnly).await;
    // 初めての相手は周回で読む（key の順で担当の item が参加の item より先に届く）。
    b.fetch_account_sync_from("device-a").await.expect("cycle");
    assert_eq!(state(&b, &first).await.controller, controller_a());
    // 変更の窓で読む。
    let second = create(&a, ChannelAudienceKind::InviteOnly).await;
    b.fetch_account_sync_from("device-a").await.expect("window");
    assert_eq!(state(&b, &second).await.controller, controller_a());
    // 旧保存の移行で担当になった分は、時刻 0 の行を送り直しで書く。
    let legacy = a
        .get_private_channel_capability(TOPIC, &first)
        .await
        .expect("capability")
        .expect("joined");
    let legacy_channel = "channel-legacy-controller";
    a.restore_private_channel_capability(PrivateChannelCapability {
        channel_id: legacy_channel.into(),
        controller: None,
        ..legacy
    })
    .await
    .expect("legacy capability");
    a.resend_account_sync_items().await.expect("resend");
    b.fetch_account_sync_from("device-a").await.expect("resent");
    assert_eq!(state(&b, legacy_channel).await.controller, controller_a());

    let old = epoch(&a, &first).await;
    let rotated = a
        .rotate_private_channel(TOPIC, &first)
        .await
        .expect("the controller rotates")
        .current_epoch_id;
    assert_ne!(rotated, old);

    // 取得元が不在: B は旧い世代のまま、未同期を示す（鍵の保存より前に新しい世代を使わない）。
    a_docs.reachable(0);
    assert!(b.fetch_account_sync_from("device-a").await.is_err());
    assert_eq!(epoch(&b, &first).await, old);
    assert!(b.account_sync_status().await.expect("status").fetch_failed);

    // A が戻ったら取り直し、鍵の行を保存してから現在の世代を進める。
    a_docs.reachable(usize::MAX);
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    assert!(
        b.services
            .projection_store
            .get_private_channel_epoch(&first, &rotated)
            .await
            .expect("epoch row")
            .is_some(),
        "the new key is saved"
    );
    assert_eq!(epoch(&b, &first).await, rotated);
    assert!(!b.account_sync_status().await.expect("status").fetch_failed);
    assert_eq!(
        state(&b, &first).await.current_epoch_id,
        rotated,
        "the post uses the new epoch"
    );
    post(&b, &first)
        .await
        .expect("the other device posts with the new epoch");

    // 旧い世代の item の再送では戻らない。
    let stale = b
        .read_account_sync_item(
            &AccountSyncItemKey::ChannelCapability {
                channel_id: ChannelId::new(first.as_str()),
                epoch_id: old.clone(),
            },
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("read the old epoch item")
        .expect("the old epoch item");
    assert!(
        !b.merge_account_sync_item(stale)
            .await
            .expect("merge the old epoch item")
    );
    assert_eq!(epoch(&b, &first).await, rotated);
}

/// 端末の変更の窓の head の seq（書いた item の数）。
async fn head(app: &AppService, device_id: &str) -> u64 {
    app.read_account_sync_item(
        &AccountSyncItemKey::ChangeHead {
            device_id: device_id.into(),
        },
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("read the head")
    .and_then(|item| item.value)
    .map_or(0, |value| {
        serde_json::from_value::<kukuri_core::AccountSyncChangeV1>(value)
            .expect("head value")
            .seq
    })
}

/// 参加の行の無い端末どうし（作成した端末で退会した channel を、後から取得した 2 端末）でも、担当の item は同じ版で
/// 往復を止める（ADR 0061 §10。#1219 AC-3 監査 B-1）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_controller_item_without_a_channel_row_stops_at_the_same_version() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    let (c, c_docs) = memory_device(&keys, "device-c").await;
    restored(&[&a, &b, &c]).await;
    let channel = create(&a, ChannelAudienceKind::InviteOnly).await;
    a.leave_private_channel(TOPIC, &channel)
        .await
        .expect("leave");
    b_docs.reading_from(&a_docs);
    b.fetch_account_sync_from("device-a")
        .await
        .expect("b from a");
    c_docs.reading_from(&b_docs);
    c.fetch_account_sync_from("device-b")
        .await
        .expect("c from b");
    for app in [&b, &c] {
        assert!(
            app.services
                .projection_store
                .get_private_channel_by_id(&channel)
                .await
                .expect("row")
                .is_none_or(|row| !row.joined),
            "the left channel is not joined"
        );
    }

    let (b_head, c_head) = (head(&b, "device-b").await, head(&c, "device-c").await);
    b_docs.reading_from(&c_docs);
    for _ in 0..2 {
        b.fetch_account_sync_from("device-c")
            .await
            .expect("b from c");
        c.fetch_account_sync_from("device-b")
            .await
            .expect("c from b");
    }
    assert_eq!(head(&b, "device-b").await, b_head, "b writes nothing again");
    assert_eq!(head(&c, "device-c").await, c_head, "c writes nothing again");
}

/// C3: 担当が本人の端末の候補にいないと、B の共有はすぐ保留になり、依頼だけが残る。担当が取得したときに鍵を更新し、
/// 同じ依頼の再受信では更新しない。B は取得で新しい世代へ移る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_without_the_controller_online_is_held_until_it_fetches() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let b_docs = DeviceDocs::new();
    let b_store = Arc::new(MemoryStore::default());
    let b = own_device(&keys, "device-b", &b_docs, b_store.clone(), b_store.clone()).await;
    b_docs.reading_from(&a_docs);
    a_docs.reading_from(&b_docs);
    restored(&[&a, &b]).await;
    let channel = create(&a, ChannelAudienceKind::InviteOnly).await;
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    let old = epoch(&b, &channel).await;

    let started = std::time::Instant::now();
    let held = b
        .export_channel_access_token(TOPIC, &channel, None)
        .await
        .expect_err("the controller is not online");
    assert!(held.is::<PrivateChannelControllerPending>());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "an offline controller is not waited for"
    );
    assert_eq!(epoch(&b, &channel).await, old);
    assert_b_created_nothing(&b_store).await;
    let request = b
        .read_account_sync_item(
            &AccountSyncItemKey::ChannelRotationRequest {
                channel_id: ChannelId::new(channel.as_str()),
            },
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("read the request")
        .expect("the request stays");
    assert_eq!(
        request.value,
        Some(
            serde_json::to_value(ChannelRotationRequestV1 {
                from_epoch_id: old.clone(),
            })
            .expect("request value")
        )
    );

    a.fetch_account_sync_from("device-b")
        .await
        .expect("the controller fetches the request");
    let rotated = epoch(&a, &channel).await;
    assert_ne!(rotated, old, "the controller rotates for the request");
    a.merge_account_sync_item(request)
        .await
        .expect("the same request again");
    assert_eq!(epoch(&a, &channel).await, rotated, "only once");

    b.fetch_account_sync_from("device-a").await.expect("fetch");
    assert_eq!(epoch(&b, &channel).await, rotated);
}

/// 本人の端末の候補を `peers` で返す、hint の流れる 2 端末（account 同期の task を起こす）。
pub(super) async fn running_devices(
    keys: &KukuriKeys,
) -> (AppService, AppService, Arc<MemoryStore>, Arc<MemoryStore>) {
    let network = FakeNetwork::default();
    let mut devices = Vec::new();
    for (id, other) in [("device-a", "device-b"), ("device-b", "device-a")] {
        let docs = DeviceDocs::new();
        let store = Arc::new(MemoryStore::default());
        let transport = Arc::new(FakeTransport::new(id, network.clone()));
        let hints = CandidateHints {
            inner: transport.clone(),
            peers: Arc::new(std::sync::Mutex::new(vec![kukuri_transport::SeedPeer {
                endpoint_id: other.into(),
                addr_hint: None,
            }])),
        };
        let app = app_service_from_dependencies(
            store.clone(),
            store.clone(),
            transport,
            Arc::new(hints),
            Arc::new(docs.clone()),
            Arc::new(MemoryBlobService::default()),
            keys.clone(),
        );
        devices.push((app, docs, store));
    }
    let [(a, a_docs, a_store), (b, b_docs, b_store)] =
        <[_; 2]>::try_from(devices).ok().expect("two devices");
    a_docs.reading_from(&b_docs);
    b_docs.reading_from(&a_docs);
    restored(&[&a, &b]).await;
    for app in [&a, &b] {
        app.start_account_sync().await.expect("account sync");
    }
    (a, b, a_store, b_store)
}

pub(super) async fn joined_on(app: &AppService, channel_id: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while app
            .joined_private_channel_state(TOPIC, channel_id)
            .await
            .is_none()
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the channel reaches the other device");
}

/// C3: 担当 A が本人の端末の候補にいれば、B の共有（invite_only）と、資格を失った参加者のいる friend_only への投稿は、
/// A への依頼で A が鍵を更新し、新しい世代が届いたところで完了する。世代を作るのは A の 1 回だけで、B は作らない。
/// 表示のための「書けるか」の判定は、A が online でも依頼も待ちもしない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_online_controller_rotates_for_a_share_and_a_post_on_the_other_device() {
    let keys = generate_keys();
    let (a, b, a_store, b_store) = running_devices(&keys).await;

    let shared = create(&a, ChannelAudienceKind::InviteOnly).await;
    joined_on(&b, &shared).await;
    let old = epoch(&b, &shared).await;
    let token = b
        .export_channel_access_token(TOPIC, &shared, None)
        .await
        .expect("the controller rotates for the share")
        .token;
    let rotated = epoch(&a, &shared).await;
    assert_ne!(rotated, old);
    assert_eq!(epoch(&b, &shared).await, rotated);
    assert_eq!(
        kukuri_core::parse_private_channel_invite_token(&token)
            .expect("invite token")
            .epoch_id,
        rotated,
        "B shares the new epoch"
    );
    assert_eq!(
        a_store
            .list_private_channel_epochs(&shared, PrivateChannelEpochRange::AtOrBefore(i64::MAX), 8)
            .await
            .expect("epochs")
            .len(),
        2,
        "one rotation"
    );
    assert_b_created_nothing(&b_store).await;

    let posted = create(&a, ChannelAudienceKind::FriendOnly).await;
    joined_on(&b, &posted).await;
    let old = epoch(&b, &posted).await;
    // 相互 follow でなくなった参加者（資格を失った人）。
    let stale = generate_keys().public_key_hex();
    for store in [&a_store, &b_store] {
        store
            .upsert_follow_edge(FollowEdge {
                subject_pubkey: Pubkey::from(stale.as_str()),
                target_pubkey: Pubkey::from(keys.public_key_hex()),
                status: FollowEdgeStatus::Active,
                updated_at: 1,
                envelope_id: EnvelopeId::from("follow-of-the-stale-participant"),
            })
            .await
            .expect("one-way follow");
        store
            .put_private_channel_participant(PrivateChannelParticipantRow {
                channel_id: posted.clone(),
                epoch_id: old.clone(),
                participant_pubkey: stale.clone(),
                left_at: None,
                updated_at: 1,
            })
            .await
            .expect("stale participant");
    }
    // 表示のための「書けるか」の判定は、依頼も待ちもせずに保留する（2026-10-03 ユーザー判断）。
    let started = std::time::Instant::now();
    let checked = b
        .private_channel_state_for_owner_action(
            TOPIC,
            &ChannelId::new(posted.as_str()),
            PrivateChannelOwnerAction::WriteCheck,
        )
        .await
        .expect_err("the check does not rotate on the other device");
    assert!(checked.is::<PrivateChannelControllerPending>());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(
        b.read_account_sync_item(
            &AccountSyncItemKey::ChannelRotationRequest {
                channel_id: ChannelId::new(posted.as_str()),
            },
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("read the request")
        .is_none(),
        "the check writes no request"
    );
    assert_eq!(epoch(&a, &posted).await, old);
    post(&b, &posted)
        .await
        .expect("the controller rotates for the post");
    let rotated = epoch(&a, &posted).await;
    assert_ne!(rotated, old);
    assert_eq!(epoch(&b, &posted).await, rotated);
    assert_eq!(
        state(&b, &posted).await.current_epoch_id,
        rotated,
        "B posts with the new epoch"
    );
    assert_b_created_nothing(&b_store).await;
}
